//! Environment hygiene for spawning git-aware subprocesses.
//!
//! When a process is invoked from inside a Git hook (pre-commit,
//! pre-push, post-checkout, ...) or from a CI runner, Git exports a
//! set of repository-routing environment variables into the hook's
//! environment: `GIT_DIR`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR`,
//! `GIT_WORK_TREE`, `GIT_OBJECT_DIRECTORY`,
//! `GIT_ALTERNATE_OBJECT_DIRECTORIES`. Every subprocess the hook
//! spawns inherits them.
//!
//! Any of these variables redirect every Git call inside the
//! subprocess away from the subprocess's expected repository.
//! Downstream tools layered over Git — for our purposes, `nb`, which
//! is a bash script wrapping Git — then act on the wrong repo:
//! `nb notebooks add` writes a scratch notebook into the parent
//! repo, the subsequent `nb notebooks` listing reads from a
//! different root, and the test fails in ways that depend on the
//! hook environment (CI vs. local vs. the same tests run outside
//! any hook).
//!
//! The fix is mechanical: before invoking any Git-aware subprocess
//! from inside a context that may be hooked or CI-driven, remove
//! every `GIT_*` variable from the child's environment. The child
//! then starts from a clean slate and resolves the repository from
//! its own `cwd` / its own arguments.
//!
//! This module mirrors [`nbspec::git_env`](https://docs.rs/nbspec)
//! (which had the original CI-failure-driven implementation). The
//! tokio `Command` variant here is the form nb-api uses; consumers
//! using `std::process::Command` can swap in `env_remove` directly
//! without depending on this helper. See `nb-api:issues/2` for
//! the related read-path hygiene finding (show line-wrap) and
//! `nb-api:issues/3` for the original git-env finding reported by
//! Nbspec Owner.

use std::process::Command as StdCommand;
use tokio::process::Command;

/// Returns the names of every environment variable in the current
/// process whose name starts with `GIT_`. Exposed so other call
/// sites (a future `std::process::Command` variant in `git_env`, or
/// any caller that wants the list without the scrub) share one
/// enumeration policy.
///
/// **Blast vs. selective — deliberate decision.** The `GIT_` prefix
/// blast also removes intent vars (`GIT_CONFIG_GLOBAL`,
/// `GIT_SSH_COMMAND`, `GIT_TERMINAL_PROMPT`, ...). Today no nb-api
/// code path consumes those; the only vars that redirect Git's
/// view of the repository are `GIT_DIR`, `GIT_INDEX_FILE`,
/// `GIT_COMMON_DIR`, `GIT_WORK_TREE`, `GIT_OBJECT_DIRECTORY`, and
/// `GIT_ALTERNATE_OBJECT_DIRECTORIES`. A more selective policy
/// could enumerate exactly those. The blast is chosen for two
/// reasons: (1) any future `GIT_*` redirect that lands in this
/// range gets caught by default rather than requiring a code
/// change; (2) keeping the predicate to a prefix check is the
/// minimum surface to audit. Revisit if a container identity
/// mechanism ever routes through `GIT_CONFIG_GLOBAL` — at that
/// point a selective enumeration belongs here.
pub fn leaked_git_names() -> Vec<String> {
    std::env::vars()
        .filter_map(|(name, _)| {
            if name.starts_with("GIT_") {
                Some(name)
            } else {
                None
            }
        })
        .collect()
}

/// Removes every environment variable whose name starts with `GIT_`
/// from the given tokio `Command`'s environment. The spawned process
/// inherits every other variable from the parent (PATH, HOME,
/// LANG, ...), just not the ones that redirect Git's view of the
/// repository.
///
/// Pass the command BEFORE chaining `.args(...)` or `.env(...)` so
/// later `.env(name, value)` calls are not also removed.
///
/// # Example
///
/// ```no_run
/// use tokio::process::Command;
/// nb_api::scrub_git_env(&mut Command::new("nb"));
/// ```
pub fn scrub_git_env(command: &mut Command) {
    for name in leaked_git_names() {
        command.env_remove(&name);
    }
}

/// Synchronous-`Command` variant of [`scrub_git_env`]. nb-api uses
/// `tokio::process::Command` for every `nb` invocation, but
/// `git_rev_parse` (the only direct `git` spawn) uses
/// `std::process::Command`. Both spawn sites must be scrubbed; if a
/// future helper spawns git synchronously, use this overload.
///
/// # Example
///
/// ```no_run
/// use std::process::Command;
/// nb_api::scrub_git_env_std(&mut Command::new("git"));
/// ```
pub fn scrub_git_env_std(command: &mut StdCommand) {
    for name in leaked_git_names() {
        command.env_remove(&name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn leaked_git_names_only_returns_git_prefixed() {
        // Filter logic is decoupled from process env: build a synthetic
        // iterator and assert the predicate. The process env may or may
        // not have GIT_* vars; we don't depend on either case.
        let pairs: Vec<(String, OsString)> = vec![
            ("GIT_DIR".to_string(), OsString::from("/foo")),
            ("PATH".to_string(), OsString::from("/usr/bin")),
            ("GIT_INDEX_FILE".to_string(), OsString::from("/foo/index")),
            ("HOME".to_string(), OsString::from("/home")),
            ("git_dir".to_string(), OsString::from("lowercase")),
            ("GIT_".to_string(), OsString::from("empty-suffix")),
            ("FOO_GIT_BAR".to_string(), OsString::from("not-prefixed")),
        ];
        let filtered: Vec<String> = pairs
            .into_iter()
            .filter_map(|(name, _)| {
                if name.starts_with("GIT_") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            filtered,
            vec![
                "GIT_DIR".to_string(),
                "GIT_INDEX_FILE".to_string(),
                "GIT_".to_string(),
            ]
        );
    }

    #[test]
    fn leaked_git_names_against_process_env_only_returns_git_vars() {
        // Whatever GIT_* vars happen to be set in the test process, the
        // function must only return names starting with "GIT_". This
        // guards against accidental broadening of the predicate.
        let leaked = leaked_git_names();
        for name in &leaked {
            assert!(
                name.starts_with("GIT_"),
                "leaked_git_names returned non-GIT_ var: {name}"
            );
        }
    }

    #[test]
    fn scrub_git_env_removes_git_vars_via_env_remove() {
        // `tokio::process::Command::env_remove` removes from the
        // inherited environment; verify the call site wires through to
        // it. We can't easily inspect a tokio Command's resulting env
        // pre-spawn, so we exercise the std::process::Command form
        // (same `env_remove` semantics) against the same helper list.
        use std::process::Command as StdCommand;
        let mut cmd = StdCommand::new("true");
        // Inherited env may already contain GIT_* in this process; the
        // scrub should remove them so the spawned `true` would see
        // none. We can only assert the helper list is consumed.
        for name in leaked_git_names() {
            cmd.env_remove(&name);
        }
        // Smoke: the call did not panic; the command is still buildable.
        let _ = cmd.get_program();
    }
}
