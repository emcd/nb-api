use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, ExitStatus, Stdio};

use crate::git_env::scrub_git_env_std;

#[cfg(feature = "testing-tokio")]
use tokio::process::Command as TokioCommand;

use super::binary::{fixture_child_path, nb_binary};

const DEFAULT_NOTEBOOK: &str = "scratch";

const GIT_AUTHOR_NAME: &str = "nb-api tests";
const GIT_AUTHOR_EMAIL: &str = "nb-api@localhost";

/// Deterministic git configuration applied to every fixture-spawned git
/// command, on top of the signing overrides. Forces `core.autocrlf=false`
/// so notebook repos are byte-identical across platforms: Git-for-Windows
/// defaults `autocrlf=true`, which renormalizes committed files to CRLF on
/// checkout and makes a fresh nb init commit appear dirty to the
/// transaction's baseline check (`nb-api:todos/api/9`, Windows CI finding).
///
/// Uses the `GIT_CONFIG_COUNT` mechanism so no global/`HOME` config file
/// is needed (the fixture HOME is a tempdir anyway).
fn apply_git_config_env(cmd: &mut impl GitEnvSetter) {
    cmd.env("GIT_CONFIG_COUNT", "4");
    cmd.env("GIT_CONFIG_KEY_0", "commit.gpgsign");
    cmd.env("GIT_CONFIG_VALUE_0", "false");
    cmd.env("GIT_CONFIG_KEY_1", "tag.gpgsign");
    cmd.env("GIT_CONFIG_VALUE_1", "false");
    cmd.env("GIT_CONFIG_KEY_2", "core.autocrlf");
    cmd.env("GIT_CONFIG_VALUE_2", "false");
    cmd.env("GIT_CONFIG_KEY_3", "core.eol");
    cmd.env("GIT_CONFIG_VALUE_3", "lf");
}

/// Minimal surface shared by [`StdCommand`] and [`TokioCommand`] so the
/// fixture's deterministic git-config env can be applied to both.
trait GitEnvSetter {
    fn env<K, V>(&mut self, key: K, value: V)
    where
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>;
}

impl GitEnvSetter for StdCommand {
    fn env<K, V>(&mut self, key: K, value: V)
    where
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>,
    {
        StdCommand::env(self, key, value);
    }
}

#[cfg(feature = "testing-tokio")]
impl GitEnvSetter for TokioCommand {
    fn env<K, V>(&mut self, key: K, value: V)
    where
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>,
    {
        TokioCommand::env(self, key, value);
    }
}

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
    /// and tag signing, pin a safe child `PATH`, and set `current_dir`
    /// to [`working_dir`](Self::working_dir).
    pub fn configure_std(&self, cmd: &mut StdCommand) {
        scrub_git_env_std(cmd);
        cmd.env("PATH", fixture_child_path());
        cmd.env("NB_DIR", &self.nb_dir);
        cmd.env("HOME", &self.home_dir);
        cmd.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
        apply_git_config_env(cmd);
        cmd.current_dir(&self.working_dir);
    }

    /// Async counterpart to [`configure_std`](Self::configure_std).
    /// Available only with the `testing-tokio` Cargo feature.
    #[cfg(feature = "testing-tokio")]
    pub fn configure_tokio(&self, cmd: &mut TokioCommand) {
        crate::git_env::scrub_git_env(cmd);
        cmd.env("PATH", fixture_child_path());
        cmd.env("NB_DIR", &self.nb_dir);
        cmd.env("HOME", &self.home_dir);
        cmd.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
        apply_git_config_env(cmd);
        cmd.current_dir(&self.working_dir);
    }

    /// Convenience accessor: a fresh `std::process::Command` for `nb`
    /// with the fixture's environment applied. Uses the absolute
    /// [`nb_binary`] path so parent-process `PATH` poison cannot
    /// prevent executable lookup (`issues/api/7`). Stdin is nulled to
    /// mirror the production [`NbClient::exec`](crate::NbClient) spawn
    /// (prevents TTY hangs / interactive prompts).
    pub fn nb_command(&self) -> StdCommand {
        let mut cmd = StdCommand::new(nb_binary());
        self.configure_std(&mut cmd);
        cmd.stdin(Stdio::null());
        cmd
    }

    /// Async counterpart to [`nb_command`](Self::nb_command). Available
    /// only with the `testing-tokio` Cargo feature.
    #[cfg(feature = "testing-tokio")]
    pub fn nb_command_async(&self) -> TokioCommand {
        let mut cmd = TokioCommand::new(nb_binary());
        self.configure_tokio(&mut cmd);
        cmd.stdin(Stdio::null());
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
        // Absolute `nb` + safe child PATH: parent PATH may be
        // poisoned by a concurrent harness test (`issues/api/7`).
        let mut cmd = StdCommand::new(nb_binary());
        scrub_git_env_std(&mut cmd);
        cmd.env("PATH", fixture_child_path());
        cmd.env("NB_DIR", &self.nb_dir);
        cmd.env("NB_NOTEBOOK_PATH", &init_stub);
        cmd.env("HOME", &self.home_dir);
        cmd.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
        cmd.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
        cmd.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
        apply_git_config_env(&mut cmd);
        cmd.stdin(Stdio::null()); // Prevent TTY hangs (mirrors NbClient::exec)
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

        // Ensure a clean committed baseline. nb's `_git checkpoint` runs
        // the init commit in a background subshell (`( ... ) &`). When nb
        // is spawned through a `.cmd` launcher (Windows), that orphaned
        // commit is torn down when the wrapper exits, leaving `.index`
        // untracked and every later `Transaction::commit` rejected with
        // `DirtyBaseline`. Run the commit synchronously ourselves with
        // the fixture's deterministic git env so the baseline is identical
        // on Unix and Windows. See `nb-api:todos/api/9`.
        //
        // `git` resolves through the inherited process PATH (as in
        // `crate::git::git_capture`); the fixture child PATH only carries
        // nb's dir + system dirs and would not locate git on Windows.
        // If git cannot spawn (poisoned process PATH, issues/api/7), the
        // baseline is skipped — the poisoned test never runs a
        // transaction, so a missing baseline is harmless there.
        let notebook_root = self.nb_dir.join(&self.notebook);
        let baseline_result: Result<(), NbTestError> = (|| {
            // Write the notebook repo's LOCAL config so every subsequent
            // git invocation (including production `git::git_capture`,
            // which scrubs GIT_CONFIG_* env) sees byte-identical LF
            // handling. The GIT_CONFIG_COUNT env override only reaches
            // fixture-spawned commands; repo-local config persists.
            // See `nb-api:todos/api/9`.
            let mut git_config = StdCommand::new("git");
            scrub_git_env_std(&mut git_config);
            git_config.env("HOME", &self.home_dir);
            apply_git_config_env(&mut git_config);
            git_config.current_dir(&notebook_root);
            git_config.args(["config", "core.autocrlf", "false"]);
            let cfg = git_config.output().map_err(|e| NbTestError::Io {
                context: format!(
                    "fixture baseline `git config core.autocrlf false` in {}",
                    notebook_root.display()
                ),
                source: e,
            })?;
            if !cfg.status.success() {
                let stdout = String::from_utf8_lossy(&cfg.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&cfg.stderr).into_owned();
                return Err(NbTestError::Nb {
                    context: format!(
                        "fixture baseline `git config core.autocrlf false` in {}",
                        notebook_root.display()
                    ),
                    failure: NbFailure {
                        status: cfg.status,
                        stdout,
                        stderr,
                    },
                });
            }
            // nb 7.24.0 fires the notebook's init commit in a background
            // subshell (`( _git_checkpoint_commit ... ) &`). Under the
            // Git Bash `.cmd` launcher on Windows that orphaned git can
            // keep staging `.index` after our synchronous baseline commit,
            // racing the first `Transaction::commit`'s dirty check
            // (intermittent `DirtyBaseline` — see todos/api/9). Settle by
            // re-committing until the worktree/index is verifiably clean.
            // The window is generous (up to ~2.5s) because on Windows the
            // orphaned background git can be delayed by job teardown.
            let mut consecutive_clean = 0u32;
            for _ in 0..50 {
                let mut status = StdCommand::new("git");
                scrub_git_env_std(&mut status);
                status.env("HOME", &self.home_dir);
                apply_git_config_env(&mut status);
                status.current_dir(&notebook_root);
                status.args(["status", "--porcelain", "-uall", "--ignored=no"]);
                let status_out = status.output().map_err(|e| NbTestError::Io {
                    context: format!(
                        "fixture baseline `git status --porcelain` in {}",
                        notebook_root.display()
                    ),
                    source: e,
                })?;
                if !status_out.status.success() {
                    let stdout = String::from_utf8_lossy(&status_out.stdout).into_owned();
                    let stderr = String::from_utf8_lossy(&status_out.stderr).into_owned();
                    return Err(NbTestError::Nb {
                        context: format!(
                            "fixture baseline `git status --porcelain` in {}",
                            notebook_root.display()
                        ),
                        failure: NbFailure {
                            status: status_out.status,
                            stdout,
                            stderr,
                        },
                    });
                }
                let dirty = !String::from_utf8_lossy(&status_out.stdout)
                    .trim()
                    .is_empty();
                if !dirty {
                    // Require two consecutive clean checks so an orphaned
                    // nb background git that has NOT yet staged `.index`
                    // cannot slip in after a single clean observation
                    // (Windows CI DirtyBaseline flake, see todos/api/9).
                    consecutive_clean += 1;
                    if consecutive_clean >= 2 {
                        return Ok(());
                    }
                } else {
                    consecutive_clean = 0;
                }

                let mut git = StdCommand::new("git");
                scrub_git_env_std(&mut git);
                git.env("HOME", &self.home_dir);
                git.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
                git.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
                git.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
                git.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
                apply_git_config_env(&mut git);
                git.current_dir(&notebook_root);
                git.arg("add").arg("-A");
                let add = git.output().map_err(|e| NbTestError::Io {
                    context: format!(
                        "fixture baseline `git add -A` in {}",
                        notebook_root.display()
                    ),
                    source: e,
                })?;
                if !add.status.success() {
                    let stdout = String::from_utf8_lossy(&add.stdout).into_owned();
                    let stderr = String::from_utf8_lossy(&add.stderr).into_owned();
                    return Err(NbTestError::Nb {
                        context: format!(
                            "fixture baseline `git add -A` in {}",
                            notebook_root.display()
                        ),
                        failure: NbFailure {
                            status: add.status,
                            stdout,
                            stderr,
                        },
                    });
                }
                let mut git_commit = StdCommand::new("git");
                scrub_git_env_std(&mut git_commit);
                git_commit.env("HOME", &self.home_dir);
                git_commit.env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
                git_commit.env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
                git_commit.env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
                git_commit.env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
                apply_git_config_env(&mut git_commit);
                git_commit.current_dir(&notebook_root);
                git_commit.args(["commit", "-m", "[nb] Initialize"]);
                let commit = git_commit.output().map_err(|e| NbTestError::Io {
                    context: format!(
                        "fixture baseline `git commit` in {}",
                        notebook_root.display()
                    ),
                    source: e,
                })?;
                if !commit.status.success() {
                    // A `nothing to commit` exit is not an error; the
                    // settle loop re-checks status next iteration. Git
                    // may write the message to stdout or stderr.
                    let stdout = String::from_utf8_lossy(&commit.stdout);
                    let stderr = String::from_utf8_lossy(&commit.stderr);
                    if !stdout.contains("nothing to commit")
                        && !stderr.contains("nothing to commit")
                    {
                        return Err(NbTestError::Nb {
                            context: format!(
                                "fixture baseline `git commit` in {}",
                                notebook_root.display()
                            ),
                            failure: NbFailure {
                                status: commit.status,
                                stdout: stdout.into_owned(),
                                stderr: stderr.into_owned(),
                            },
                        });
                    }
                }
                // Let nb's background checkpoint finish before re-checking.
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(NbTestError::Io {
                context: format!(
                    "fixture baseline did not settle clean in {}",
                    notebook_root.display()
                ),
                source: std::io::Error::other("notebook repo never reached a clean baseline"),
            })
        })();
        if let Err(NbTestError::Io { source, .. }) = &baseline_result
            && source.kind() == std::io::ErrorKind::NotFound
        {
            // Poisoned process PATH (issues/api/7): git unresolvable,
            // baseline intentionally skipped.
            tracing::debug!("fixture baseline skipped: git not on process PATH");
        } else {
            baseline_result?;
        }

        Ok(())
    }
}
