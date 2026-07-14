//! Hermetic regression tests for `GIT_*` env scrub at every spawn
//! site in `nb-api`. Each test sets every `GIT_*` routing var to a
//! poison (non-existent, non-repo) path, then verifies the spawned
//! subprocess does NOT inherit them — only possible if
//! `nb_api::git_env::scrub_git_env` (or `scrub_git_env_std`) ran
//! before spawn.
//!
//! Per the test-infrastructure specification, every subprocess
//! spawn site in the crate requires a regression test. Currently
//! two: `NbClient::exec` (every public method funnels through this)
//! and `git_rev_parse` (the only direct `git` spawn). A future third
//! spawn site requires a new test here.
//!
//! No project repository config used by this reproduction; the
//! `NbTestEnv` fixture provides an isolated `NB_DIR`.

use std::process::Command as StdCommand;

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient};

use crate::common::{GIT_ROUTING_VARS, with_isolated_env, with_leaked_git_env};

/// Every `NbClient` public method funnels through `NbClient::exec`.
/// A successful `nb status` (which internally invokes `git status` on
/// the notebook's git repo) under a leaked `GIT_DIR=/poison` proves
/// the spawn site stripped the poison before exec.
#[tokio::test]
async fn nb_client_exec_does_not_inherit_leaked_git_dir() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_isolated_env(&env, true, || async {
        let client = NbClient::new(&Config {
            notebook: Some(env.notebook().to_string()),
            create_notebook: false,
            ..Config::default()
        })
        .expect("client construction");

        let result = client.status(None).await;
        assert!(
            result.is_ok(),
            "NbClient::status under leaked GIT_* failed: {:?}",
            result.err()
        );
    })
    .await;
}

/// Spawn a child via the fixture's `configure_std` and assert that
/// none of the `GIT_*` routing vars are present in the child's env.
///
/// This is the blast-by-prefix coverage: a regression to a
/// single-var allowlist (e.g. only scrubbing `GIT_DIR`) would let
/// `GIT_OBJECT_DIRECTORY` or one of the other routing vars leak
/// through and break this test under a different poison. The
/// earlier `..._leaked_git_dir` test covers the most-cited var
/// specifically; this one is the all-vars coverage.
///
/// The probe is the Unix `env` command which prints every inherited
/// variable to stdout in `NAME=VALUE` form.
#[test]
fn configure_std_scrubs_all_git_routing_vars() {
    let env = NbTestEnv::new().expect("fixture initialization");
    with_leaked_git_env(|| {
        let mut cmd = StdCommand::new("env");
        env.configure_std(&mut cmd);
        let output = cmd.output().expect("spawn env probe");
        let stdout = String::from_utf8_lossy(&output.stdout);
        for var in GIT_ROUTING_VARS {
            let prefix = format!("{var}=");
            assert!(
                !stdout.lines().any(|l| l.starts_with(&prefix)),
                "child env should not contain {var}, but it leaked; \
                 full child env:\n{stdout}"
            );
        }
    });
}

/// `git_rev_parse` is the only direct `git` subprocess spawn in the
/// crate. With leaked `GIT_DIR=/poison`, the spawned `git` would
/// try to resolve the repository under `/poison` and fail. A `Some`
/// return proves the scrub removed the poison before exec and the
/// command resolved the local repo from its own cwd.
#[test]
fn git_rev_parse_does_not_inherit_leaked_git_dir() {
    with_leaked_git_env(|| {
        let toplevel = nb_api::git_rev_parse(&["--show-toplevel"]);
        assert!(
            toplevel.is_some(),
            "git_rev_parse returned None under leaked GIT_DIR; scrub regressed"
        );
        let toplevel = toplevel.unwrap();
        assert!(
            toplevel.is_absolute(),
            "git_rev_parse returned non-absolute path: {}",
            toplevel.display()
        );
    });
}
