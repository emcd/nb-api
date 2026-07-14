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
//! # `nb` must be available on `PATH`
//!
//! The fixture spawns the `nb` CLI during initialization. Tests
//! using this module assume `nb` resolves on `PATH`. The repository's
//! `qa` workflow installs `nb` (pinned to the `7.24.0` tag) before
//! running tests.

use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, ExitStatus};

use crate::git_env::scrub_git_env_std;

#[cfg(feature = "testing-tokio")]
use tokio::process::Command as TokioCommand;

const DEFAULT_NOTEBOOK: &str = "scratch";

const GIT_AUTHOR_NAME: &str = "nb-api tests";
const GIT_AUTHOR_EMAIL: &str = "nb-api@localhost";

/// A captured `nb` subprocess failure: exit status, stdout, and
/// stderr preserved separately so callers can inspect all three
/// streams when a fixture-initialization command fails.
#[derive(Debug)]
pub struct NbFailure {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl std::fmt::Display for NbFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "exit status: {}\nstdout: {}\nstderr: {}",
            self.status,
            if self.stdout.is_empty() {
                "<empty>"
            } else {
                &self.stdout
            },
            if self.stderr.is_empty() {
                "<empty>"
            } else {
                &self.stderr
            },
        )
    }
}

impl std::error::Error for NbFailure {}

/// Errors raised while building or initializing an [`NbTestEnv`].
#[derive(Debug, thiserror::Error)]
pub enum NbTestError {
    #[error("io error during {context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("nb command failed during {context}: {failure}")]
    Nb {
        context: String,
        #[source]
        failure: NbFailure,
    },
}

/// Builder for [`NbTestEnv`]. Constructed via [`NbTestEnv::builder`].
#[derive(Debug, Default)]
pub struct NbTestEnvBuilder {
    notebook: Option<String>,
    working_directory: Option<PathBuf>,
}

impl NbTestEnvBuilder {
    /// Override the notebook name created during fixture initialization.
    pub fn notebook(mut self, name: impl Into<String>) -> Self {
        self.notebook = Some(name.into());
        self
    }

    /// Use a caller-owned path as the execution cwd of spawned `nb`
    /// processes. The fixture does not delete this path on `Drop`;
    /// the fixture-owned data store under the fixture's root tempdir
    /// is still cleaned up automatically.
    pub fn working_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(path.into());
        self
    }

    /// Build the fixture, initialize its notebook, and return it.
    pub fn build(self) -> Result<NbTestEnv, NbTestError> {
        let root = tempfile::Builder::new()
            .prefix("nb-api-test-")
            .tempdir()
            .map_err(|e| NbTestError::Io {
                context: "create isolated root tempdir".to_string(),
                source: e,
            })?;
        let nb_dir = root.path().join(".nb");
        std::fs::create_dir_all(&nb_dir).map_err(|e| NbTestError::Io {
            context: format!("create nb dir at {}", nb_dir.display()),
            source: e,
        })?;
        let working_dir = match self.working_directory {
            Some(path) => path,
            None => {
                let path = root.path().join("work");
                std::fs::create_dir_all(&path).map_err(|e| NbTestError::Io {
                    context: format!("create working dir at {}", path.display()),
                    source: e,
                })?;
                path
            }
        };
        let home_dir = root.path().join("home");
        std::fs::create_dir_all(&home_dir).map_err(|e| NbTestError::Io {
            context: format!("create fixture HOME at {}", home_dir.display()),
            source: e,
        })?;
        let notebook = self
            .notebook
            .unwrap_or_else(|| DEFAULT_NOTEBOOK.to_string());
        let env = NbTestEnv {
            root,
            nb_dir,
            working_dir,
            home_dir,
            notebook,
        };
        env.initialize_notebook()?;
        Ok(env)
    }
}

/// Hermetic fixture for integration tests that exercise the `nb` CLI.
///
/// Constructed via [`NbTestEnv::new`] for the common case or
/// [`NbTestEnv::builder`] for configuration.
///
/// # Drop semantics
///
/// The fixture-owned root tempdir is removed on `Drop`. A
/// caller-supplied `working_directory` (via [`NbTestEnvBuilder`]) is
/// outside the root and is left intact.
pub struct NbTestEnv {
    /// Held for its `Drop` cleanup of the fixture-owned root
    /// tempdir; never read by name. The fixture's derived paths
    /// (`nb_dir`, `working_dir`) are stored separately so callers
    /// can inspect them without taking the root.
    #[allow(dead_code)]
    root: tempfile::TempDir,
    nb_dir: PathBuf,
    working_dir: PathBuf,
    /// Fixture-owned `$HOME` so `nb`'s `_git_required` global-config
    /// check (`git config --global --includes user.name`) finds a
    /// deterministic `user.name`/`user.email` and never falls into
    /// its interactive prompt. The fixture writes `.gitconfig`
    /// here during [`initialize_notebook`](Self::initialize_notebook).
    home_dir: PathBuf,
    notebook: String,
}

impl NbTestEnv {
    /// Build a fixture with hermetic defaults (isolated root,
    /// default notebook name, separate `nb_dir` and `working_dir`,
    /// scrubbed inherited `GIT_*`, deterministic git identity, signing
    /// disabled).
    pub fn new() -> Result<Self, NbTestError> {
        Self::builder().build()
    }

    /// Begin building a fixture with non-default knobs.
    pub fn builder() -> NbTestEnvBuilder {
        NbTestEnvBuilder::default()
    }

    /// Path of the `NB_DIR` data store where `nb` writes notebooks.
    /// Isolated git repository under the fixture-owned root.
    pub fn nb_dir(&self) -> &Path {
        &self.nb_dir
    }

    /// Path of the execution cwd for spawned `nb` processes. Distinct
    /// from [`nb_dir`](Self::nb_dir) by design.
    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    /// Fixture-owned `$HOME` directory. The fixture writes a
    /// deterministic `.gitconfig` here so `nb`'s `_git_required`
    /// global-config check always finds a `user.name`/`user.email`
    /// and never falls into its interactive prompt.
    pub fn home_dir(&self) -> &Path {
        &self.home_dir
    }

    /// Name of the notebook created during fixture initialization.
    pub fn notebook(&self) -> &str {
        &self.notebook
    }

    /// Apply the fixture's environment to a `std::process::Command`:
    /// strip inherited `GIT_*` routing vars, set `NB_DIR`, set a
    /// deterministic git author/committer identity, disable commit
    /// and tag signing, and set `current_dir` to
    /// [`working_dir`](Self::working_dir).
    pub fn configure_std(&self, cmd: &mut StdCommand) {
        scrub_git_env_std(cmd);
        cmd.env("NB_DIR", &self.nb_dir);
        cmd.env("HOME", &self.home_dir);
        cmd.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_CONFIG_COUNT", "2");
        cmd.env("GIT_CONFIG_KEY_0", "commit.gpgsign");
        cmd.env("GIT_CONFIG_VALUE_0", "false");
        cmd.env("GIT_CONFIG_KEY_1", "tag.gpgsign");
        cmd.env("GIT_CONFIG_VALUE_1", "false");
        cmd.current_dir(&self.working_dir);
    }

    /// Async counterpart to [`configure_std`](Self::configure_std).
    /// Available only with the `testing-tokio` Cargo feature.
    #[cfg(feature = "testing-tokio")]
    pub fn configure_tokio(&self, cmd: &mut TokioCommand) {
        crate::git_env::scrub_git_env(cmd);
        cmd.env("NB_DIR", &self.nb_dir);
        cmd.env("HOME", &self.home_dir);
        cmd.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_CONFIG_COUNT", "2");
        cmd.env("GIT_CONFIG_KEY_0", "commit.gpgsign");
        cmd.env("GIT_CONFIG_VALUE_0", "false");
        cmd.env("GIT_CONFIG_KEY_1", "tag.gpgsign");
        cmd.env("GIT_CONFIG_VALUE_1", "false");
        cmd.current_dir(&self.working_dir);
    }

    /// Convenience accessor: a fresh `std::process::Command` for `nb`
    /// with the fixture's environment applied. Degenerate form of
    /// `configure_std(Command::new("nb"))`; both call sites are valid.
    pub fn nb_command(&self) -> StdCommand {
        let mut cmd = StdCommand::new("nb");
        self.configure_std(&mut cmd);
        cmd
    }

    /// Async counterpart to [`nb_command`](Self::nb_command). Available
    /// only with the `testing-tokio` Cargo feature.
    #[cfg(feature = "testing-tokio")]
    pub fn nb_command_async(&self) -> TokioCommand {
        let mut cmd = TokioCommand::new("nb");
        self.configure_tokio(&mut cmd);
        cmd
    }

    fn initialize_notebook(&self) -> Result<(), NbTestError> {
        // Write `$HOME/.gitconfig` so `nb`'s `_git_required` global
        // check finds a deterministic `user.name`/`user.email` and
        // never falls into its interactive prompt. Without this, the
        // first `nb` invocation hangs on a `read` for Name/Email
        // when stdin is not a TTY.
        let gitconfig = self.home_dir.join(".gitconfig");
        let gitconfig_contents =
            format!("[user]\n\tname = {GIT_AUTHOR_NAME}\n\temail = {GIT_AUTHOR_EMAIL}\n",);
        std::fs::write(&gitconfig, gitconfig_contents).map_err(|e| NbTestError::Io {
            context: format!("write .gitconfig to {}", gitconfig.display()),
            source: e,
        })?;

        // Pre-create a hidden init stub. `nb`'s main loop short-
        // circuits its first-run `_init` (welcome screen and
        // interactive author prompt) when both `NB_DIR` and
        // `NB_NOTEBOOK_PATH` exist. The stub satisfies that check
        // without leaving a phantom `home` notebook: a leading dot
        // hides it from `ls -1` (which `nb notebooks` uses), and
        // `NB_NOTEBOOK_PATH` is pointed at the stub only for the
        // init command. Subsequent commands use `.current` and
        // never resolve through the stub.
        //
        // We use `.init_stub` rather than the conventional
        // `home` so that callers can build a fixture with
        // `notebook("home")` if they want — there is no conflict
        // because the stub is a different name.
        let init_stub = self.nb_dir.join(".init_stub");
        std::fs::create_dir_all(&init_stub).map_err(|e| NbTestError::Io {
            context: format!("create init stub at {}", init_stub.display()),
            source: e,
        })?;

        // Build the init command's env inline rather than via
        // `configure_std`. The init command needs `NB_NOTEBOOK_PATH`
        // pointing at the stub; `configure_std` deliberately does
        // not set `NB_NOTEBOOK_PATH` because subsequent operations
        // must resolve the current notebook through `.current`.
        let mut cmd = StdCommand::new("nb");
        scrub_git_env_std(&mut cmd);
        cmd.env("NB_DIR", &self.nb_dir);
        cmd.env("NB_NOTEBOOK_PATH", &init_stub);
        cmd.env("HOME", &self.home_dir);
        cmd.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_CONFIG_COUNT", "2");
        cmd.env("GIT_CONFIG_KEY_0", "commit.gpgsign");
        cmd.env("GIT_CONFIG_VALUE_0", "false");
        cmd.env("GIT_CONFIG_KEY_1", "tag.gpgsign");
        cmd.env("GIT_CONFIG_VALUE_1", "false");
        cmd.current_dir(&self.working_dir);
        cmd.arg("notebooks").arg("add").arg(&self.notebook);
        let output = cmd.output().map_err(|e| NbTestError::Io {
            context: format!("spawn `nb notebooks add {}`", self.notebook),
            source: e,
        })?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            return Err(NbTestError::Nb {
                context: format!("`nb notebooks add {}`", self.notebook),
                failure: NbFailure {
                    status: output.status,
                    stdout,
                    stderr,
                },
            });
        }

        // Point `nb` at our notebook so subsequent invocations
        // target it via the on-disk `.current` marker that nb
        // reads on every call. The init stub is left in place
        // (hidden, never listed) so the test process can verify
        // `_init` bypass behavior without rebuilding it.
        std::fs::write(self.nb_dir.join(".current"), &self.notebook).map_err(|e| {
            NbTestError::Io {
                context: format!(
                    "write .current to {}",
                    self.nb_dir.join(".current").display()
                ),
                source: e,
            }
        })?;

        Ok(())
    }
}
