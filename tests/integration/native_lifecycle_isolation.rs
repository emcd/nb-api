//! Native lifecycle ancestor isolation: notebooks under a
//! containing git repository never resolve the ancestor's `HEAD`.
//! Hermetic native fixtures only; no `nb` spawns.
//!
//! Split from the hardening module to honor the file-size gate.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, NbError};
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

/// A malformed notebook inside a committed ancestor repository
/// never resolves the ancestor's `HEAD`: local Git identity is
/// bound explicitly, so parent discovery cannot promote the child.
/// Covers query, ensure with and without creation, and enumeration.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn malformed_child_inside_committed_ancestor_is_isolated() {
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
    let ancestor_head = {
        let mut rev = std::process::Command::new("git");
        nb_api::scrub_git_env_std(&mut rev);
        rev.current_dir(ancestor.path()).args(["rev-parse", "HEAD"]);
        let out = rev.output().expect("ancestor rev-parse runs");
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert!(!ancestor_head.is_empty(), "ancestor HEAD committed");

    let _guard = lock_env();
    let _snap = EnvSnapshot::capture(&["NB_DIR", "HOME", "PATH"]);
    let store = ancestor.path().join("nbstore");
    std::fs::create_dir_all(&store).expect("nb store");
    unsafe {
        std::env::set_var("NB_DIR", &store);
        std::env::set_var("HOME", env.home_dir());
    }
    let client = NbClient::new(&Config {
        notebook: None,
        create_notebook: true,
        allow_top_level_notes: true,
        disable_git_signing: true,
        ..Default::default()
    })
    .expect("client");
    let disabled = NbClient::new(&Config {
        notebook: None,
        create_notebook: false,
        allow_top_level_notes: true,
        disable_git_signing: true,
        ..Default::default()
    })
    .expect("disabled client");
    let root = client
        .create_notebook(Some("victim"))
        .await
        .expect("create victim");
    std::fs::write(root.join(".git/HEAD"), "abc123\n")
        .expect("malformed HEAD");

    let err = format!(
        "{:?}",
        client
            .show_notebook_path(Some("victim"))
            .await
            .expect_err("malformed child must not resolve")
    );
    assert!(
        !err.contains(&ancestor_head),
        "ancestor HEAD leaked into child query: {err:?}"
    );
    for (label, c) in [("enabled", &client), ("disabled", &disabled)] {
        let err = format!(
            "{:?}",
            c.transaction(Some("victim"))
                .await
                .map(|_| ())
                .expect_err("malformed child must fail ensure")
        );
        assert!(
            !err.contains(&ancestor_head),
            "ancestor HEAD leaked into {label} ensure: {err:?}"
        );
        assert!(
            !err.contains("automatic creation is disabled"),
            "malformed masked as creation-disabled ({label}): {err:?}"
        );
    }
    let names = client.list_notebook_names().await.expect("enumerate");
    assert!(
        !names.contains(&"victim".to_string()),
        "malformed child must not enumerate: {names:?}"
    );
}
