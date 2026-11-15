//! Native notebook lifecycle: creation layout, validity matrix,
//! confinement, enumeration, concurrent creation, and true-`nb`
//! interoperability.
//!
//! Everything here except the explicitly `#[ignore]`d oracle test
//! runs in the default suite with zero `nb` spawns: fixtures are
//! assembled with `git` and the filesystem only. The ignored test
//! belongs to the extended oracle profile.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, NbError};

use crate::common::with_isolated_env;
use crate::native_reads_support::NbOracle;

fn config_for(env: &NbTestEnv) -> Config {
    Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: false,
        allow_top_level_notes: true,
        disable_git_signing: true,
        ..Config::default()
    }
}

fn config_for_create(env: &NbTestEnv) -> Config {
    Config {
        create_notebook: true,
        ..config_for(env)
    }
}

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let mut cmd = std::process::Command::new("git");
    nb_api::scrub_git_env_std(&mut cmd);
    cmd.current_dir(root).args(args);
    let out = cmd.output().expect("git runs");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Creation replicates the `nb notebooks add` layout and the result
/// is immediately committable.
#[tokio::test]
async fn create_resolves_layout_and_transacts_immediately() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let root = client
            .show_notebook_path(Some("fresh-native"))
            .await
            .expect("create");
        assert!(root.join(".index").is_file(), "empty index exists");
        assert_eq!(
            std::fs::read(root.join(".index")).expect("read").len(),
            0,
            "index starts empty"
        );
        assert!(
            !git(&root, &["rev-parse", "HEAD"]).trim().is_empty(),
            "HEAD resolves immediately"
        );
        assert_eq!(
            git(&root, &["ls-files"]).trim(),
            ".index",
            "only the index is tracked"
        );

        let mut tx =
            client.transaction(Some("fresh-native")).await.expect("tx");
        tx.add_note("hello.md", Some("Hello"), "hello\n", &[])
            .unwrap();
        tx.commit().await.expect("immediate commit works");
        assert!(root.join("hello.md").is_file(), "note landed");
    })
    .await;
}

/// A missing notebook is created on write paths when allowed.
#[tokio::test]
async fn missing_notebook_write_path_creates_when_allowed() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let root = client
            .show_notebook_path(Some("brand-new"))
            .await
            .expect("create");
        assert!(root.is_dir(), "notebook directory created");
    })
    .await;
}

/// A missing notebook refuses on write paths when creation is off,
/// without creating anything.
#[tokio::test]
async fn missing_notebook_write_path_refuses_when_disabled() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let err = client
            .show_notebook_path(Some("nope"))
            .await
            .expect_err("missing notebook must refuse");
        match err {
            NbError::ValidationError { reason, .. } => {
                assert!(
                    reason.contains("automatic creation is disabled"),
                    "unexpected reason: {reason:?}"
                );
            },
            other => {
                panic!("expected ValidationError, got: {other:?}");
            },
        }
        assert!(
            !env.nb_dir().join("nope").exists(),
            "refusal creates nothing"
        );
    })
    .await;
}

/// Read paths never create: a missing notebook is `NotFound` even
/// when creation is enabled.
#[tokio::test]
async fn missing_notebook_read_path_is_not_found() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let err = client
            .show_note("whatever", Some("nope"))
            .await
            .expect_err("missing notebook must be NotFound");
        assert!(
            matches!(err, NbError::NotFound { .. }),
            "expected NotFound, got: {err:?}"
        );
        assert!(!env.nb_dir().join("nope").exists(), "reads never create");
    })
    .await;
}

/// A plain directory is never git-initialized by ensure.
#[tokio::test]
async fn plain_directory_is_never_initialized() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        std::fs::create_dir(env.nb_dir().join("plain")).expect("plain dir");
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let err = client
            .show_notebook_path(Some("plain"))
            .await
            .expect_err("plain directory must refuse");
        match err {
            NbError::ValidationError { reason, .. } => {
                assert!(
                    reason.contains("refusing to initialize"),
                    "unexpected reason: {reason:?}"
                );
            },
            other => {
                panic!("expected ValidationError, got: {other:?}");
            },
        }
        assert!(
            !env.nb_dir().join("plain/.git").exists(),
            "plain directory left alone"
        );
    })
    .await;
}

/// Dot-segment names are not notebook names on any path.
#[tokio::test]
async fn dot_names_refused() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        for name in [".", ".."] {
            let err = client
                .show_notebook_path(Some(name))
                .await
                .expect_err("dot name must refuse");
            assert!(
                matches!(err, NbError::ValidationError { .. }),
                "expected ValidationError for {name:?}, got: {err:?}"
            );
        }
    })
    .await;
}

/// Enumeration returns sorted valid notebook names and skips
/// anything else without touching it.
#[tokio::test]
async fn enumeration_lists_valid_notebooks_sorted() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        client
            .show_notebook_path(Some("zzz-native"))
            .await
            .expect("create zzz");
        client
            .show_notebook_path(Some("aaa-native"))
            .await
            .expect("create aaa");
        std::fs::create_dir(env.nb_dir().join("plain")).expect("plain dir");
        std::fs::create_dir(env.nb_dir().join("bad:name"))
            .expect("invalid-name dir");

        let names = client.list_notebook_names().await.expect("enumerate");
        let mut expected = vec![
            env.notebook().to_string(),
            "aaa-native".to_string(),
            "zzz-native".to_string(),
        ];
        expected.sort();
        assert_eq!(names, expected, "sorted valid notebooks only");
        assert!(
            !env.nb_dir().join("plain/.git").exists(),
            "enumeration initializes nothing"
        );
    })
    .await;
}

/// Concurrent creators converge on one notebook: every caller
/// succeeds and exactly one initial commit exists.
#[tokio::test]
async fn concurrent_creates_converge_on_single_notebook() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let barrier = std::sync::Barrier::new(4);
        let results = std::thread::scope(|s| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    s.spawn(|| {
                        barrier.wait();
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("runtime");
                        rt.block_on(async {
                            let client =
                                NbClient::new(&config_for_create(&env))
                                    .expect("client");
                            client.show_notebook_path(Some("race")).await
                        })
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("thread"))
                .collect::<Vec<_>>()
        });
        assert_eq!(results.len(), 4, "all creators finished");
        for result in &results {
            assert!(result.is_ok(), "every creator succeeds: {result:?}");
        }
        let first = results[0]
            .as_ref()
            .expect("first creator succeeded")
            .clone();
        for result in &results[1..] {
            assert_eq!(
                result.as_ref().expect("creator succeeded"),
                &first,
                "all creators resolve the same notebook"
            );
        }
        assert_eq!(
            git(&first, &["rev-list", "--count", "HEAD"]).trim(),
            "1",
            "exactly one initial commit"
        );
    })
    .await;
}

/// Symlink notebook roots are refused without following them.
/// Unix-only: directory symlinks need privileges on Windows.
#[cfg(unix)]
#[tokio::test]
async fn symlink_root_is_refused() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let target = env.nb_dir().join(env.notebook());
        std::os::unix::fs::symlink(&target, env.nb_dir().join("alias"))
            .expect("symlink");
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let err = client
            .show_notebook_path(Some("alias"))
            .await
            .expect_err("symlink root must refuse");
        assert!(
            matches!(err, NbError::UnsupportedStructure { .. }),
            "expected UnsupportedStructure, got: {err:?}"
        );
        assert!(!target.join("alias").exists(), "target left untouched");
    })
    .await;
}

/// An `nb`-created notebook resolves natively (no `nb` spawns).
#[tokio::test]
async fn interop_nb_created_notebook_resolves_natively() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let root = client
            .show_notebook_path(Some(env.notebook()))
            .await
            .expect("nb-created notebook resolves");
        assert!(
            !git(&root, &["rev-parse", "HEAD"]).trim().is_empty(),
            "HEAD resolves"
        );
        let names = client.list_notebook_names().await.expect("enumerate");
        assert!(
            names.contains(&env.notebook().to_string()),
            "nb-created notebook enumerated: {names:?}"
        );
    })
    .await;
}

/// A natively created notebook opens in true `nb`.
/// Extended oracle suite only: spawns the real CLI.
#[ignore]
#[tokio::test]
async fn interop_native_created_notebook_opens_in_nb() {
    let env = NbTestEnv::new().expect("fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let root = client
            .show_notebook_path(Some("interop-native"))
            .await
            .expect("create");
        let mut tx = client
            .transaction(Some("interop-native"))
            .await
            .expect("tx");
        tx.add_note("hello.md", Some("Hello"), "hello\n", &[])
            .unwrap();
        tx.commit().await.expect("seed note");
        assert!(root.join("hello.md").is_file());

        let oracle = NbOracle::new(&env);
        assert!(
            oracle.show_ok("interop-native:hello.md"),
            "true nb opens the natively created notebook"
        );
    })
    .await;
}
