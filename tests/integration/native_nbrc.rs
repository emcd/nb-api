//! Native NBRC store resolution end-to-end: the rc selects the
//! store that later operations use, per caller-selected mode.
//!
//! Hermetic: fixtures are native (`NbTestEnv::new_native`), the rc
//! lives in the fixture-owned `HOME`, and `with_isolated_env`
//! serializes the process-environment surgery.

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient, ResolutionMode};

use crate::common::with_isolated_env;

fn config_for(env: &NbTestEnv, mode: ResolutionMode, strict: bool) -> Config {
    Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: true,
        allow_top_level_notes: true,
        disable_git_signing: true,
        nbrc_mode: mode,
        require_explicit_nb_dir: strict,
        ..Config::default()
    }
}

fn write_nbrc(home: &std::path::Path, body: &str) {
    std::fs::write(home.join(".nbrc"), body).expect("rc fixture");
}

/// The rc-selected store is the one creation lands in (ParseNbrc).
#[tokio::test]
async fn rc_selected_store_receives_create() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        // SAFETY: serialized by with_isolated_env; restored on exit.
        unsafe { std::env::remove_var("NB_DIR") };
        let picked = env.home_dir().join("picked");
        std::fs::create_dir_all(&picked).expect("picked store");
        write_nbrc(env.home_dir(), "export NB_DIR=\"$HOME/picked\"\n");
        let client =
            NbClient::new(&config_for(&env, ResolutionMode::ParseNbrc, false))
                .expect("client");
        let root = client
            .create_notebook(Some("rc-notebook"))
            .await
            .expect("create resolves through rc");
        assert_eq!(
            root,
            picked.join("rc-notebook"),
            "creation lands in the rc-selected store",
        );
    })
    .await;
}

/// A preset `NB_DIR` survives a self-default rc (always-parse).
#[tokio::test]
async fn preset_survives_self_default_rc() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        let picked = env.home_dir().join("env-picked");
        std::fs::create_dir_all(&picked).expect("picked store");
        // SAFETY: serialized by with_isolated_env; restored on exit.
        unsafe {
            std::env::set_var("NB_DIR", &picked);
        }
        write_nbrc(
            env.home_dir(),
            "export NB_DIR=\"${NB_DIR:-/stores/ignored}\"\n",
        );
        let client =
            NbClient::new(&config_for(&env, ResolutionMode::ParseNbrc, false))
                .expect("client");
        let root = client
            .create_notebook(Some("env-notebook"))
            .await
            .expect("create resolves through preset");
        assert_eq!(
            root,
            picked.join("env-notebook"),
            "preset wins over the rc self-default",
        );
    })
    .await;
}

/// Strict mode refuses an unset `NB_DIR` without consulting the rc.
#[tokio::test]
async fn strict_refuses_unset_without_reading_rc() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        // SAFETY: serialized by with_isolated_env; restored on exit.
        unsafe { std::env::remove_var("NB_DIR") };
        write_nbrc(env.home_dir(), "NB_DIR=/stores/never-read\n");
        let client =
            NbClient::new(&config_for(&env, ResolutionMode::ParseNbrc, true))
                .expect("client");
        let err = client
            .create_notebook(Some("nope"))
            .await
            .expect_err("strict unset must refuse");
        assert!(
            format!("{err:?}").contains("not configured"),
            "unexpected error: {err:?}",
        );
    })
    .await;
}

/// `IgnoreNbrc` never reads the rc, even when one exists.
#[tokio::test]
async fn ignore_mode_skips_present_rc() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        // SAFETY: serialized by with_isolated_env; restored on exit.
        unsafe { std::env::remove_var("NB_DIR") };
        write_nbrc(env.home_dir(), "NB_DIR=/stores/never-read\n");
        std::fs::create_dir_all(env.home_dir().join(".nb"))
            .expect("default store exists");
        let client = NbClient::new(&config_for(
            &env,
            ResolutionMode::IgnoreNbrc,
            false,
        ))
        .expect("client");
        let root = client
            .create_notebook(Some("default-notebook"))
            .await
            .expect("ignore falls back to ~/.nb");
        assert_eq!(
            root,
            env.home_dir().join(".nb").join("default-notebook"),
            "ignore resolves the default store",
        );
    })
    .await;
}

/// `EvaluateNbrc` sources the rc for real, including constructs
/// the static subset rejects.
#[tokio::test]
async fn evaluate_mode_sources_rc() {
    let env = NbTestEnv::new_native().expect("native fixture");
    with_isolated_env(&env, false, || async {
        // SAFETY: serialized by with_isolated_env; restored on exit.
        unsafe { std::env::remove_var("NB_DIR") };
        let picked = env.home_dir().join("eval-picked");
        std::fs::create_dir_all(&picked).expect("picked store");
        write_nbrc(env.home_dir(), "NB_DIR=$(echo \"$HOME/eval-picked\")\n");
        let client = NbClient::new(&config_for(
            &env,
            ResolutionMode::EvaluateNbrc,
            false,
        ))
        .expect("client");
        let root = client
            .create_notebook(Some("eval-notebook"))
            .await
            .expect("create resolves through evaluation");
        assert_eq!(
            root,
            picked.join("eval-notebook"),
            "evaluation result selects the store",
        );
    })
    .await;
}
