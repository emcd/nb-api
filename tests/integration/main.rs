//! Integration tests that exercise `NbClient` end-to-end against a
//! real `nb` CLI. Each module covers a single regression concern and
//! uses the `NbTestEnv` fixture from `nb_api::testing` for hermetic
//! isolation.
//!
//! The `qa` workflow installs `nb` (pinned to the `7.24.0` tag)
//! before running `cargo test`; these tests assume `nb` resolves
//! on `PATH`.

mod common;
mod duplicate_title;
mod git_env_scrub;
mod show_line_wrap;
mod show_probe;

#[cfg(feature = "testing-tokio")]
#[path = "async_helpers.rs"]
mod async_helpers_under_testing_tokio;
