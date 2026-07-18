//! Regression test for the `nb-api 0.2.0` downstream bug:
//! `apply_git_signing_env` read `GIT_CONFIG_COUNT` from the
//! parent process env via `std::env::var`, but `scrub_git_env`
//! has already removed all `GIT_*` from the spawn-time `Command`
//! env. Result: when the parent had `GIT_CONFIG_COUNT=2`, the
//! child `nb` received `GIT_CONFIG_COUNT=4` with only `KEY_2`/
//! `KEY_3` emitted (gap at indices 0,1 — those keys were
//! scrubbed). Git config parsing failed downstream.
//!
//! The patch (commit landing on master for `nb-api 0.2.1`)
//! hard-codes `start_index = 0` in `apply_git_signing_env` so
//! the emitted overrides occupy indices 0,1 contiguously. This
//! test proves the contract: the spawned `nb` child receives
//! `GIT_CONFIG_COUNT=2`, `KEY_0=commit.gpgsign`,
//! `VALUE_0=false`, `KEY_1=tag.gpgsign`, `VALUE_1=false`, with
//! no entries at indices 2 or 3 (no gap). The parent's
//! `user.signingkey=DEADBEEF` and `commit.gpgsign=true` do
//! NOT leak through.
//!
//! Uses the existing infrastructure: `ENV_LOCK` (serialization
//! across concurrent tests), `EnvSnapshot` (RAII env restore on
//! panic), and a custom `nb` shim that captures its own env to
//! a file. The test does not expose any internals; it observes
//! the spawned child through the same `NbClient` API the
//! downstream consumers use.
//!
//! Both tests in this module hold `ENV_LOCK` across the
//! `client.list_notes(...).await` call to serialize env mutation
//! against concurrent tests in the same binary. The lock is
//! released when `_snap` drops (at end of function), and the
//! mutex is non-reentrant per `tests/integration/common/mod.rs`.
//! Justified per the same pattern as `with_isolated_env`: all
//! integration tests use `#[tokio::test]` with the default
//! current-thread flavor, the closures do not spawn tasks, and
//! the alternative (drop-and-reacquire with shared state) is
//! racy.

#![cfg(unix)]
#![allow(clippy::await_holding_lock)]

use std::fs;
use std::os::unix::fs::PermissionsExt;

use nb_api::testing::NbTestEnv;
use nb_api::{Config, NbClient};

use crate::common::{EnvSnapshot, lock_env};

/// Env vars this test mutates and restores via `EnvSnapshot`.
/// Mirrors the variables that `NbTestEnv::configure_std` and
/// `with_shim_nb_env` care about: NB_DIR, HOME, GIT_* routing
/// vars (blast-by-prefix scrub), PATH, SHIM_OUTPUT.
const TEST_ENV_VARS: &[&str] = &[
    "NB_DIR",
    "HOME",
    "PATH",
    "SHIM_OUTPUT",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_KEY_0",
    "GIT_CONFIG_VALUE_0",
    "GIT_CONFIG_KEY_1",
    "GIT_CONFIG_VALUE_1",
];

#[tokio::test]
async fn spawned_nb_child_receives_contiguous_signing_overrides_at_indices_zero_and_one() {
    let env = NbTestEnv::new().expect("fixture initialization");
    let _guard = lock_env();
    let _snap = EnvSnapshot::capture(TEST_ENV_VARS);

    // Configure parent env with `GIT_CONFIG_COUNT=2` plus a
    // non-default `KEY_0=user.signingkey`, `VALUE_0=DEADBEEF`,
    // and a parent-side `commit.gpgsign=true` at `KEY_1`. The
    // pre-patch bug used the parent's `GIT_CONFIG_COUNT=2` as
    // `start_index` and emitted overrides at indices 2/3,
    // leaving a gap at 0/1. The fix hard-codes `start_index=0`.

    let parent_overrides: &[(&str, &str)] = &[
        ("GIT_CONFIG_COUNT", "2"),
        ("GIT_CONFIG_KEY_0", "user.signingkey"),
        ("GIT_CONFIG_VALUE_0", "DEADBEEF"),
        ("GIT_CONFIG_KEY_1", "commit.gpgsign"),
        ("GIT_CONFIG_VALUE_1", "true"),
    ];
    for (k, v) in parent_overrides {
        // SAFETY: serialized by ENV_LOCK; restored on `_snap`
        // drop (RAII env restore).
        unsafe { std::env::set_var(k, v) };
    }

    // Write a custom `nb` shim that captures its own env to a
    // file before emitting `SHIM_OUTPUT`. The shim does NOT
    // pass through to real `nb` because we want to capture the
    // exact env our `NbClient::exec` constructed, regardless
    // of whether real `nb` would have errored on the captured
    // config.
    let capture_dir = tempfile::Builder::new()
        .prefix("nb-env-cap-")
        .tempdir()
        .expect("create capture tempdir");
    let capture_path = capture_dir.path().join("captured_env");
    let shim_dir = tempfile::Builder::new()
        .prefix("nb-shim-")
        .tempdir()
        .expect("create shim tempdir");
    let shim_path = shim_dir.path().join("nb");

    let capture_path_str = capture_path.display().to_string();
    let shim_script = format!(
        r#"#!/usr/bin/env bash
# Capture own env to the file before any other action.
env | LC_ALL=C sort > {capture_path_str}
# Emit SHIM_OUTPUT if set, else no-op (the test asserts on
# the captured env, not on stdout content).
if [[ -n "${{SHIM_OUTPUT:-}}" ]]; then
  printf '%s' "$SHIM_OUTPUT"
fi
exit 0
"#
    );
    fs::write(&shim_path, shim_script).expect("write shim script");
    fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755)).expect("chmod shim script");

    // Prepend the shim dir to PATH so the shim resolves first.
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = format!(
        "{}:{}",
        shim_dir.path().display(),
        current_path.to_string_lossy()
    );
    // SAFETY: see above.
    unsafe { std::env::set_var("PATH", &new_path) };
    unsafe { std::env::set_var("NB_DIR", env.nb_dir()) };
    unsafe { std::env::set_var("HOME", env.home_dir()) };
    // SHIM_OUTPUT triggers the existing empty-result hint
    // sanitization (`0 items.\n`) without depending on real
    // `nb` output. The hint block detection finds a blank
    // separator and a hint marker below — but `SHIM_OUTPUT`
    // here is just `0 items.` with no hint block, so the
    // helper short-circuits with input unchanged and we get
    // exactly the signal back. We just need any well-formed
    // empty-list output; the captured env is what matters.
    unsafe { std::env::set_var("SHIM_OUTPUT", "0 items.\n") };
    // Also set author/committer env (consistent with
    // `NbTestEnv::configure_std`) so the test exercises the
    // same blast-by-prefix scrub path.
    unsafe { std::env::set_var("GIT_AUTHOR_NAME", "test") };
    unsafe { std::env::set_var("GIT_AUTHOR_EMAIL", "test@test") };
    unsafe { std::env::set_var("GIT_COMMITTER_NAME", "test") };
    unsafe { std::env::set_var("GIT_COMMITTER_EMAIL", "test@test") };

    let config = Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: false,
        allow_top_level_notes: true,
        disable_git_signing: true,
    };
    let client = NbClient::new(&config).expect("client construction");

    // Trigger an `nb` invocation. `list_notes` invokes
    // `nb list` (and the hint-block sanitization runs over
    // the captured stdout, which is just `0 items.\n`).
    let _ = client.list_notes(None, &[], None, None).await;

    // Read the captured env (the spawn-time env as seen by
    // the `nb` child after `scrub_git_env` + `apply_git_signing_env`).
    let captured = fs::read_to_string(&capture_path)
        .unwrap_or_else(|e| panic!("read captured env {}: {e}", capture_path.display()));

    // `apply_git_signing_env` emits: GIT_CONFIG_COUNT=2,
    // KEY_0=commit.gpgsign, VALUE_0=false, KEY_1=tag.gpgsign,
    // VALUE_1=false. (Pre-patch with parent count=2 emitted
    // count=4 with overrides at indices 2,3 — a gap at 0,1.)
    assert_eq!(
        captured
            .lines()
            .find(|l| l.starts_with("GIT_CONFIG_COUNT=")),
        Some("GIT_CONFIG_COUNT=2"),
        "GIT_CONFIG_COUNT must equal override count (2), not parent count + overrides; \
         captured:\n{captured}"
    );
    assert_eq!(
        captured
            .lines()
            .find(|l| l.starts_with("GIT_CONFIG_KEY_0=")),
        Some("GIT_CONFIG_KEY_0=commit.gpgsign"),
        "GIT_CONFIG_KEY_0 must be commit.gpgsign (signing override at index 0); \
         captured:\n{captured}"
    );
    assert_eq!(
        captured
            .lines()
            .find(|l| l.starts_with("GIT_CONFIG_VALUE_0=")),
        Some("GIT_CONFIG_VALUE_0=false"),
        "GIT_CONFIG_VALUE_0 must be 'false' (signing override); captured:\n{captured}"
    );
    assert_eq!(
        captured
            .lines()
            .find(|l| l.starts_with("GIT_CONFIG_KEY_1=")),
        Some("GIT_CONFIG_KEY_1=tag.gpgsign"),
        "GIT_CONFIG_KEY_1 must be tag.gpgsign (signing override at index 1); \
         captured:\n{captured}"
    );
    assert_eq!(
        captured
            .lines()
            .find(|l| l.starts_with("GIT_CONFIG_VALUE_1=")),
        Some("GIT_CONFIG_VALUE_1=false"),
        "GIT_CONFIG_VALUE_1 must be 'false' (signing override); captured:\n{captured}"
    );

    // Pre-patch bug: no entries at indices 2 or 3 (gap from
    // start_index=2). After patch: still no entries at 2 or 3
    // (we only emit 0 and 1).
    assert!(
        !captured.lines().any(|l| l.starts_with("GIT_CONFIG_KEY_2=")),
        "no KEY_2 entry expected (override count is 2, not 4); captured:\n{captured}"
    );
    assert!(
        !captured.lines().any(|l| l.starts_with("GIT_CONFIG_KEY_3=")),
        "no KEY_3 entry expected (override count is 2, not 4); captured:\n{captured}"
    );

    // Parent's GIT_CONFIG must NOT leak through. (Blast-by-
    // prefix scrub removes every GIT_* from the spawn-time env
    // before the signing overrides are re-added.)
    assert!(
        !captured.lines().any(|l| l.contains("DEADBEEF")),
        "parent's GIT_CONFIG_VALUE_0=DEADBEEF must not leak; captured:\n{captured}"
    );
    assert!(
        !captured
            .lines()
            .any(|l| l.starts_with("GIT_CONFIG_KEY_0=user.signingkey")),
        "parent's GIT_CONFIG_KEY_0=user.signingkey must not leak; captured:\n{captured}"
    );
    assert!(
        !captured.lines().any(|l| l == "GIT_CONFIG_VALUE_1=true"),
        "parent's GIT_CONFIG_VALUE_1=true must not leak (signing override is false); \
         captured:\n{captured}"
    );

    // Parent's GIT_AUTHOR_* and GIT_COMMITTER_* are also
    // scrubbed (blast-by-prefix `GIT_`). Verify the spawn-time
    // env has no GIT_AUTHOR/COMMITTER entries.
    assert!(
        !captured.lines().any(|l| l.starts_with("GIT_AUTHOR_")),
        "GIT_AUTHOR_* must not leak; captured:\n{captured}"
    );
    assert!(
        !captured.lines().any(|l| l.starts_with("GIT_COMMITTER_")),
        "GIT_COMMITTER_* must not leak; captured:\n{captured}"
    );
}

/// Companion baseline: with no parent `GIT_CONFIG_*` set (the
/// upstream `with_isolated_env` style), the spawned child still
/// receives the contiguous overrides. This test guards against a
/// regression where the patch accidentally reads *no* parent
/// count and emits 0 entries.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn spawned_nb_child_receives_signing_overrides_when_parent_has_no_git_config() {
    let env = NbTestEnv::new().expect("fixture initialization");
    let _guard = lock_env();
    let _snap = EnvSnapshot::capture(TEST_ENV_VARS);

    // Explicitly unset any parent GIT_CONFIG_* (the EnvSnapshot
    // captures the pre-test state and restores on drop).
    for name in [
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_KEY_0",
        "GIT_CONFIG_VALUE_0",
        "GIT_CONFIG_KEY_1",
        "GIT_CONFIG_VALUE_1",
    ] {
        // SAFETY: see above.
        unsafe { std::env::remove_var(name) };
    }

    // Custom shim (same as the primary test).
    let capture_dir = tempfile::Builder::new()
        .prefix("nb-env-cap-")
        .tempdir()
        .expect("create capture tempdir");
    let capture_path = capture_dir.path().join("captured_env");
    let shim_dir = tempfile::Builder::new()
        .prefix("nb-shim-")
        .tempdir()
        .expect("create shim tempdir");
    let shim_path = shim_dir.path().join("nb");

    let capture_path_str = capture_path.display().to_string();
    let shim_script = format!(
        r#"#!/usr/bin/env bash
env | LC_ALL=C sort > {capture_path_str}
if [[ -n "${{SHIM_OUTPUT:-}}" ]]; then
  printf '%s' "$SHIM_OUTPUT"
fi
exit 0
"#
    );
    fs::write(&shim_path, shim_script).expect("write shim script");
    fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755)).expect("chmod shim script");

    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = format!(
        "{}:{}",
        shim_dir.path().display(),
        current_path.to_string_lossy()
    );
    unsafe { std::env::set_var("PATH", &new_path) };
    unsafe { std::env::set_var("NB_DIR", env.nb_dir()) };
    unsafe { std::env::set_var("HOME", env.home_dir()) };
    unsafe { std::env::set_var("SHIM_OUTPUT", "0 items.\n") };

    let config = Config {
        notebook: Some(env.notebook().to_string()),
        create_notebook: false,
        allow_top_level_notes: true,
        disable_git_signing: true,
    };
    let client = NbClient::new(&config).expect("client construction");
    let _ = client.list_notes(None, &[], None, None).await;

    let captured = fs::read_to_string(&capture_path)
        .unwrap_or_else(|e| panic!("read captured env {}: {e}", capture_path.display()));

    // Same invariants as the primary test, but with no parent
    // GIT_CONFIG set — proves the patch doesn't accidentally
    // emit zero entries when start_index would have been 0.
    assert_eq!(
        captured
            .lines()
            .find(|l| l.starts_with("GIT_CONFIG_COUNT=")),
        Some("GIT_CONFIG_COUNT=2"),
        "GIT_CONFIG_COUNT must equal override count (2); captured:\n{captured}"
    );
    assert_eq!(
        captured
            .lines()
            .find(|l| l.starts_with("GIT_CONFIG_KEY_0=")),
        Some("GIT_CONFIG_KEY_0=commit.gpgsign"),
        "GIT_CONFIG_KEY_0 must be commit.gpgsign; captured:\n{captured}"
    );
    assert_eq!(
        captured
            .lines()
            .find(|l| l.starts_with("GIT_CONFIG_KEY_1=")),
        Some("GIT_CONFIG_KEY_1=tag.gpgsign"),
        "GIT_CONFIG_KEY_1 must be tag.gpgsign; captured:\n{captured}"
    );
}
