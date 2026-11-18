//! Integration tests that exercise `NbClient` end-to-end against a
//! real `nb` CLI. Each module covers a single regression concern and
//! uses the `NbTestEnv` fixture from `nb_api::testing` for hermetic
//! isolation.
//!
//! The `qa` workflow installs `nb` (pinned to the `7.24.0` tag)
//! before running `cargo test`; these tests assume `nb` resolves
//! on `PATH`.

mod common;
/// Cross-process coexistence races against the real `nb` CLI.
/// Unix only: every test in this module is commit-dense against
/// nb-created fixtures, and nb 7.24.0 fires its init/checkpoint
/// commits in a background subshell that the Git Bash `.cmd`
/// launcher tears down, racing every commit baseline
/// (todos/api/9). Mutation coverage stays on Unix until the
/// Windows-viability spike; on Windows the read-only suites plus
/// the hardened nbrc write_path remain enabled.
#[cfg(unix)]
mod cross_process;
mod duplicate_title;
mod edit_mode_required;
mod empty_result_hints;
mod eol_model;
mod git_env_scrub;
mod native_lists;
mod native_reads;
mod native_reads_guard;
mod native_reads_support;
mod nbrc_defaults;
mod note_identity;
mod show_line_wrap;
mod show_probe;
mod subcommand_collision;
mod tasks_tags;
mod transaction;

#[cfg(feature = "testing-tokio")]
#[path = "async_helpers.rs"]
mod async_helpers_under_testing_tokio;
