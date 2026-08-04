//! [`NbClient`] method implementations.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::LazyLock;

use regex::Regex;
use tokio::process::Command;

use crate::argv::{
    child_folder_names, edit_args, empty_tasks_message, is_empty_tasks_error, mkdir_selector,
    normalize_folder, search_command_args, task_command_args, tasks_command_args, tasks_scope,
    todo_command_args,
};
use crate::diagnostics::{append_warning, is_notebook_not_found, is_selector_not_found};
use crate::error::NbError;
use crate::git::derive_git_notebook_name;
use crate::git_env::scrub_git_env;
use crate::git_signing::apply_git_signing_env;
use crate::output::strip_empty_result_hint;
use crate::validate::{
    detect_duplicate_title_heading, parse_qualified_selector, validate_destination,
    validate_folder_option, validate_folder_path, validate_notebook_name,
};
use crate::{Config, EditMode, NbClient, SearchMode, TaskStatus};

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
enum ShowClassification {
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
        })
    }

    fn require_folder_for_new_note(&self, folder: Option<&str>) -> Result<(), NbError> {
        if self.allow_top_level_notes || folder.is_some_and(|value| !value.trim().is_empty()) {
            return Ok(());
        }
        Err(NbError::ValidationError {
            reason: FOLDER_REQUIRED_MESSAGE.to_string(),
            location: None,
        })
    }

    async fn resolve_target_selector(
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

    fn append_notebook_warning(&self, output: String, notebook: &str) -> String {
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
    fn resolve_notebook_name(&self, notebook: Option<&str>) -> Result<String, NbError> {
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

    async fn resolve_notebook(&self, notebook: Option<&str>) -> Result<String, NbError> {
        let name = self.resolve_notebook_name(notebook)?;
        self.ensure_notebook(&name).await?;
        Ok(name)
    }

    async fn ensure_notebook(&self, notebook: &str) -> Result<(), NbError> {
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

    async fn ensure_existing_notebook(&self, notebook: &str) -> Result<(), NbError> {
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
    async fn exec(&self, args: &[&str]) -> Result<String, NbError> {
        tracing::debug!(?args, "executing nb command");
        let mut command = Command::new("nb");
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
    async fn exec_vec(&self, args: Vec<String>) -> Result<String, NbError> {
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.exec(&args_ref).await
    }

    /// Returns status information about the resolved notebook.
    pub async fn show_notebook_status(&self, notebook: Option<&str>) -> Result<String, NbError> {
        let notebook = self.resolve_notebook(notebook).await?;
        self.exec_vec(vec![format!("{}:", notebook), "status".to_string()])
            .await
    }

    /// Lists available notebooks.
    pub async fn list_notebooks(&self) -> Result<String, NbError> {
        // Use --no-color to avoid ANSI escape codes
        self.exec(&["notebooks", "--no-color"]).await
    }

    /// Returns the path for a notebook.
    pub async fn show_notebook_path(&self, notebook: Option<&str>) -> Result<PathBuf, NbError> {
        let notebook = self.resolve_notebook(notebook).await?;
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
            // Synthesized CommandFailed.command must match the
            // argv actually executed by exec_vec, not a
            // reformatted display string. The executed argv is
            // `nb notebooks show {notebook} --path`.
            return Err(NbError::CommandFailed {
                command: format!("nb notebooks show {notebook} --path"),
                stderr: "nb notebooks path output was empty".to_string(),
                exit_code: None,
            });
        }
        Ok(PathBuf::from(path))
    }

    /// Creates a new note.
    pub async fn add_note(
        &self,
        title: Option<&str>,
        content: &str,
        tags: &[String],
        folder: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        // Reject duplicate-H1 BEFORE any subprocess invocation or
        // notebook side effect (including `resolve_notebook`).
        // The validation is a pure in-process check; no `nb`
        // side effect can result from the rejection. This is
        // the general principle: validate input before any
        // state-mutating call.
        if let Some(t) = title
            && let Some(heading) = detect_duplicate_title_heading(t, content)
        {
            return Err(NbError::DuplicateTitleHeading {
                title: t.to_string(),
                heading,
            });
        }
        let mut args = Vec::new();
        self.require_folder_for_new_note(folder)?;
        validate_folder_option(folder)?;

        let notebook = self.resolve_notebook(notebook).await?;
        let cmd = format!("{}:add", notebook);
        args.push(cmd);

        // Title (if provided)
        if let Some(t) = title {
            args.push("--title".to_string());
            args.push(t.to_string());
        }

        // Content via --content flag (avoids shell escaping issues)
        args.push("--content".to_string());
        args.push(content.to_string());

        // Tags (nb expects #hashtag format)
        for tag in tags {
            args.push("--tags".to_string());
            let tag_str = if tag.starts_with('#') {
                tag.clone()
            } else {
                format!("#{}", tag)
            };
            args.push(tag_str);
        }

        // Folder
        if let Some(f) = folder {
            args.push("--folder".to_string());
            args.push(f.to_string());
        }

        self.exec_vec(args)
            .await
            .map(|output| self.append_notebook_warning(output, &notebook))
    }

    /// Shows a note's content.
    pub async fn show_note(&self, id: &str, notebook: Option<&str>) -> Result<String, NbError> {
        let (_, selector) = self.resolve_target_selector(id, notebook).await?;
        // Probe the selector's classification before reading.
        // `nb show <selector> --type text` reports whether the
        // type is text (rc 0) or not (rc non-zero). If the type
        // is not text, a follow-up `nb show <selector> --type`
        // reports the actual_type for the error diagnostic. When
        // the probe cannot classify (selector not found, internal
        // error), fall through to the original show path so
        // existing missing-selector diagnostics are preserved.
        // The semantic check delegates "what is text" to `nb`
        // itself, ensuring forward compatibility as `nb` adds
        // new textual types.
        match self.probe_show_classification(&selector).await {
            ShowClassification::NonTextual { actual_type } => {
                return Err(NbError::UnsupportedShowTarget {
                    selector: selector.clone(),
                    actual_type,
                });
            }
            ShowClassification::Textual | ShowClassification::ProbeFailure => {
                // Proceed to content read (Textual) or fall
                // through to original show (ProbeFailure).
            }
        }
        // Pass `--print` so `nb show` writes stored bytes to stdout instead of
        // piping through the renderer/pager. The renderer path word-wraps at
        // ~80 columns when stdout is a pipe, silently corrupting any stored
        // line longer than that (e.g. JSON in change-meta notes, code blocks,
        // long URLs). `--print` returns the file verbatim. Do not remove.
        // See `nb-api:issues/2`.
        let args = vec![
            "show".to_string(),
            selector.clone(),
            "--print".to_string(),
            "--no-color".to_string(),
        ];
        match self.exec_vec(args).await {
            Ok(stdout) => Ok(stdout),
            Err(NbError::CommandFailed {
                command,
                stderr,
                exit_code,
            }) => {
                // Map genuine `nb` not-found diagnostics to typed
                // NotFound at this public selector boundary. Other
                // CommandFailed metadata is preserved (real subprocess
                // failures, infra issues). The `selector` field
                // carries the resolved selector verbatim (e.g.,
                // `home:does-not-exist`) — the original id the
                // caller passed, qualified against the resolved
                // notebook, with no decorative verb suffix.
                //
                // The exact-match classifier rejects appended
                // failures (e.g., a retry that succeeds after a
                // missing-selector error, or a foreign-line
                // diagnostic with a "not found" substring):
                // `nb`'s complete normalized diagnostic for THIS
                // selector must be `! Not found: <selector>` —
                // not merely start with the prefix.
                if is_selector_not_found(&stderr, &selector) {
                    Err(NbError::NotFound {
                        selector: selector.clone(),
                    })
                } else {
                    Err(NbError::CommandFailed {
                        command,
                        stderr,
                        exit_code,
                    })
                }
            }
            Err(err) => Err(err),
        }
    }

    /// Probe the textual classification of a selector via `nb`'s
    /// native `--type` mechanism.
    ///
    /// Two-step probe: first `nb show <selector> --type text` to
    /// ask `nb` whether the type is text. If yes, return
    /// [`ShowClassification::Textual`]. If no, follow up with
    /// `nb show <selector> --type` to recover the `actual_type`
    /// for the error diagnostic. If the follow-up also fails
    /// (selector not found, internal error), return
    /// [`ShowClassification::ProbeFailure`] so the caller can
    /// fall through to the original show path.
    async fn probe_show_classification(&self, selector: &str) -> ShowClassification {
        let textual = self
            .exec_vec(vec![
                "show".to_string(),
                selector.to_string(),
                "--type".to_string(),
                "text".to_string(),
                "--no-color".to_string(),
            ])
            .await;
        if textual.is_ok() {
            return ShowClassification::Textual;
        }
        match self
            .exec_vec(vec![
                "show".to_string(),
                selector.to_string(),
                "--type".to_string(),
                "--no-color".to_string(),
            ])
            .await
        {
            Ok(stdout) => {
                let trimmed = stdout.trim();
                if trimmed.is_empty() {
                    ShowClassification::ProbeFailure
                } else {
                    ShowClassification::NonTextual {
                        actual_type: trimmed.to_string(),
                    }
                }
            }
            Err(_) => ShowClassification::ProbeFailure,
        }
    }

    /// Lists notes in a notebook or folder.
    pub async fn list_notes(
        &self,
        folder: Option<&str>,
        tags: &[String],
        limit: Option<u32>,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        let mut args = Vec::new();
        validate_folder_option(folder)?;

        let notebook = self.resolve_notebook(notebook).await?;
        let cmd = match folder {
            Some(f) => format!("{}:{}/", notebook, f),
            None => format!("{}:", notebook),
        };

        args.push("list".to_string());
        args.push(cmd);

        // No color for parsing
        args.push("--no-color".to_string());

        // Limit
        if let Some(n) = limit {
            args.push("-n".to_string());
            args.push(n.to_string());
        }

        // Tags filter
        for tag in tags {
            args.push("--tags".to_string());
            let tag_str = if tag.starts_with('#') {
                tag.clone()
            } else {
                format!("#{}", tag)
            };
            args.push(tag_str);
        }

        // Strip the trailing usage/help hint block from empty
        // results (`0 items.` followed by `Add a note:`,
        // `Import a file:`, `Help information:`). Detection
        // keys off the empty-result signal per the
        // `output-behavior` specification. See `output.rs`
        // for the helper's contract.
        self.exec_vec(args)
            .await
            .map(|output| strip_empty_result_hint(&output))
    }

    /// Searches notes.
    pub async fn search_notes(
        &self,
        queries: &[String],
        mode: SearchMode,
        tags: &[String],
        folder: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        validate_folder_option(folder)?;
        if queries.is_empty() {
            return Err(NbError::ValidationError {
                reason: "at least one search query is required".to_string(),
                location: None,
            });
        }

        let notebook = self.resolve_notebook(notebook).await?;
        let scope = match folder {
            Some(f) => format!("{}:{}/", notebook, f),
            None => format!("{}:", notebook),
        };
        let args = search_command_args(scope, queries, mode, tags);
        self.exec_vec(args).await
    }

    /// Edits a note using the provided content mode.
    ///
    /// See [`EditMode`] for the vocabulary rationale (the variant
    /// previously named `Replace` is now `Overwrite` to remove the
    /// vocabulary trap at the root of `nb-api:issues/api/6`).
    ///
    /// Requiredness on the consumer side (e.g., the `mode` field on
    /// `nb-mcp-server`'s `EditArgs`) is a consumer-layer concern,
    /// not enforced here.
    pub async fn edit_note(
        &self,
        id: &str,
        content: &str,
        mode: EditMode,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        let (notebook, selector) = self.resolve_target_selector(id, notebook).await?;
        let output = self.exec_vec(edit_args(selector, content, mode)).await?;
        Ok(self.append_notebook_warning(output, &notebook))
    }

    /// Deletes a note.
    pub async fn delete_note(&self, id: &str, notebook: Option<&str>) -> Result<String, NbError> {
        let (notebook, selector) = self.resolve_target_selector(id, notebook).await?;
        let output = self
            .exec_vec(vec!["delete".to_string(), selector, "--force".to_string()])
            .await?;
        Ok(self.append_notebook_warning(output, &notebook))
    }

    /// Moves or renames a note.
    pub async fn move_note(
        &self,
        id: &str,
        destination: &str,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        validate_destination(destination)?;
        let (notebook, selector) = self.resolve_target_selector(id, notebook).await?;
        let output = self
            .exec_vec(vec![
                "move".to_string(),
                selector,
                destination.to_string(),
                "--force".to_string(),
            ])
            .await?;
        Ok(self.append_notebook_warning(output, &notebook))
    }

    /// Creates a todo item.
    pub async fn add_todo(
        &self,
        title: &str,
        description: Option<&str>,
        tasks: &[String],
        tags: &[String],
        folder: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        self.require_folder_for_new_note(folder)?;
        validate_folder_option(folder)?;
        let notebook = self.resolve_notebook(notebook).await?;
        let output = self
            .exec_vec(todo_command_args(
                &notebook,
                title,
                description,
                tasks,
                tags,
                folder,
            ))
            .await?;
        Ok(self.append_notebook_warning(output, &notebook))
    }

    /// Marks a todo as done.
    pub async fn mark_task_done(
        &self,
        id: &str,
        task_number: Option<u32>,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        let (notebook, selector) = self.resolve_target_selector(id, notebook).await?;
        let output = self
            .exec_vec(task_command_args("do", selector, task_number))
            .await?;
        Ok(self.append_notebook_warning(output, &notebook))
    }

    /// Marks a todo as not done.
    pub async fn unmark_task_done(
        &self,
        id: &str,
        task_number: Option<u32>,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        let (notebook, selector) = self.resolve_target_selector(id, notebook).await?;
        let output = self
            .exec_vec(task_command_args("undo", selector, task_number))
            .await?;
        Ok(self.append_notebook_warning(output, &notebook))
    }

    /// Lists checklist items within todos.
    ///
    /// Invokes the `nb tasks` subcommand. The method enumerates
    /// the checklist items within todos (and recursively into
    /// subfolders when `recursive = true`), filtered by
    /// `status` if provided. The method name matches the
    /// underlying `nb` CLI command (`nb tasks`); a future
    /// `list_todos` method for the todo **container** listing
    /// (invoking `nb todos`) is tracked at
    /// `nb-api:todos/api/5` (deferred to `0.3.0+`).
    pub async fn list_tasks(
        &self,
        folder: Option<&str>,
        status: Option<TaskStatus>,
        recursive: bool,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        validate_folder_option(folder)?;
        let notebook = self.resolve_notebook(notebook).await?;
        let folder = folder.map(normalize_folder);
        let scopes = if recursive {
            self.tasks_scopes_recursive(&notebook, folder.as_deref())
                .await?
        } else {
            vec![tasks_scope(&notebook, folder.as_deref())]
        };

        let mut outputs: Vec<String> = Vec::new();
        let mut saw_empty = false;
        for scope in scopes {
            match self.exec_vec(tasks_command_args(scope, status)).await {
                Ok(output) => {
                    let output = output.trim();
                    if !output.is_empty() {
                        outputs.push(output.to_string());
                    }
                }
                Err(NbError::CommandFailed { stderr, .. }) if is_empty_tasks_error(&stderr) => {
                    saw_empty = true;
                }
                Err(err) => return Err(err),
            }
        }
        if outputs.is_empty() && saw_empty {
            // The `nb` subprocess actually succeeded (exit 0);
            // it simply returned no tasks. This is a policy-level
            // empty-result, not a command failure. Map to
            // ValidationError rather than fabricate a
            // CommandFailed with `exit_code: Some(0)` (which
            // would be factually misleading).
            return Err(NbError::ValidationError {
                reason: empty_tasks_message(status),
                location: None,
            });
        }
        Ok(outputs.join("\n"))
    }

    async fn tasks_scopes_recursive(
        &self,
        notebook: &str,
        folder: Option<&str>,
    ) -> Result<Vec<String>, NbError> {
        let notebook_root = self.show_notebook_path(Some(notebook)).await?;
        let start = folder.unwrap_or_default().to_string();
        let mut queue = VecDeque::new();
        queue.push_back(start.clone());

        let mut scopes = vec![tasks_scope(notebook, folder)];
        while let Some(current) = queue.pop_front() {
            let base = if current.is_empty() {
                notebook_root.clone()
            } else {
                notebook_root.join(&current)
            };
            let children = child_folder_names(&base)?;
            for child in children {
                let next = if current.is_empty() {
                    child
                } else {
                    format!("{}/{}", current, child)
                };
                scopes.push(tasks_scope(notebook, Some(&next)));
                queue.push_back(next);
            }
        }
        Ok(scopes)
    }

    /// Creates a bookmark.
    pub async fn add_bookmark(
        &self,
        url: &str,
        title: Option<&str>,
        tags: &[String],
        comment: Option<&str>,
        folder: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        let mut args = Vec::new();
        self.require_folder_for_new_note(folder)?;
        validate_folder_option(folder)?;

        // Build the destination path with optional folder
        let notebook = self.resolve_notebook(notebook).await?;
        let dest = match folder {
            Some(f) => format!("{}:{}/", notebook, f),
            None => format!("{}:", notebook),
        };

        let cmd = format!("{}bookmark", dest);
        args.push(cmd);
        args.push(url.to_string());

        if let Some(t) = title {
            args.push("--title".to_string());
            args.push(t.to_string());
        }

        if let Some(c) = comment {
            args.push("--comment".to_string());
            args.push(c.to_string());
        }

        for tag in tags {
            args.push("--tags".to_string());
            let tag_str = if tag.starts_with('#') {
                tag.clone()
            } else {
                format!("#{}", tag)
            };
            args.push(tag_str);
        }

        self.exec_vec(args)
            .await
            .map(|output| self.append_notebook_warning(output, &notebook))
    }

    /// Lists folders in a notebook.
    pub async fn list_folders(
        &self,
        parent: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        let mut args = vec!["list".to_string()];
        validate_folder_option(parent)?;

        let notebook = self.resolve_notebook(notebook).await?;
        let path = match parent {
            Some(p) => format!("{}:{}/", notebook, p),
            None => format!("{}:", notebook),
        };
        args.push(path);

        // Filter to only show folders
        args.push("--type".to_string());
        args.push("folder".to_string());
        args.push("--no-color".to_string());

        // Strip the trailing usage/help hint block from empty
        // results (`0 folders.` followed by `Import a file:`,
        // `Help information:`). Detection keys off the
        // empty-result signal per the `output-behavior`
        // specification. See `output.rs` for the helper's
        // contract.
        self.exec_vec(args)
            .await
            .map(|output| strip_empty_result_hint(&output))
    }

    /// Creates a folder.
    pub async fn add_folder(&self, path: &str, notebook: Option<&str>) -> Result<String, NbError> {
        validate_folder_path(path)?;
        let notebook = self.resolve_notebook(notebook).await?;
        let folder_path = mkdir_selector(&notebook, path);
        let output = self
            .exec_vec(vec!["add".to_string(), "folder".to_string(), folder_path])
            .await?;
        Ok(self.append_notebook_warning(output, &notebook))
    }

    /// Imports a file or URL into the notebook as a note.
    ///
    /// Invokes `nb import`, which only handles notes (HTML,
    /// Markdown, plain text, and other source formats that
    /// `nb` can convert into a note body). The `_note` suffix
    /// is correct because `nb import` cannot create bookmarks
    /// or folders — those use `add_bookmark` and `add_folder`
    /// respectively. The `source` may be a local file path or
    /// a URL; HTML sources may be converted to Markdown via
    /// `convert = true`.
    pub async fn import_note(
        &self,
        source: &str,
        folder: Option<&str>,
        filename: Option<&str>,
        convert: bool,
        notebook: Option<&str>,
    ) -> Result<String, NbError> {
        let mut args = Vec::new();
        self.require_folder_for_new_note(folder)?;
        validate_folder_option(folder)?;

        let notebook = self.resolve_notebook(notebook).await?;
        let cmd = format!("{}:import", notebook);
        args.push(cmd);

        // Source path or URL
        args.push(source.to_string());

        // Convert HTML to Markdown
        if convert {
            args.push("--convert".to_string());
        }

        // Destination: notebook:folder/filename or just folder/filename
        // nb import expects destination as a positional argument after source
        if folder.is_some() || filename.is_some() {
            let dest = match (folder, filename) {
                (Some(f), Some(n)) => format!("{}/{}", f, n),
                (Some(f), None) => format!("{}/", f),
                (None, Some(n)) => n.to_string(),
                (None, None) => unreachable!(),
            };
            args.push(dest);
        }

        self.exec_vec(args)
            .await
            .map(|output| self.append_notebook_warning(output, &notebook))
    }
}
