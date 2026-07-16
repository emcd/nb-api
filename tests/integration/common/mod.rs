//! Shared test helpers for the integration test suite.
//!
//! All env-mutating helpers share a single `std::sync::Mutex`
//! (`ENV_LOCK`) that serializes process-env mutations across
//! concurrent tests in this binary. The mutex is **not reentrant**,
//! so helpers must never be nested; combined helpers ([`with_isolated_env`])
//! acquire the lock once and apply every override inside the same
//! critical section.

use std::ffi::OsString;
use std::future::Future;
use std::sync::{Mutex, MutexGuard};

use nb_api::testing::NbTestEnv;

/// Bash script template for the fake `nb` shim used by
/// [`with_shim_nb_env`]. The `__REAL_NB__` placeholder is
/// replaced with the absolute path to the real `nb` binary
/// (resolved against the unmodified PATH) before writing the
/// script to disk. Using a template constant keeps the script
/// readable and avoids double-escaping bash variables inside
/// a Rust format string. Unix-only because [`with_shim_nb_env`]
/// is Unix-only (Bash script + executable-bit chmod).
#[cfg(unix)]
const SHIM_SCRIPT_TEMPLATE: &str = r#"#!/usr/bin/env bash
# Pass through to real nb UNLESS both:
# 1. SHIM_OUTPUT is set (the shim test is active), AND
# 2. The invocation is list-like (any arg matches "list" or
#    ends with ":list", e.g. "scratch:list").
# Without the subcommand check, the shim would echo
# SHIM_OUTPUT for any concurrent `nb` invocation
# (e.g., a sibling test's `nb notebooks add` during
# fixture init), corrupting the sibling's notebook
# creation. The subcommand check routes non-list
# invocations to real nb regardless of SHIM_OUTPUT.
# REAL_NB is the absolute path to the real `nb` binary,
# resolved against the unmodified PATH by the Rust
# helper before prepending the shim dir. Defaults to
# /usr/local/bin/nb if unset (defensive fallback).
shim_output="${SHIM_OUTPUT:-}"
list_invocation=false
for arg in "$@"; do
  if [[ "$arg" == "list" || "$arg" == *:list* ]]; then
    list_invocation=true
    break
  fi
done
if [[ -n "$shim_output" && "$list_invocation" == "true" ]]; then
  printf '%s' "$shim_output"
  exit 0
fi
exec "${REAL_NB:-__REAL_NB__}" "$@"
"#;

/// All env vars the env-mutating helpers may read, write, or
/// restore. `PATH` and `SHIM_OUTPUT` are included so the
/// RAII `EnvSnapshot` handles them automatically (panic-safe
/// restoration on drop). Adding `PATH` is also a hardening
/// for `with_isolated_env`: a panic inside the closure no
/// longer leaks a mutated PATH to subsequent tests.
const ENV_VARS_OF_INTEREST: &[&str] = &[
    "NB_DIR",
    "HOME",
    "GIT_DIR",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_WORK_TREE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "PATH",
    "SHIM_OUTPUT",
];

/// All `GIT_*` routing vars used in the blast-by-prefix scrub. Exposed
/// so tests can assert per-var child-env presence / absence.
pub const GIT_ROUTING_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_WORK_TREE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// Serializes environment mutations across integration tests so that
/// concurrent tests cannot observe each other's poison vars or
/// restore each other's `NB_DIR` / `HOME` overrides.
pub static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Acquire [`ENV_LOCK`]. Tolerates a poisoned mutex by recovering
/// the inner guard; tests intentionally mutate process env and we
/// do not want a panic in one test to permanently lock out the rest.
pub fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Configure the parent process env for a test invocation that
/// exercises `NbClient` against an isolated `NbTestEnv`. Sets
/// `NB_DIR` and `HOME` to the fixture's paths, and (when
/// `poison_git` is `true`) sets every `GIT_*` routing var to a
/// poison (non-existent, non-repo) path so the test can verify
/// they are scrubbed at every spawn site.
///
/// Holds `ENV_LOCK` for the entire closure (which may `.await`) so
/// concurrent tests cannot observe each other's env mutations. The
/// `await_holding_lock` allow is justified because all integration
/// tests use `#[tokio::test]` with the default current-thread
/// flavor and the closures do not spawn tasks; the alternative
/// (drop-and-reacquire with shared state) is racy.
///
/// The mutex is **not reentrant**. Do not nest this helper with
/// itself or with [`with_leaked_git_env`].
///
/// Env restoration runs via [`EnvSnapshot`]'s `Drop` impl, so
/// `NB_DIR`, `HOME`, and the `GIT_*` poison vars are restored to
/// their pre-call values even if the closure panics. Without that
/// guard an assertion failure inside a `with_isolated_env` closure
/// would leak the fixture's `NB_DIR` / `HOME` and the poison vars
/// into the next test that acquires `ENV_LOCK` (notably
/// `with_leaked_git_env`, which does not overwrite `NB_DIR` or
/// `HOME`).
#[allow(clippy::await_holding_lock)]
pub async fn with_isolated_env<F, Fut, R>(env: &NbTestEnv, poison_git: bool, f: F) -> R
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R>,
{
    let _guard = lock_env();
    // `_snap` is an RAII guard: its `Drop` impl restores the env
    // (see [`EnvSnapshot`]). Holding the binding until end of scope
    // is the whole point.
    let _snap = EnvSnapshot::capture(ENV_VARS_OF_INTEREST);
    // SAFETY: serialized by ENV_LOCK; restoration runs via `_snap`'s
    // Drop impl on both normal return and panic unwinding.
    unsafe { std::env::set_var("NB_DIR", env.nb_dir()) };
    unsafe { std::env::set_var("HOME", env.home_dir()) };
    if poison_git {
        set_poison();
    }
    f().await
}

/// Sync counterpart to [`with_isolated_env`] for tests that do not
/// exercise the `nb` CLI through `NbClient` (e.g., the
/// `git_rev_parse` test and the per-var child-env probe). The
/// mutex is released before returning so subsequent tests can
/// acquire it. Env restoration runs via [`EnvSnapshot`]'s `Drop`
/// impl on panic as well as normal return.
pub fn with_leaked_git_env<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = lock_env();
    let _snap = EnvSnapshot::capture(ENV_VARS_OF_INTEREST);
    // SAFETY: see with_isolated_env.
    set_poison();
    f()
}

fn set_poison() {
    // SAFETY: see with_isolated_env.
    unsafe { std::env::set_var("GIT_DIR", "/nonexistent-GIT_DIR-poison") };
    unsafe { std::env::set_var("GIT_INDEX_FILE", "/nonexistent-GIT_INDEX_FILE-poison") };
    unsafe { std::env::set_var("GIT_COMMON_DIR", "/nonexistent-GIT_COMMON_DIR-poison") };
    unsafe { std::env::set_var("GIT_WORK_TREE", "/nonexistent-GIT_WORK_TREE-poison") };
    unsafe {
        std::env::set_var(
            "GIT_OBJECT_DIRECTORY",
            "/nonexistent-GIT_OBJECT_DIRECTORY-poison",
        )
    };
    unsafe {
        std::env::set_var(
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "/nonexistent-GIT_ALTERNATE_OBJECT_DIRECTORIES-poison",
        )
    };
}

/// Captures the current value of a fixed set of env vars and
/// restores them on [`restore`](Self::restore) or on `Drop`.
///
/// Uses [`std::env::var_os`] so non-UTF-8 values round-trip
/// faithfully. (`var().ok()` would silently treat non-UTF-8 as
/// absent and drop the bytes on restore.)
pub struct EnvSnapshot {
    vars: Vec<(String, Option<OsString>)>,
}

impl EnvSnapshot {
    pub fn capture(names: &[&str]) -> Self {
        let vars = names
            .iter()
            .map(|name| (name.to_string(), std::env::var_os(name)))
            .collect();
        Self { vars }
    }

    pub fn restore(&self) {
        for (name, value) in &self.vars {
            match value {
                Some(v) => unsafe { std::env::set_var(name, v) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

impl Drop for EnvSnapshot {
    fn drop(&mut self) {
        // Idempotent with `restore()`: callers that invoke
        // `restore()` explicitly then drop will write the same
        // values twice, which is harmless. The Drop impl exists so
        // restoration also runs on panic unwinding.
        self.restore();
    }
}

/// Resolve `nb` against the given PATH value. Returns the
/// absolute path of the first executable `nb` found, or
/// `None` if no match exists in any PATH directory. Continues
/// past missing or non-executable candidates rather than
/// bailing on the first missing dir — a typical CI PATH may
/// have many directories, only one of which contains `nb`.
/// Used by [`with_shim_nb_env`] to embed the real-binary path
/// in the shim so pass-through works in CI
/// (`$HOME/.local/bin/nb` per the `qa` workflow) as well as on
/// local machines (`/usr/local/bin/nb`).
///
/// Unix-only: the only caller is [`with_shim_nb_env`] which is
/// `#[cfg(unix)]`. The non-Unix stub was removed in the sixth
/// fixup because it was private dead code (no caller on
/// non-Unix) and would fail warning-denied cross-platform
/// Clippy. Tests and helpers are gated `#[cfg(unix)]` end to
/// end, so the resolver has no non-Unix surface.
#[cfg(unix)]
fn resolve_nb_in_path(path_var: Option<&OsString>) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path_var = path_var?;
    for dir in std::env::split_paths(path_var) {
        let candidate = dir.join("nb");
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            // Missing dir or file: not a candidate; keep scanning.
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        if metadata.permissions().mode() & 0o111 == 0 {
            continue;
        }
        return Some(candidate);
    }
    None
}

/// Run a test closure with a fake `nb` shim in `PATH` that
/// emits `shim_output` (verbatim) as its stdout, plus
/// standard fixture env (`NB_DIR`, `HOME` from the
/// `NbTestEnv`).
///
/// Used to drive the `NbClient` public API with crafted `nb`
/// output that real `nb 7.24.0` does not produce — for example,
/// CRLF terminators, missing terminators, or `0 <kind>.`
/// signal lines without a trailing hint block. These cases
/// cannot be triggered through real `nb` invocations, but
/// the public API contract is exercised end-to-end via the
/// shim.
///
/// The helper acquires `ENV_LOCK`, lets `EnvSnapshot`
/// (which now covers `PATH` and `SHIM_OUTPUT` via
/// `ENV_VARS_OF_INTEREST`) capture the current env, writes a
/// fake `nb` script to a temporary directory, prepends that
/// directory to `PATH`, sets `SHIM_OUTPUT`, runs the closure,
/// and restores the env on drop. The RAII snapshot is
/// panic-safe: a panic inside the closure triggers `Drop`
/// which restores `PATH` / `SHIM_OUTPUT` / `NB_DIR` / `HOME` /
/// `GIT_*` to their pre-helper values before the tempdir is
/// dropped (avoiding the "deleted tempdir in PATH" poisoning
/// of subsequent tests).
#[cfg(unix)]
#[allow(dead_code, clippy::await_holding_lock)]
pub async fn with_shim_nb_env<F, Fut, R>(env: &NbTestEnv, shim_output: &str, f: F) -> R
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R>,
{
    let _guard = lock_env();
    let _snap = EnvSnapshot::capture(ENV_VARS_OF_INTEREST);

    // Resolve the real `nb` binary against the unmodified PATH
    // BEFORE prepending the shim dir. The shim uses this path
    // for pass-through (`exec "${REAL_NB}"`), so the helper
    // works on both local machines (`nb` at `/usr/local/bin/nb`)
    // and CI (`nb` at `$HOME/.local/bin/nb` per `qa` workflow's
    // `Install nb` step). If no `nb` is found in PATH, fail
    // setup clearly rather than hard-coding a fallback (which
    // would recreate the portability failure).
    let path_var = std::env::var_os("PATH");
    let real_nb = resolve_nb_in_path(path_var.as_ref()).unwrap_or_else(|| {
        panic!(
            "with_shim_nb_env: could not find `nb` in PATH. \
             Ensure `nb` is installed and discoverable. \
             Checked PATH={:?}",
            path_var
        )
    });

    let shim_dir = tempfile::Builder::new()
        .prefix("nb-shim-")
        .tempdir()
        .expect("create shim tempdir");
    let shim_path = shim_dir.path();
    std::fs::write(
        shim_path.join("nb"),
        SHIM_SCRIPT_TEMPLATE.replace("__REAL_NB__", &real_nb.display().to_string()),
    )
    .expect("write shim script");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(shim_path.join("nb"), std::fs::Permissions::from_mode(0o755))
            .expect("chmod shim script");
    }

    // SAFETY: serialized by ENV_LOCK (held above).
    let new_path = match path_var.as_ref() {
        Some(p) => format!("{}:{}", shim_path.display(), p.to_string_lossy()),
        None => shim_path.display().to_string(),
    };
    unsafe {
        std::env::set_var("PATH", &new_path);
    }
    unsafe {
        std::env::set_var("SHIM_OUTPUT", shim_output);
    }
    unsafe {
        std::env::set_var("NB_DIR", env.nb_dir());
    }
    unsafe {
        std::env::set_var("HOME", env.home_dir());
    }

    // EnvSnapshot's Drop restores PATH, SHIM_OUTPUT, NB_DIR,
    // HOME, GIT_* to the values captured at function entry —
    // panic-safe (runs on unwind as well as on normal return).
    f().await
}

/// Panic-restoration regression for the RAII `EnvSnapshot`.
///
/// `with_shim_nb_env` is the production path that captures
/// and restores `PATH` / `SHIM_OUTPUT` / `NB_DIR` / `HOME` /
/// `GIT_*` on `Drop`. This test exercises the `Drop`
/// restoration directly inside one locked critical section so
/// it is not subject to races from concurrent helper
/// invocations (MCP Owner caught a racy variant in the
/// previous fixup round that read env vars before and after
/// the helper outside `ENV_LOCK`). The test asserts that a
/// panic inside the locked scope triggers the `Drop` and
/// restores the captured values, including the `PATH` and
/// `SHIM_OUTPUT` entries that were added to
/// `ENV_VARS_OF_INTEREST` in the fourth fixup.
#[test]
fn env_snapshot_restores_path_and_shim_output_on_panic() {
    // Hold ENV_LOCK for the ENTIRE test (baseline read, capture,
    // panic, and post-catch assertions). The lock is acquired
    // OUTSIDE the catch_unwind closure so it is still held
    // when the post-catch assertions run — without this,
    // another helper that acquires the lock between catch_unwind
    // returning and the assertions reading env could
    // interleave and the post-catch reads could observe that
    // helper's values rather than ours. This is not a
    // self-locking helper call (no `with_shim_nb_env` or
    // `with_isolated_env` inside the closure), so there is no
    // reentrancy concern.
    let _guard = lock_env();

    let path_before = std::env::var_os("PATH").expect("PATH must be set");
    let shim_output_before = std::env::var_os("SHIM_OUTPUT");

    let result = std::panic::catch_unwind(|| {
        let _snap = EnvSnapshot::capture(ENV_VARS_OF_INTEREST);
        // SAFETY: serialized by ENV_LOCK (held by `_guard`
        // declared outside this closure).
        unsafe {
            std::env::set_var("PATH", "/poisoned/path");
            std::env::set_var("SHIM_OUTPUT", "poisoned-output");
        }
        // Verify the poisoning is observable before the panic.
        assert_eq!(
            std::env::var_os("PATH").as_deref(),
            Some(std::ffi::OsStr::new("/poisoned/path"))
        );
        assert_eq!(
            std::env::var_os("SHIM_OUTPUT").as_deref(),
            Some(std::ffi::OsStr::new("poisoned-output"))
        );
        panic!("env_snapshot_restores_path_and_shim_output_on_panic: deliberate panic");
    });

    assert!(
        result.is_err(),
        "the closure should have panicked; if it returned Ok, the panic did not propagate"
    );

    // Lock is still held (`_guard` not yet dropped) — assertions
    // run inside the same critical section as the baseline.
    let path_after = std::env::var_os("PATH").expect("PATH must be set");
    let shim_output_after = std::env::var_os("SHIM_OUTPUT");
    assert_eq!(
        path_after, path_before,
        "PATH must be restored to its pre-helper value after a panic; \
         EnvSnapshot::Drop must run on unwind"
    );
    assert_eq!(
        shim_output_after, shim_output_before,
        "SHIM_OUTPUT must be restored to its pre-helper value after a panic; \
         EnvSnapshot::Drop must run on unwind"
    );
}
