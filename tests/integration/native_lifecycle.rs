//! Native notebook lifecycle: creation layout, validity matrix,
//! confinement, enumeration, concurrent creation, failure
//! injection, and true-`nb` interoperability.
//!
//! The lifecycle default tests are hermetic: fixtures are built
//! natively (git only, no `nb` spawn including setup) and every
//! unignored lifecycle test passes with a dead `nb` on `PATH` (see
//! the tripwire below). The two interop tests need a real `nb` and
//! belong to the ignore-gated oracle set for the extended
//! profile.

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

/// Creation lays out the exact `nb` notebook shape and the result
/// is immediately committable.
#[tokio::test]
async fn create_resolves_layout_and_transacts_immediately() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let root = client
            .create_notebook(Some("fresh-native"))
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

/// Explicit creation refuses an existing notebook instead of
/// converging onto it.
#[tokio::test]
async fn explicit_create_refuses_existing() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let first = client
            .create_notebook(Some("once"))
            .await
            .expect("first create");
        let err = client
            .create_notebook(Some("once"))
            .await
            .expect_err("second create must refuse");
        match err {
            NbError::ValidationError { reason, .. } => {
                assert!(
                    reason.contains("already exists"),
                    "unexpected reason: {reason:?}"
                );
            },
            other => {
                panic!("expected ValidationError, got: {other:?}");
            },
        }
        assert!(first.join(".index").is_file(), "original untouched");
    })
    .await;
}

/// A missing notebook is ensured on write paths when allowed.
#[tokio::test]
async fn missing_notebook_write_path_ensures_when_allowed() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        client
            .transaction(Some("brand-new"))
            .await
            .map(|_| ())
            .expect("ensure creates");
        assert!(env.nb_dir().join("brand-new/.index").is_file());
    })
    .await;
}

/// A missing notebook refuses on write paths when creation is off,
/// creating nothing.
#[tokio::test]
async fn missing_notebook_write_path_refuses_when_disabled() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for(&env)).expect("client");
        let err = client
            .transaction(Some("nope"))
            .await
            .map(|_| ())
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
    let env = NbTestEnv::new_native().expect("native fixture");
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

/// A plain directory is never git-initialized, on any write path.
#[tokio::test]
async fn plain_directory_is_never_initialized() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        std::fs::create_dir(env.nb_dir().join("plain")).expect("plain dir");
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        for result in [
            client.create_notebook(Some("plain")).await.map(|_| ()),
            client.transaction(Some("plain")).await.map(|_| ()),
        ] {
            let err = result.expect_err("plain directory must refuse");
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
    let env = NbTestEnv::new_native().expect("native fixture");
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
/// anything else: plain directories, symlinked roots, and
/// invalid names are left untouched.
#[tokio::test]
async fn enumeration_lists_valid_notebooks_sorted() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        client
            .create_notebook(Some("zzz-native"))
            .await
            .expect("create zzz");
        client
            .create_notebook(Some("aaa-native"))
            .await
            .expect("create aaa");
        std::fs::create_dir(env.nb_dir().join("plain")).expect("plain dir");
        #[cfg(unix)]
        std::fs::create_dir(env.nb_dir().join("bad:name"))
            .expect("invalid-name dir");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            env.nb_dir().join("aaa-native"),
            env.nb_dir().join("linked"),
        )
        .expect("symlink");

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

/// Concurrent explicit creates resolve to exactly one winner; the
/// losers are refused without touching the winner.
#[tokio::test]
async fn concurrent_creates_converge_on_single_notebook() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let results = std::thread::scope(|s| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    s.spawn(|| {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("runtime");
                        rt.block_on(async {
                            let client =
                                NbClient::new(&config_for_create(&env))
                                    .expect("client");
                            client.create_notebook(Some("race")).await
                        })
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("thread"))
                .collect::<Vec<_>>()
        });
        let mut winners = 0;
        for result in &results {
            match result {
                Ok(_) => winners += 1,
                Err(NbError::ValidationError { reason, .. }) => {
                    assert!(
                        reason.contains("already exists"),
                        "losers refused as existing: {reason:?}"
                    );
                },
                Err(other) => {
                    panic!("unexpected failure: {other:?}");
                },
            }
        }
        assert_eq!(winners, 1, "exactly one winner: {results:?}");
        let root = env.nb_dir().join("race");
        assert_eq!(
            git(&root, &["rev-list", "--count", "HEAD"]).trim(),
            "1",
            "exactly one initial commit"
        );
    })
    .await;
}

/// A delayed initializer still converges: observers that arrive
/// mid-init wait for the real `HEAD` instead of refusing the
/// notebook as invalid. The observer proves immediate usability
/// by committing right after ensure, not just constructing a
/// transaction.
#[tokio::test]
async fn slow_initializer_still_converges() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        unsafe {
            std::env::set_var("NB_API_PAUSE_BEFORE_INIT_COMMIT_MS", "1000");
        }
        let root = env.nb_dir().join("slow-race");
        let (winner, observer) = std::thread::scope(|s| {
            let creator = s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(async {
                    let client = NbClient::new(&config_for_create(&env))
                        .expect("client");
                    client.create_notebook(Some("slow-race")).await
                })
            });
            let mut poll = 0;
            while !root.join(".index").exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                poll += 1;
                assert!(poll < 500, "creator never reached its pause point");
            }
            let waiter = s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(async {
                    let client = NbClient::new(&config_for_create(&env))
                        .expect("client");
                    let mut tx = client.transaction(Some("slow-race")).await?;
                    tx.add_note(
                        "observer.md",
                        Some("Observer"),
                        "observer\n",
                        &[],
                    )
                    .unwrap();
                    tx.commit().await?;
                    Ok::<(), NbError>(())
                })
            });
            (
                creator.join().expect("creator thread"),
                waiter.join().expect("waiter thread"),
            )
        });
        winner.expect("creator succeeds");
        observer.expect("observer commits immediately after ensure");
        assert!(root.join("observer.md").is_file(), "observer note landed");
        assert_eq!(
            git(&root, &["rev-list", "--count", "HEAD"]).trim(),
            "2",
            "creator init plus observer commit"
        );
    })
    .await;
}

/// An injected init failure retains the partial directory for
/// inspection instead of recursively deleting it: ownership of
/// git-internal state is never assumed from names or shapes.
#[tokio::test]
async fn injected_failure_preserves_owned_directory() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        unsafe {
            std::env::set_var("NB_API_NOTEBOOK_INIT_FAIL", "1");
        }
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let err = client
            .create_notebook(Some("doomed"))
            .await
            .expect_err("injected failure must fail");
        assert!(
            matches!(err, NbError::Io { .. }),
            "expected injected Io failure, got: {err:?}"
        );
        assert!(
            env.nb_dir().join("doomed/.git/HEAD").is_file(),
            "partial directory retained for inspection"
        );
    })
    .await;
}

/// An injected init failure preserves foreign changes instead of
/// deleting them: a foreign file and a foreign commit survive.
#[tokio::test]
async fn injected_failure_preserves_foreign_changes() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        unsafe {
            std::env::set_var("NB_API_PAUSE_BEFORE_INIT_COMMIT_MS", "1000");
            std::env::set_var("NB_API_NOTEBOOK_INIT_FAIL", "1");
        }
        let root = env.nb_dir().join("contested");
        let outcome = std::thread::scope(|s| {
            let creator = s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(async {
                    let client = NbClient::new(&config_for_create(&env))
                        .expect("client");
                    client.create_notebook(Some("contested")).await
                })
            });
            let mut poll = 0;
            while !root.join(".index").exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                poll += 1;
                assert!(poll < 500, "creator never reached its pause point");
            }
            std::fs::write(root.join("foreign.txt"), "foreign\n")
                .expect("foreign file");
            assert_eq!(
                git(&root, &["add", "-A"]).trim(),
                "",
                "stage foreign content"
            );
            assert!(
                !git(&root, &["commit", "-m", "foreign", "--no-gpg-sign"])
                    .is_empty(),
                "foreign commit lands"
            );
            creator.join().expect("creator thread")
        });
        let err = outcome.expect_err("injected failure must fail");
        assert!(
            matches!(err, NbError::Io { .. }),
            "expected injected Io failure, got: {err:?}"
        );
        assert!(
            root.join("foreign.txt").is_file(),
            "foreign file survives cleanup"
        );
        assert!(
            git(&root, &["log", "--format=%s"]).contains("foreign"),
            "foreign commit survives cleanup"
        );
    })
    .await;
}

/// Initialization commits without invoking commit signing, even
/// when the isolated git config requires it.
#[tokio::test]
async fn init_commit_bypasses_required_signing() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        std::fs::write(
            env.home_dir().join(".gitconfig"),
            "[user]\n\tname = probe\n\temail = probe@example.com\n[commit]\n\tgpgsign = true\n[gpg]\n\tprogram = /nonexistent-gpg-program\n",
        )
        .expect("signing-required gitconfig");
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let root = client
            .create_notebook(Some("unsigned-init"))
            .await
            .expect("init must not invoke signing");
        assert_eq!(
            git(&root, &["rev-list", "--count", "HEAD"]).trim(),
            "1",
            "initial commit landed unsigned"
        );
    })
    .await;
}

/// Symlink notebook roots are refused without following them.
/// Unix-only: directory symlinks need privileges on Windows.
#[cfg(unix)]
#[tokio::test]
async fn symlink_root_is_refused() {
    let env = NbTestEnv::new_native().expect("native fixture");
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
        assert!(!target.join("alias").exists(), "target untouched");
    })
    .await;
}

/// Dead-`nb` tripwire: with a logging `nb` stub first on `PATH`
/// while git stays alive, the full native lifecycle still passes.
/// The counter log must never appear, proving zero `nb` attempts
/// (not merely zero successful spawns). Fixture setup itself runs
/// under the tripwire. Unix-only: the stub relies on executable-bit
/// script dispatch.
#[cfg(unix)]
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn native_lifecycle_spawns_no_nb() {
    use std::os::unix::fs::PermissionsExt;

    use crate::common::{EnvSnapshot, lock_env};
    let stub_holder = tempfile::Builder::new()
        .prefix("nb-dead-")
        .tempdir()
        .expect("stub dir");
    let log = stub_holder.path().join("calls.log");
    let shim = stub_holder.path().join("nb");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 127\n",
            log.display()
        ),
    )
    .expect("stub script");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
        .expect("chmod stub");

    let _guard = lock_env();
    let _snap = EnvSnapshot::capture(&[
        "NB_DIR",
        "HOME",
        "PATH",
        "NB_API_PAUSE_BEFORE_INIT_COMMIT_MS",
        "NB_API_PAUSE_AFTER_MKDIR_MS",
        "NB_API_NOTEBOOK_INIT_FAIL",
    ]);
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![stub_holder.path().to_path_buf()];
    paths.extend(std::env::split_paths(&path_var));
    unsafe {
        std::env::set_var(
            "PATH",
            std::env::join_paths(paths).expect("join PATH"),
        );
    }
    // Fixture setup itself runs under the tripwire.
    let env = NbTestEnv::new_native().expect("native fixture");
    assert!(
        !log.exists(),
        "fixture setup spawned nb: {:?}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    unsafe {
        std::env::set_var("NB_DIR", env.nb_dir());
        std::env::set_var("HOME", env.home_dir());
    }
    let assert_quiet = |stage: &str| {
        assert!(
            !log.exists(),
            "stage {stage} spawned nb: {:?}",
            std::fs::read_to_string(&log).unwrap_or_default()
        );
    };
    let client = NbClient::new(&config_for_create(&env)).expect("client");
    let root = client
        .create_notebook(Some("tripwire"))
        .await
        .expect("create with dead nb");
    assert_quiet("create");
    let mut tx = client
        .transaction(Some("tripwire"))
        .await
        .expect("tx with dead nb");
    assert_quiet("ensure");
    tx.add_note("hello.md", Some("Hello"), "hello\n", &[])
        .unwrap();
    tx.commit().await.expect("commit with dead nb");
    assert_quiet("commit");
    assert!(root.join("hello.md").is_file(), "note landed");
    let names = client.list_notebook_names().await.expect("enumerate");
    assert_quiet("enumerate");
    assert!(names.contains(&"tripwire".to_string()), "{names:?}");
    client
        .show_notebook_path(Some("tripwire"))
        .await
        .expect("path with dead nb");
    assert_quiet("path");
}

/// An `nb`-created notebook resolves natively (no `nb` spawned for
/// resolution itself; only fixture setup uses `nb` here).
/// Oracle set: requires the `nb`-provisioned fixture, so it runs
/// only in the extended profile.
#[ignore]
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
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let root = client
            .create_notebook(Some("interop-native"))
            .await
            .expect("create");
        let mut tx = client
            .transaction(Some("interop-native"))
            .await
            .expect("tx");
        tx.add_note("hello.md", Some("Hello"), "hello\n", &[])
            .unwrap();
        tx.commit().await.expect("seed note");
        assert!(root.join("hello.md").is_file(), "note landed");

        let oracle = NbOracle::new(&env);
        assert!(
            oracle.show_ok("interop-native:hello.md"),
            "true nb opens the natively created notebook"
        );
    })
    .await;
}
