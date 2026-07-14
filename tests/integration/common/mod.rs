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

/// All env vars [`with_isolated_env`] may read, write, or restore.
const ENV_VARS_OF_INTEREST: &[&str] = &[
    "NB_DIR",
    "HOME",
    "GIT_DIR",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_WORK_TREE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
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
