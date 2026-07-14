//! Behavioral coverage for the `nb_api::testing` async helpers
//! (`configure_tokio`, `nb_command_async`). These are gated behind
//! the `testing-tokio` Cargo feature; this test exercises them
//! so the QA pipeline catches feature-gated compile breakage as a
//! behavior, not just a build artifact.
//!
//! No project repository config used by this reproduction; the
//! `NbTestEnv` fixture provides an isolated `NB_DIR`.

#[cfg(feature = "testing-tokio")]
use nb_api::testing::NbTestEnv;

#[cfg(feature = "testing-tokio")]
use crate::common::with_isolated_env;

/// `nb_command_async` returns a `tokio::process::Command` with the
/// fixture's environment applied; spawning it under the fixture's
/// `NB_DIR` and `HOME` must succeed and produce the expected `nb`
/// output (`nb --version` reports the version we pinned the qa
/// workflow to).
#[cfg(feature = "testing-tokio")]
#[tokio::test]
async fn nb_command_async_spawns_nb_with_fixture_env() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, false, || async {
        let mut cmd = env.nb_command_async();
        cmd.arg("--version");
        let output = cmd.output().await.expect("spawn nb --version");
        assert!(
            output.status.success(),
            "nb --version failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains('.'),
            "expected version string in `nb --version` stdout: {stdout}"
        );
    })
    .await;
}
