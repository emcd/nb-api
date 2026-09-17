use std::path::PathBuf;
use std::process::Stdio;
use std::sync::LazyLock;

use regex::Regex;
use tokio::process::Command;

use crate::diagnostics::{append_warning, is_notebook_not_found};
use crate::error::NbError;
use crate::gate;
use crate::git::derive_git_notebook_name;
use crate::git_env::scrub_git_env;
use crate::git_signing::apply_git_signing_env;
use crate::nb_program::nb_program;
use crate::transaction::Transaction;
use crate::validate::{parse_qualified_selector, validate_notebook_name};
use crate::{Config, NbClient};

use super::read::notebook_dir_from_env;

/// Regex to match ANSI/ISO 2022 escape sequences.
///
/// Covers:
/// - Fe sequences: `ESC [@-Z\-_]` (single byte after ESC)
/// - CSI sequences: `ESC [ ... m` (SGR colors, cursor control, etc.)
/// - nF sequences: `ESC [ -/]* [0-~]` (character set designation like `ESC ( B`)
static ANSI_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~]|[ -/]*[0-~])").unwrap());

/// Strip ANSI escape sequences from text.
fn strip_ansi(text: &str) -> String {
    ANSI_REGEX.replace_all(text, "").into_owned()
}

/// Result of probing a selector's textual classification via
/// `nb show <selector> --type text`. Used by
/// [`NbClient::show_note`](crate::NbClient::show_note) to decide whether
/// the content-read path is safe.
pub(super) enum ShowClassification {
    /// `nb` classified the type as text. Caller proceeds to
    /// the content-read path.
    Textual,
    /// `nb` classified the type as non-text (folder, archive,
    /// image, audio, video, etc.). `actual_type` carries the
    /// `nb`-reported type string (file extension or `folder`).
    NonTextual { actual_type: String },
    /// The probe could not classify the selector (selector not
    /// found, internal error). Caller falls through to the
    /// original show path so existing missing-selector
    /// diagnostics are preserved.
    ProbeFailure,
}

const FOLDER_REQUIRED_MESSAGE: &str = "This server is configured to require `folder` for new notes. Use the `nb.mkdir` tool to create new folders and the `nb.folders` tool to list existing folders.";

impl NbClient {
    /// Creates a new nb client.
    ///
    /// Uses the notebook from config if set, otherwise falls back to a
    /// Git-derived notebook name. Does NOT read `NB_MCP_NOTEBOOK` —
    /// that is an MCP-server-specific env var resolved by the server.
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let default_notebook = config
            .notebook
            .as_deref()
            .map(String::from)
            .or_else(derive_git_notebook_name);
        Ok(Self {
            default_notebook,
            create_notebook: config.create_notebook,
            disable_git_signing: config.disable_git_signing,
            allow_top_level_notes: config.allow_top_level_notes,
            gate_timeout: config.gate_timeout,
        })
    }

    /// Begin a collect-then-commit [`Transaction`] for `notebook`.
    ///
    /// Construction performs no durable I/O and does not acquire the gate.
    pub async fn transaction(&self, notebook: Option<&str>) -> Result<Transaction, NbError> {
        let notebook = self.resolve_notebook_name(notebook)?;
        // Prefer filesystem existence under NB_DIR so plan construction does
        // not invoke `nb` (which can auto-checkpoint a dirty worktree).
        if notebook_dir_from_env(&notebook).is_none() {
            self.ensure_notebook(&notebook).await?;
        }
        Ok(Transaction::new(self.clone(), notebook, self.gate_timeout))
    }

    /// Process-shared gate queue timeout configured on this client.
    pub fn gate_timeout(&self) -> std::time::Duration {
        self.gate_timeout
    }

    /// Notebook path without acquiring the notebook gate (caller holds gate or
    /// is still resolving identity).
    ///
    /// Prefers `$NB_DIR/<notebook>` when that directory exists so path
    /// resolution does not invoke `nb` (which can auto-checkpoint and clear a
    /// dirty baseline before `Transaction::commit` inspects status).
    pub(crate) async fn show_notebook_path_unguarded(
        &self,
        notebook: Option<&str>,
    ) -> Result<PathBuf, NbError> {
        let notebook = self.resolve_notebook_name(notebook)?;
        if let Some(path) = notebook_dir_from_env(&notebook) {
            return Ok(path);
        }
        self.ensure_notebook(&notebook).await?;
        let output = self
            .exec_vec(vec![
                "notebooks".to_string(),
                "show".to_string(),
                notebook.clone(),
                "--path".to_string(),
            ])
            .await?;
        let path = output.trim();
        if path.is_empty() {
            return Err(NbError::CommandFailed {
                command: format!("nb notebooks show {notebook} --path"),
                stderr: "nb notebooks path output was empty".to_string(),
                exit_code: None,
            });
        }
        Ok(PathBuf::from(path))
    }

    pub(super) async fn with_notebook_gate<F, T>(&self, notebook: &str, f: F) -> Result<T, NbError>
    where
        F: std::future::Future<Output = Result<T, NbError>>,
    {
        let root = self.show_notebook_path_unguarded(Some(notebook)).await?;
        let key = gate::git_common_dir_realpath(&root)?;
        let _hold = gate::acquire_notebook(key, self.gate_timeout, false).await?;
        f.await
    }

    pub(super) fn require_folder_for_new_note(&self, folder: Option<&str>) -> Result<(), NbError> {
        if self.allow_top_level_notes || folder.is_some_and(|value| !value.trim().is_empty()) {
            return Ok(());
        }
        Err(NbError::ValidationError {
            reason: FOLDER_REQUIRED_MESSAGE.to_string(),
            location: None,
        })
    }

    pub(super) async fn resolve_target_selector(
        &self,
        id: &str,
        notebook: Option<&str>,
    ) -> Result<(String, String), NbError> {
        if let Some((embedded_notebook, path)) = parse_qualified_selector(id)? {
            let notebook = match notebook {
                Some(value) => {
                    validate_notebook_name(value)?;
                    if value != embedded_notebook {
                        return Err(NbError::ValidationError {
                            reason: format!(
                                "ambiguous selector: id targets notebook `{embedded_notebook}`, but notebook field is `{value}`"
                            ),
                            location: None,
                        });
                    }
                    embedded_notebook.to_string()
                }
                _ => embedded_notebook.to_string(),
            };
            self.ensure_existing_notebook(&notebook).await?;
            return Ok((notebook, format!("{}:{}", embedded_notebook, path)));
        }
        let notebook = self.resolve_notebook(notebook).await?;
        Ok((notebook.clone(), format!("{}:{}", notebook, id)))
    }

    pub(super) fn append_notebook_warning(&self, output: String, notebook: &str) -> String {
        let Some(default_notebook) = self.default_notebook.as_deref() else {
            return output;
        };
        if default_notebook == notebook {
            return output;
        }
        append_warning(
            output,
            format!(
                "Warning: wrote to notebook `{notebook}`, not the project default notebook `{default_notebook}`. If this was unintended, move or delete the note and retry with the correct notebook/folder."
            ),
        )
    }

    /// Resolves the notebook to use for a command.
    pub(super) fn resolve_notebook_name(&self, notebook: Option<&str>) -> Result<String, NbError> {
        if let Some(name) = notebook {
            validate_notebook_name(name)?;
            return Ok(name.to_string());
        }
        if let Some(name) = self.default_notebook.as_deref() {
            validate_notebook_name(name)?;
            return Ok(name.to_string());
        }
        Err(NbError::ValidationError {
            reason: "notebook not configured; set --notebook or NB_MCP_NOTEBOOK".to_string(),
            location: None,
        })
    }

    pub(super) async fn resolve_notebook(&self, notebook: Option<&str>) -> Result<String, NbError> {
        let name = self.resolve_notebook_name(notebook)?;
        self.ensure_notebook(&name).await?;
        Ok(name)
    }

    pub(super) async fn ensure_notebook(&self, notebook: &str) -> Result<(), NbError> {
        match self.check_notebook(notebook).await {
            Ok(()) => Ok(()),
            // Genuine infrastructure failure (no nb binary, IO
            // error) — surface verbatim; do not try to create.
            Err(err @ (NbError::ExecutableNotFound { .. } | NbError::Io { .. })) => Err(err),
            // Typed NotFound from `check_notebook` means the
            // pinned diagnostic matched the genuine notebook-
            // absence shape (`Notebook not found: <name>`).
            // Try to create it; if creation is disabled, surface
            // a typed validation error; if creation fails,
            // propagate the new CommandFailed verbatim.
            Err(NbError::NotFound { .. }) => {
                if !self.create_notebook {
                    return Err(NbError::ValidationError {
                        reason: format!(
                            "notebook not found; create it with the nb CLI (`nb notebooks add {}`) \
                             or remove --no-create-notebook",
                            notebook
                        ),
                        location: None,
                    });
                }
                self.exec_vec(vec![
                    "notebooks".to_string(),
                    "add".to_string(),
                    notebook.to_string(),
                ])
                .await?;
                Ok(())
            }
            // Any other error from `check_notebook` (permission
            // denied, transient crash, etc.) propagates verbatim;
            // we deliberately do not try `notebooks add`, because
            // a non-NotFound CommandFailed likely indicates a
            // real failure that creation would not rescue.
            Err(err) => Err(err),
        }
    }

    pub(super) async fn ensure_existing_notebook(&self, notebook: &str) -> Result<(), NbError> {
        match self.check_notebook(notebook).await {
            Ok(()) => Ok(()),
            // Infrastructure failure — surface verbatim.
            Err(err @ (NbError::ExecutableNotFound { .. } | NbError::Io { .. })) => Err(err),
            // Genuine notebook absence: propagate the typed
            // `NbError::NotFound` produced by `check_notebook`
            // verbatim. The qualified-selector path
            // (`<notebook>:<item>`) used by `show_note`,
            // `add_note`, `edit_note`, etc. surfaces this to the
            // caller as the typed `NotFound` variant — the
            // caller can distinguish "notebook gone" from a
            // generic validation failure. Erasing the typed
            // `NotFound` into a `ValidationError` would lose
            // the variant distinction.
            Err(err @ NbError::NotFound { .. }) => Err(err),
            // Other CommandFailed errors propagate verbatim
            // rather than being misclassified as "not found."
            Err(err) => Err(err),
        }
    }

    async fn check_notebook(&self, notebook: &str) -> Result<(), NbError> {
        let show_result = self
            .exec_vec(vec![
                "notebooks".to_string(),
                "show".to_string(),
                notebook.to_string(),
                "--path".to_string(),
            ])
            .await;
        match show_result {
            Ok(output) => {
                if output.trim().is_empty() {
                    // Use the actual argument vector that was passed
                    // to `exec_vec`, not a reformatted display
                    // string. This way the `command` field matches
                    // what was actually executed on the wire, with
                    // no possibility of drift between the synthetic
                    // field and the real argv.
                    return Err(NbError::CommandFailed {
                        command: format!("nb notebooks show {notebook} --path"),
                        stderr: "nb notebooks path output was empty".to_string(),
                        exit_code: None,
                    });
                }
                Ok(())
            }
            Err(err) => {
                // Surface genuine infrastructure failures verbatim
                // so callers can distinguish "nb is broken" from
                // "notebook does not exist". Genuine notebook
                // absence (nb's pinned diagnostic on stderr) maps
                // to typed `NotFound`. Any other subprocess
                // failure (e.g., permission denied, a transient
                // crash, an nb bug surfacing as `NotFound` in
                // stderr without the literal `Notebook not found:`
                // prefix) propagates as the original
                // `CommandFailed` rather than being swallowed
                // here. The caller (`ensure_notebook` /
                // `ensure_existing_notebook`) decides whether to
                // try to create the notebook on `NotFound` or
                // surface the error verbatim otherwise.
                match err {
                    NbError::ExecutableNotFound { .. } | NbError::Io { .. } => Err(err),
                    NbError::CommandFailed { ref stderr, .. }
                        if is_notebook_not_found(stderr, notebook) =>
                    {
                        Err(NbError::NotFound {
                            selector: format!("{notebook}:"),
                        })
                    }
                    other => Err(other),
                }
            }
        }
    }

    /// Executes an nb command and returns stdout.
    pub(super) async fn exec(&self, args: &[&str]) -> Result<String, NbError> {
        tracing::debug!(?args, "executing nb command");
        // `nb_program` resolves `nb` to a spawnable absolute path on
        // Windows (PATHEXT-aware: Rust std only finds `.exe` there and
        // would miss a `nb.cmd` launcher). Unix keeps PATH resolution.
        let mut command = Command::new(nb_program());
        // Strip inherited `GIT_*` routing vars before chaining `.args` /
        // `.env`. Without this, any caller invoking us from inside a
        // git hook (pre-commit, pre-push, post-checkout) or CI runner
        // propagates GIT_DIR / GIT_INDEX_FILE / GIT_COMMON_DIR /
        // GIT_WORK_TREE / GIT_OBJECT_DIRECTORY /
        // GIT_ALTERNATE_OBJECT_DIRECTORIES into the spawned `nb`,
        // which is a bash script wrapping git — every git call inside
        // nb then redirects to the parent repo instead of the
        // notebook's repo. The blast-by-prefix also covers future
        // GIT_* redirect vars without requiring a code change.
        // See `nb-api:issues/3`. Do not remove.
        scrub_git_env(&mut command);
        command
            .args(args)
            .stdin(Stdio::null()) // Prevent TTY hangs
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if self.disable_git_signing {
            apply_git_signing_env(&mut command);
        }
        let joined = format!("nb {}", args.join(" "));
        let output = command
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    NbError::ExecutableNotFound {
                        path: "nb".to_string(),
                    }
                } else {
                    NbError::Io {
                        path: PathBuf::from("nb"),
                        source: e.into(),
                    }
                }
            })?
            .wait_with_output()
            .await
            .map_err(|e| NbError::Io {
                path: PathBuf::from("nb"),
                source: e.into(),
            })?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(strip_ansi(&stdout))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            // nb sometimes writes errors to stdout
            let stderr_text = if stderr.is_empty() {
                strip_ansi(&stdout)
            } else {
                strip_ansi(&stderr)
            };
            Err(NbError::CommandFailed {
                command: joined,
                stderr: stderr_text,
                exit_code: output.status.code(),
            })
        }
    }

    /// Executes an nb command with dynamic arguments.
    pub(super) async fn exec_vec(&self, args: Vec<String>) -> Result<String, NbError> {
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.exec(&args_ref).await
    }
}
