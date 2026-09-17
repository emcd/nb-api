//! Hermetic test fixtures for integration tests that exercise the `nb`
//! CLI.
//!
//! This module is gated behind the `testing` Cargo feature so that
//! test-only dependencies (notably [`tempfile`]) do not enter the
//! build graph of normal consumers. Enable with
//! `--features testing`; add `--features testing-tokio` to also reach
//! the async helpers ([`NbTestEnv::configure_tokio`],
//! [`NbTestEnv::nb_command_async`]).
//!
//! # Quick start
//!
//! ```no_run
//! # #[cfg(feature = "testing")]
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use nb_api::testing::NbTestEnv;
//!
//! let env = NbTestEnv::new()?;
//! let mut command = env.nb_command();
//! command.arg("notebooks");
//! let output = command.output()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Hermeticity
//!
//! The fixture owns an isolated [`NB_DIR`] (the data store where `nb`
//! writes notebooks) and a separate [`NbTestEnv::working_dir`]
//! (the execution cwd for spawned `nb` processes). Neither inherits
//! the caller's project-root CWD by default, though
//! [`NbTestEnvBuilder::working_directory`] can supply a caller-owned
//! path (the fixture then does not delete it on `Drop`). Cleanup
//! happens on `Drop`.
//!
//! Inherited `GIT_*` routing vars (`GIT_DIR`, `GIT_INDEX_FILE`,
//! `GIT_COMMON_DIR`, `GIT_WORK_TREE`, `GIT_OBJECT_DIRECTORY`,
//! `GIT_ALTERNATE_OBJECT_DIRECTORIES`) are stripped via
//! [`crate::git_env::scrub_git_env`] (or its `std` sibling) before
//! any intentional fixture overrides apply. See `nb-api:issues/3`.
//!
//! [`NB_DIR`]: https://github.com/xwmx/nb#environment-variables
//!
//! # `nb` binary resolution
//!
//! The fixture spawns the `nb` CLI during initialization via an
//! **absolute path** discovered once per process (see
//! [`nb_binary`]). Child processes receive a **safe `PATH`** that
//! always includes system directories so `#!/usr/bin/env bash` on
//! the `nb` script can resolve `bash` even when a concurrent test
//! has poisoned the parent process `PATH` (see `nb-api:issues/api/7`).
//!
//! The repository's `qa` workflow installs `nb` (pinned to the
//! `7.24.0` tag) before running tests. Override discovery with
//! `NB_API_TEST_NB` when needed.

mod binary;
mod env;
mod home;

pub use binary::{fixture_child_path, nb_binary};
pub use env::{NbFailure, NbTestEnv, NbTestEnvBuilder, NbTestError};
