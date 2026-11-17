//! Native lifecycle hardening: malformed-ref validity, ancestor
//! isolation, ownership-preserving cleanup, fixture confinement,
//! and root validation. Hermetic like the core lifecycle tests:
//! native fixtures only, no `nb` spawns.
//!
//! Split from `native_lifecycle` to honor the file-size gate; both
//! modules share the same helpers by small duplication.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, NbError};

use crate::common::with_isolated_env;

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
fn current_ref(root: &std::path::Path) -> String {
    let head =
        std::fs::read_to_string(root.join(".git/HEAD")).expect("read HEAD");
    head.trim()
        .strip_prefix("ref: ")
        .expect("HEAD is symbolic after create")
        .trim()
        .to_string()
}

/// Bogus, empty, detached-short, and packed-invalid refs never
/// read as ready: queries and write-path ensures surface the
/// genuine git failure instead of `NotFound` or creation-disabled,
/// and enumeration never lists them.
#[tokio::test]
async fn malformed_refs_surface_git_failure() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let disabled =
            NbClient::new(&config_for(&env)).expect("disabled client");

        let bogus = client
            .create_notebook(Some("bogus-loose"))
            .await
            .expect("create bogus-loose");
        let r = current_ref(&bogus);
        std::fs::write(bogus.join(".git").join(&r), "bogus\n")
            .expect("corrupt loose ref");

        let empty = client
            .create_notebook(Some("empty-loose"))
            .await
            .expect("create empty-loose");
        let r = current_ref(&empty);
        std::fs::write(empty.join(".git").join(&r), "\n")
            .expect("empty loose ref");

        let short = client
            .create_notebook(Some("short-detached"))
            .await
            .expect("create short-detached");
        std::fs::write(short.join(".git/HEAD"), "abc123\n")
            .expect("short detached HEAD");

        let packed = client
            .create_notebook(Some("bogus-packed"))
            .await
            .expect("create bogus-packed");
        let r = current_ref(&packed);
        std::fs::remove_file(packed.join(".git").join(&r))
            .expect("remove loose ref");
        std::fs::write(
            packed.join(".git/packed-refs"),
            format!("bogus {r}\n"),
        )
        .expect("bogus packed-refs");

        for name in [
            "bogus-loose",
            "empty-loose",
            "short-detached",
            "bogus-packed",
        ] {
            let err = format!(
                "{:?}",
                client
                    .show_notebook_path(Some(name))
                    .await
                    .expect_err("malformed must not resolve")
            );
            assert!(
                !err.contains("NotFound"),
                "{name} collapsed to absence: {err:?}"
            );
            let err = format!(
                "{:?}",
                disabled
                    .transaction(Some(name))
                    .await
                    .map(|_| ())
                    .expect_err("malformed must surface git failure")
            );
            assert!(
                !err.contains("automatic creation is disabled"),
                "{name} masked as creation-disabled: {err:?}"
            );
        }
        let names = client.list_notebook_names().await.expect("enumerate");
        for name in [
            "bogus-loose",
            "empty-loose",
            "short-detached",
            "bogus-packed",
        ] {
            assert!(
                !names.contains(&name.to_string()),
                "malformed {name} must not enumerate: {names:?}"
            );
        }
    })
    .await;
}

/// Unreadable `HEAD` propagates `Io` instead of collapsing into
/// absence: queries fail typed and enumeration returns the error
/// rather than skipping the entry. Unix-only: permission bits.
#[cfg(unix)]
#[tokio::test]
async fn unreadable_head_propagates_io() {
    use std::os::unix::fs::PermissionsExt;
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let root = client
            .create_notebook(Some("unreadable"))
            .await
            .expect("create");
        std::fs::set_permissions(
            root.join(".git/HEAD"),
            std::fs::Permissions::from_mode(0o000),
        )
        .expect("chmod HEAD");
        if std::fs::read_to_string(root.join(".git/HEAD")).is_ok() {
            std::fs::set_permissions(
                root.join(".git/HEAD"),
                std::fs::Permissions::from_mode(0o644),
            )
            .expect("restore HEAD");
            return;
        }
        let err = format!(
            "{:?}",
            client
                .show_notebook_path(Some("unreadable"))
                .await
                .expect_err("unreadable HEAD must fail")
        );
        assert!(err.contains("Io"), "expected Io, got: {err:?}");
        let err = format!(
            "{:?}",
            client
                .list_notebook_names()
                .await
                .expect_err("enumeration must propagate Io")
        );
        assert!(err.contains("Io"), "expected Io, got: {err:?}");
        std::fs::set_permissions(
            root.join(".git/HEAD"),
            std::fs::Permissions::from_mode(0o644),
        )
        .expect("restore HEAD");
    })
    .await;
}

/// A plain child inside an ancestor git repository is never
/// accepted via ancestor `HEAD` discovery: it has no notebook of
/// its own, so queries miss and write paths refuse without
/// initializing.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn plain_child_inside_ancestor_git_is_refused() {
    use crate::common::{EnvSnapshot, lock_env};
    let env = NbTestEnv::new_native().expect("native fixture");
    let ancestor = tempfile::Builder::new()
        .prefix("nb-ancestor-")
        .tempdir()
        .expect("ancestor dir");
    let mut init = std::process::Command::new("git");
    nb_api::scrub_git_env_std(&mut init);
    init.current_dir(ancestor.path()).arg("init");
    assert!(init.output().expect("git init runs").status.success());
    let mut commit = std::process::Command::new("git");
    nb_api::scrub_git_env_std(&mut commit);
    commit.current_dir(ancestor.path()).args([
        "-c",
        "user.name=nb-api-test",
        "-c",
        "user.email=nb-api-test@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--allow-empty",
        "-q",
        "-m",
        "ancestor",
    ]);
    assert!(
        commit
            .output()
            .expect("ancestor commit runs")
            .status
            .success()
    );
    let store = ancestor.path().join("nbstore");
    std::fs::create_dir_all(&store).expect("nb store");
    std::fs::create_dir(store.join("plainchild")).expect("plain child");

    let _guard = lock_env();
    let _snap = EnvSnapshot::capture(&["NB_DIR", "HOME", "PATH"]);
    unsafe {
        std::env::set_var("NB_DIR", &store);
        std::env::set_var("HOME", env.home_dir());
    }
    let client = NbClient::new(&Config {
        notebook: None,
        create_notebook: true,
        allow_top_level_notes: true,
        disable_git_signing: true,
        ..Config::default()
    })
    .expect("client");
    let err = client
        .show_notebook_path(Some("plainchild"))
        .await
        .expect_err("plain child must miss");
    assert!(
        matches!(err, NbError::NotFound { .. }),
        "expected NotFound, got: {err:?}"
    );
    let err = client
        .transaction(Some("plainchild"))
        .await
        .map(|_| ())
        .expect_err("plain child must refuse");
    assert!(
        format!("{err:?}").contains("refusing to initialize"),
        "unexpected: {err:?}"
    );
    assert!(
        !store.join("plainchild/.git").exists(),
        "plain child left alone"
    );
}

/// A malformed existing notebook with creation disabled surfaces
/// the underlying git failure, never creation-disabled.
#[tokio::test]
async fn malformed_with_creation_disabled_surfaces_git_failure() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let creator = NbClient::new(&config_for_create(&env)).expect("client");
        let root = creator
            .create_notebook(Some("malformed-disabled"))
            .await
            .expect("create");
        let r = current_ref(&root);
        std::fs::write(root.join(".git").join(&r), "bogus\n")
            .expect("corrupt ref");
        let disabled =
            NbClient::new(&config_for(&env)).expect("disabled client");
        let err = format!(
            "{:?}",
            disabled
                .transaction(Some("malformed-disabled"))
                .await
                .map(|_| ())
                .expect_err("malformed must fail")
        );
        assert!(
            !err.contains("automatic creation is disabled"),
            "masked as creation-disabled: {err:?}"
        );
    })
    .await;
}

/// An injected init failure preserves a foreign-only `.index`
/// edit (no extra top-level file, no commit): the directory and
/// the foreign bytes survive cleanup.
#[tokio::test]
async fn injected_failure_preserves_index_only_edit() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        unsafe {
            std::env::set_var("NB_API_PAUSE_BEFORE_INIT_COMMIT_MS", "1000");
            std::env::set_var("NB_API_NOTEBOOK_INIT_FAIL", "1");
        }
        let root = env.nb_dir().join("index-only");
        let outcome = std::thread::scope(|s| {
            let creator = s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(async {
                    let client = NbClient::new(&config_for_create(&env))
                        .expect("client");
                    client.create_notebook(Some("index-only")).await
                })
            });
            let mut poll = 0;
            while !root.join(".index").exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                poll += 1;
                assert!(poll < 500, "creator never reached its pause point");
            }
            std::fs::write(root.join(".index"), "foreign\n")
                .expect("foreign index edit");
            creator.join().expect("creator thread")
        });
        let err = outcome.expect_err("injected failure must fail");
        assert!(
            matches!(err, NbError::Io { .. }),
            "expected injected Io failure, got: {err:?}"
        );
        assert!(root.join(".index").is_file(), "directory preserved");
        assert_eq!(
            std::fs::read_to_string(root.join(".index")).expect("read"),
            "foreign\n",
            "foreign index bytes survive"
        );
    })
    .await;
}

/// An injected init failure preserves a foreign detached `HEAD`:
/// the detached state survives instead of being deleted.
#[tokio::test]
async fn injected_failure_preserves_detached_head() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        unsafe {
            std::env::set_var("NB_API_PAUSE_BEFORE_INIT_COMMIT_MS", "1000");
            std::env::set_var("NB_API_NOTEBOOK_INIT_FAIL", "1");
        }
        let root = env.nb_dir().join("detached-foreign");
        let outcome = std::thread::scope(|s| {
            let creator = s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(async {
                    let client = NbClient::new(&config_for_create(&env))
                        .expect("client");
                    client.create_notebook(Some("detached-foreign")).await
                })
            });
            let mut poll = 0;
            while !root.join(".index").exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                poll += 1;
                assert!(poll < 500, "creator never reached its pause point");
            }
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
            git(&root, &["checkout", "--detach", "HEAD"]);
            creator.join().expect("creator thread")
        });
        outcome.expect_err("injected failure must fail");
        assert!(root.join(".git/HEAD").is_file(), "directory preserved");
        let head =
            std::fs::read_to_string(root.join(".git/HEAD")).expect("HEAD");
        assert!(
            !head.trim().starts_with("ref: "),
            "detached HEAD survives: {head:?}"
        );
    })
    .await;
}

/// An injected init failure preserves packed-refs foreign state:
/// a packed-only ref layout is not mistaken for an empty repo.
#[tokio::test]
async fn injected_failure_preserves_packed_refs() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        unsafe {
            std::env::set_var("NB_API_PAUSE_BEFORE_INIT_COMMIT_MS", "1000");
            std::env::set_var("NB_API_NOTEBOOK_INIT_FAIL", "1");
        }
        let root = env.nb_dir().join("packed-foreign");
        let outcome = std::thread::scope(|s| {
            let creator = s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(async {
                    let client = NbClient::new(&config_for_create(&env))
                        .expect("client");
                    client.create_notebook(Some("packed-foreign")).await
                })
            });
            let mut poll = 0;
            while !root.join(".index").exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                poll += 1;
                assert!(poll < 500, "creator never reached its pause point");
            }
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
            git(&root, &["pack-refs", "--all"]);
            creator.join().expect("creator thread")
        });
        outcome.expect_err("injected failure must fail");
        assert!(
            root.join(".git/packed-refs").is_file(),
            "packed-refs survives cleanup"
        );
    })
    .await;
}

/// An injected init failure preserves the directory when refs
/// cannot be read: unknown state fails closed. Unix-only.
#[cfg(unix)]
#[tokio::test]
async fn injected_failure_preserves_ref_read_failure() {
    use std::os::unix::fs::PermissionsExt;
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        unsafe {
            std::env::set_var("NB_API_PAUSE_BEFORE_INIT_COMMIT_MS", "1000");
            std::env::set_var("NB_API_NOTEBOOK_INIT_FAIL", "1");
        }
        let root = env.nb_dir().join("ref-unreadable");
        let outcome = std::thread::scope(|s| {
            let creator = s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(async {
                    let client = NbClient::new(&config_for_create(&env))
                        .expect("client");
                    client.create_notebook(Some("ref-unreadable")).await
                })
            });
            let mut poll = 0;
            while !root.join(".index").exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                poll += 1;
                assert!(poll < 500, "creator never reached its pause point");
            }
            std::fs::set_permissions(
                root.join(".git/refs"),
                std::fs::Permissions::from_mode(0o000),
            )
            .expect("chmod refs");
            if std::fs::read_dir(root.join(".git/refs")).is_ok() {
                std::fs::set_permissions(
                    root.join(".git/refs"),
                    std::fs::Permissions::from_mode(0o755),
                )
                .expect("restore refs");
                let _ = creator.join().expect("creator thread");
                return None;
            }
            let joined = creator.join().expect("creator thread");
            std::fs::set_permissions(
                root.join(".git/refs"),
                std::fs::Permissions::from_mode(0o755),
            )
            .expect("restore refs");
            Some(joined)
        });
        let Some(outcome) = outcome else {
            return;
        };
        outcome.expect_err("injected failure must fail");
        assert!(root.is_dir(), "directory preserved on unknown refs state");
    })
    .await;
}

/// The native fixture builder refuses escaping names before
/// touching the filesystem outside its store: absolute, parent
/// traversal, and dot-dot names fail and a caller sentinel stays
/// untouched.
#[tokio::test]
async fn native_fixture_builder_rejects_escaping_names() {
    let sentinel_holder = tempfile::Builder::new()
        .prefix("nb-sentinel-")
        .tempdir()
        .expect("sentinel dir");
    let sentinel = sentinel_holder.path().join("sentinel.txt");
    std::fs::write(&sentinel, "sentinel\n").expect("sentinel");
    for bad in ["../evil", "/abs-escape", ".."] {
        let result = NbTestEnv::builder().notebook(bad).build_native();
        let err = match result {
            Ok(_) => panic!("escaping name {bad:?} must fail"),
            Err(e) => format!("{e:?}"),
        };
        assert!(
            err.contains("invalid notebook name")
                || err.contains("Invalid `notebook`"),
            "unexpected error for {bad:?}: {err:?}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&sentinel).expect("sentinel"),
        "sentinel\n",
        "external sentinel untouched"
    );
}

/// Root validation: unset, non-directory, and I/O failures are
/// typed configuration or `Io` errors, never silent absence or
/// creation-disabled masking.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn root_validation_cases() {
    use crate::common::{EnvSnapshot, lock_env};
    let env = NbTestEnv::new_native().expect("native fixture");
    let _guard = lock_env();
    let _snap = EnvSnapshot::capture(&["NB_DIR", "HOME", "PATH"]);
    unsafe {
        std::env::set_var("HOME", env.home_dir());
    }
    let client = NbClient::new(&config_for_create(&env)).expect("client");

    unsafe {
        std::env::remove_var("NB_DIR");
    }
    let err = format!(
        "{:?}",
        client
            .show_notebook_path(Some("whatever"))
            .await
            .expect_err("unset root must fail")
    );
    assert!(
        err.contains("not configured"),
        "unset root is configuration failure: {err:?}"
    );

    let file = env.nb_dir().join("not-a-dir");
    std::fs::write(&file, "x\n").expect("root file");
    unsafe {
        std::env::set_var("NB_DIR", &file);
    }
    let err = format!(
        "{:?}",
        client
            .show_notebook_path(Some("whatever"))
            .await
            .expect_err("file root must fail")
    );
    assert!(
        err.contains("not a directory"),
        "file root is configuration failure: {err:?}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        unsafe {
            std::env::set_var("NB_DIR", env.nb_dir());
        }
        std::fs::set_permissions(
            env.nb_dir(),
            std::fs::Permissions::from_mode(0o000),
        )
        .expect("chmod nb dir");
        if std::fs::read_dir(env.nb_dir()).is_ok() {
            std::fs::set_permissions(
                env.nb_dir(),
                std::fs::Permissions::from_mode(0o755),
            )
            .expect("restore nb dir");
            return;
        }
        let err = format!(
            "{:?}",
            client
                .list_notebook_names()
                .await
                .expect_err("unreadable root must fail")
        );
        std::fs::set_permissions(
            env.nb_dir(),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("restore nb dir");
        assert!(err.contains("Io"), "unreadable root propagates Io: {err:?}");
    }
}

/// Full-width bogus OIDs and real non-commit objects never read
/// as ready: syntax-valid but unusable commits surface the git
/// failure instead of enumerating or transacting.
#[tokio::test]
async fn unusable_commit_objects_never_read_as_ready() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let disabled = NbClient::new(&config_for(&env)).expect("disabled");

        let bogus = client
            .create_notebook(Some("bogus-oid"))
            .await
            .expect("create bogus-oid");
        let r = current_ref(&bogus);
        std::fs::write(
            bogus.join(".git").join(&r),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .expect("full-width bogus OID");

        let blob_host = client
            .create_notebook(Some("blob-host"))
            .await
            .expect("create blob-host");
        let input = env.nb_dir().join("blob-input.txt");
        std::fs::write(&input, "blob\n").expect("blob input");
        let blob_oid =
            git(&blob_host, &["hash-object", "-w", &input.to_string_lossy()])
                .trim()
                .to_string();
        assert_eq!(blob_oid.len(), 40, "blob OID created");
        let blob = client
            .create_notebook(Some("blob-oid"))
            .await
            .expect("create blob-oid");
        let r = current_ref(&blob);
        std::fs::write(blob.join(".git").join(&r), format!("{blob_oid}\n"))
            .expect("blob OID as ref");

        for name in ["bogus-oid", "blob-oid"] {
            client
                .show_notebook_path(Some(name))
                .await
                .expect_err("unusable commit must not resolve");
            disabled
                .transaction(Some(name))
                .await
                .map(|_| ())
                .expect_err("unusable commit must fail ensure");
        }
        let names = client.list_notebook_names().await.expect("enumerate");
        assert!(
            !names.contains(&"bogus-oid".to_string()),
            "bogus OID must not enumerate: {names:?}"
        );
        assert!(
            !names.contains(&"blob-oid".to_string()),
            "blob OID must not enumerate: {names:?}"
        );
    })
    .await;
}

/// A valid branch name containing `+` initializes and is
/// recognized: git-grammar names are not rejected by an invented
/// whitelist, and enumeration includes them.
#[tokio::test]
async fn plus_branch_initializes_and_enumerates() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        std::fs::write(
            env.home_dir().join(".gitconfig"),
            "[user]\n\tname = nb-api tests\n\temail = nb-api@localhost\n[init]\n\tdefaultBranch = topic+work\n",
        )
        .expect("default branch config");
        let client = NbClient::new(&config_for_create(&env)).expect("client");
        let root = client
            .create_notebook(Some("plus-branch"))
            .await
            .expect("init on topic+work must succeed");
        let head = std::fs::read_to_string(root.join(".git/HEAD"))
            .expect("HEAD");
        assert!(
            head.contains("topic+work"),
            "branch preserved: {head:?}"
        );
        let names = client.list_notebook_names().await.expect("enumerate");
        assert!(
            names.contains(&"plus-branch".to_string()),
            "plus-branch enumerates: {names:?}"
        );
        client
            .show_notebook_path(Some("plus-branch"))
            .await
            .expect("plus-branch resolves");
    })
    .await;
}

/// An ensure arriving in the mkdir-to-init window converges: the
/// winner exposes an empty directory first, the observer waits
/// through git init, then commits immediately on the real HEAD.
/// Pre-existing plain directories are still refused elsewhere.
#[tokio::test]
async fn ensure_converges_through_mkdir_window() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        unsafe {
            std::env::set_var("NB_API_PAUSE_AFTER_MKDIR_MS", "1000");
        }
        let root = env.nb_dir().join("mkdir-race");
        let (winner, observer) = std::thread::scope(|s| {
            let creator = s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(async {
                    let client = NbClient::new(&config_for_create(&env))
                        .expect("client");
                    client.create_notebook(Some("mkdir-race")).await
                })
            });
            let mut poll = 0;
            while !root.exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                poll += 1;
                assert!(poll < 500, "creator never exposed mkdir");
            }
            let waiter = s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(async {
                    let client = NbClient::new(&config_for_create(&env))
                        .expect("client");
                    let mut tx =
                        client.transaction(Some("mkdir-race")).await?;
                    tx.add_note("early.md", Some("Early"), "early\n", &[])
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
        observer.expect("ensure converges through mkdir window");
        assert!(root.join("early.md").is_file(), "observer note landed");
    })
    .await;
}

/// An injected init failure preserves a foreign-only
/// `.git/config` change (remote/marker): staged status and empty
/// index do not prove git metadata belongs to this call.
#[tokio::test]
async fn injected_failure_preserves_config_only_change() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        unsafe {
            std::env::set_var("NB_API_PAUSE_BEFORE_INIT_COMMIT_MS", "1000");
            std::env::set_var("NB_API_NOTEBOOK_INIT_FAIL", "1");
        }
        let root = env.nb_dir().join("config-foreign");
        let outcome = std::thread::scope(|s| {
            let creator = s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(async {
                    let client = NbClient::new(&config_for_create(&env))
                        .expect("client");
                    client.create_notebook(Some("config-foreign")).await
                })
            });
            let mut poll = 0;
            while !root.join(".index").exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                poll += 1;
                assert!(poll < 500, "creator never reached its pause point");
            }
            let config = root.join(".git/config");
            let mut contents =
                std::fs::read_to_string(&config).expect("read config");
            contents.push_str(
                "[remote \"origin\"]\n\turl = https://example.invalid/foreign.git\n[foreign]\n\tmarker = true\n",
            );
            std::fs::write(&config, &contents).expect("foreign config");
            creator.join().expect("creator thread")
        });
        let err = outcome.expect_err("injected failure must fail");
        assert!(
            matches!(err, NbError::Io { .. }),
            "expected injected Io failure, got: {err:?}"
        );
        let contents =
            std::fs::read_to_string(root.join(".git/config")).expect("config");
        assert!(
            contents.contains("foreign.git") && contents.contains("marker"),
            "foreign config survives cleanup: {contents:?}"
        );
    })
    .await;
}
