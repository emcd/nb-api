//! [`NbClient`] method implementations.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;

use regex::Regex;
use tokio::process::Command;

use crate::argv::{
    child_folder_names, empty_tasks_message, is_empty_tasks_error, normalize_folder,
    search_command_args, tasks_command_args, tasks_scope,
};
use crate::diagnostics::{append_warning, is_notebook_not_found, is_selector_not_found};
use crate::error::NbError;
use crate::fingerprint;
use crate::fingerprint::Fingerprint;
use crate::gate;
use crate::git::derive_git_notebook_name;
use crate::git_env::scrub_git_env;
use crate::git_signing::apply_git_signing_env;
use crate::lines::{
    note_line_from_body_line, require_contiguous_body, search_lines, split_body_lines,
};
use crate::output::strip_empty_result_hint;
use crate::parser::{ParseContext, parse};
use crate::transaction::{self, Transaction};
use crate::types::{
    BodyFragment, ByteString, CommitOutcome, LineEdit, NoteTarget, Occurrence, SearchNoteLines,
    ShowNote, ShowNoteLines,
};
use crate::validate::{
    detect_duplicate_title_heading, parse_qualified_selector, validate_destination,
    validate_folder_option, validate_folder_path, validate_notebook_name,
};
use crate::{Config, NbClient, SearchMode, TaskStatus};

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
        Ok(Transaction::new(self.clone(), notebook))
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

    async fn with_notebook_gate<F, T>(&self, notebook: &str, f: F) -> Result<T, NbError>
    where
        F: std::future::Future<Output = Result<T, NbError>>,
    {
        let root = self.show_notebook_path_unguarded(Some(notebook)).await?;
        let key = gate::git_common_dir_realpath(&root)?;
        let _hold = gate::acquire_notebook(key, self.gate_timeout, false).await?;
        f.await
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
        let _g = gate::acquire_global(self.gate_timeout).await?;
        // Use --no-color to avoid ANSI escape codes
        self.exec(&["notebooks", "--no-color"]).await
    }

    /// Returns the path for a notebook.
    pub async fn show_notebook_path(&self, notebook: Option<&str>) -> Result<PathBuf, NbError> {
        let notebook = self.resolve_notebook_name(notebook)?;
        self.ensure_notebook(&notebook).await?;
        self.with_notebook_gate(
            &notebook,
            self.show_notebook_path_unguarded(Some(&notebook)),
        )
        .await
    }

    /// Creates a new note (one-shot transaction; optional auto-name).
    pub async fn add_note(
        &self,
        title: Option<&str>,
        content: &str,
        tags: &[String],
        folder: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        if let Some(t) = title
            && let Some(heading) = detect_duplicate_title_heading(t, content)
        {
            return Err(NbError::DuplicateTitleHeading {
                title: t.to_string(),
                heading,
            });
        }
        self.require_folder_for_new_note(folder)?;
        validate_folder_option(folder)?;
        let filename = transaction::auto_filename("md");
        let path = transaction::join_folder_file(folder, &filename);
        let mut tx = self.transaction(notebook).await?;
        tx.add_note(&path, title, content, tags)?;
        tx.commit().await
    }

    /// Shows a note as a structured [`ShowNote`] (gate-held).
    pub async fn show_note(&self, id: &str, notebook: Option<&str>) -> Result<ShowNote, NbError> {
        let (notebook, selector) = self.resolve_target_selector(id, notebook).await?;
        self.with_notebook_gate(&notebook, async {
            self.show_note_inner(&notebook, &selector).await
        })
        .await
    }

    async fn show_note_inner(&self, notebook: &str, selector: &str) -> Result<ShowNote, NbError> {
        match self.probe_show_classification(selector).await {
            ShowClassification::NonTextual { actual_type } => {
                return Err(NbError::UnsupportedShowTarget {
                    selector: selector.to_string(),
                    actual_type,
                });
            }
            ShowClassification::Textual | ShowClassification::ProbeFailure => {}
        }
        let path = self.resolve_item_path(selector).await?;
        let root = self.show_notebook_path_unguarded(Some(notebook)).await?;
        let rel = path_relative_to(&root, &path)?;
        let source = std::fs::read(&path).map_err(|e| NbError::Io {
            path: path.clone(),
            source: e.into(),
        })?;
        let doc = match parse(&source, ParseContext::FromPath(PathBuf::from(&rel))) {
            Ok(doc) => doc,
            // nb may classify additional textual extensions as showable; map
            // unrecognized formats to a Note partition for structured show.
            Err(NbError::UnsupportedDocumentFormat { .. }) => {
                parse(&source, ParseContext::Explicit(crate::DocumentKind::Note))?
            }
            Err(err) => return Err(err),
        };
        let fragments: Vec<BodyFragment> = doc
            .body()
            .enumerate()
            .map(|(i, bytes)| BodyFragment {
                index: i as u32,
                bytes: ByteString::from_bytes(bytes),
            })
            .collect();
        let body_contiguous = fragments.len() <= 1;
        let body_bytes = doc.body_bytes();
        let tags: Vec<String> = doc
            .tags_str()
            .filter_map(|t| t.ok().map(|s| s.trim_start_matches('#').to_string()))
            .collect();
        let title = doc.title().map(ByteString::from_bytes);
        let title_text = doc.title_str().and_then(|r| {
            r.ok().map(|s| {
                s.trim_end_matches('\n')
                    .trim_start_matches('#')
                    .trim()
                    .to_string()
            })
        });
        Ok(ShowNote {
            selector: selector.to_string(),
            path: rel,
            kind: doc.kind(),
            todo_state: doc.todo_state(),
            title,
            title_text,
            tags,
            body_fragments: fragments,
            body_contiguous,
            body: ByteString::from_bytes(body_bytes),
            fingerprint: fingerprint::fingerprint(&doc),
            source: ByteString::from_bytes(source),
        })
    }

    /// Enumerate body lines for a contiguous-body note.
    pub async fn show_note_lines(
        &self,
        target: NoteTarget,
        offset: Option<u32>,
        limit: Option<u32>,
        notebook: Option<&str>,
    ) -> Result<ShowNoteLines, NbError> {
        let (notebook, selector) = self.resolve_note_target(&target, notebook).await?;
        self.with_notebook_gate(&notebook, async {
            let shown = self.show_note_inner(&notebook, &selector).await?;
            let source = shown.source.as_bytes()?;
            let doc = parse(&source, ParseContext::FromPath(PathBuf::from(&shown.path)))?;
            let body = require_contiguous_body(&doc)?;
            let lines = split_body_lines(&body);
            let total_lines = lines.len() as u32;
            let offset = offset.unwrap_or(1);
            let limit = limit.unwrap_or(100);
            if offset == 0 {
                return Err(NbError::InvalidLineWindow {
                    offset,
                    limit,
                    total_lines,
                });
            }
            if total_lines == 0 {
                if offset != 1 {
                    return Err(NbError::InvalidLineWindow {
                        offset,
                        limit,
                        total_lines,
                    });
                }
                return Ok(ShowNoteLines {
                    selector: shown.selector,
                    path: shown.path,
                    kind: shown.kind,
                    total_lines: 0,
                    offset,
                    limit,
                    next_offset: None,
                    lines: Vec::new(),
                    title: shown.title,
                    tags: shown.tags,
                    body_fingerprint: shown.fingerprint,
                });
            }
            if offset > total_lines + 1 {
                return Err(NbError::InvalidLineWindow {
                    offset,
                    limit,
                    total_lines,
                });
            }
            let start_idx = (offset - 1) as usize;
            let end_idx = (start_idx + limit as usize).min(lines.len());
            let window: Vec<_> = lines[start_idx..end_idx]
                .iter()
                .map(|l| note_line_from_body_line(l, &body))
                .collect();
            let next_offset = if end_idx < lines.len() {
                Some(offset + window.len() as u32)
            } else {
                None
            };
            Ok(ShowNoteLines {
                selector: shown.selector,
                path: shown.path,
                kind: shown.kind,
                total_lines,
                offset,
                limit,
                next_offset,
                lines: window,
                title: shown.title,
                tags: shown.tags,
                body_fingerprint: shown.fingerprint,
            })
        })
        .await
    }

    /// Search body line texts for a byte pattern (contiguous body only).
    pub async fn search_note_lines(
        &self,
        target: NoteTarget,
        pattern: &[u8],
        notebook: Option<&str>,
    ) -> Result<SearchNoteLines, NbError> {
        let (notebook, selector) = self.resolve_note_target(&target, notebook).await?;
        self.with_notebook_gate(&notebook, async {
            let shown = self.show_note_inner(&notebook, &selector).await?;
            let source = shown.source.as_bytes()?;
            let doc = parse(&source, ParseContext::FromPath(PathBuf::from(&shown.path)))?;
            let body = require_contiguous_body(&doc)?;
            let hits = search_lines(&body, pattern)?;
            Ok(SearchNoteLines {
                selector: shown.selector,
                path: shown.path,
                kind: shown.kind,
                hits,
                body_fingerprint: shown.fingerprint,
            })
        })
        .await
    }

    async fn resolve_item_path(&self, selector: &str) -> Result<PathBuf, NbError> {
        let output = self
            .exec_vec(vec![
                "show".to_string(),
                selector.to_string(),
                "--path".to_string(),
                "--no-color".to_string(),
            ])
            .await
            .map_err(|err| match err {
                NbError::CommandFailed { stderr, .. }
                    if is_selector_not_found(&stderr, selector) =>
                {
                    NbError::NotFound {
                        selector: selector.to_string(),
                    }
                }
                other => other,
            })?;
        let path = output.trim();
        if path.is_empty() {
            return Err(NbError::CommandFailed {
                command: format!("nb show {selector} --path"),
                stderr: "empty path".into(),
                exit_code: None,
            });
        }
        Ok(PathBuf::from(path))
    }

    async fn resolve_note_target(
        &self,
        target: &NoteTarget,
        notebook: Option<&str>,
    ) -> Result<(String, String), NbError> {
        match target {
            NoteTarget::Selector { value } => self.resolve_target_selector(value, notebook).await,
            NoteTarget::Path { value } => {
                let notebook = self.resolve_notebook(notebook).await?;
                Ok((notebook.clone(), format!("{notebook}:{value}")))
            }
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

    /// Deletes a note (one-shot transaction).
    pub async fn delete_note(
        &self,
        id: &str,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let (nb, selector) = self.resolve_target_selector(id, notebook).await?;
        let target = self.note_target_for_selector(&nb, &selector).await?;
        let mut tx = self.transaction(Some(&nb)).await?;
        tx.delete_note(target)?;
        tx.commit().await
    }

    /// Moves or renames a note (one-shot transaction).
    pub async fn move_note(
        &self,
        id: &str,
        destination: &str,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        validate_destination(destination)?;
        let (nb, selector) = self.resolve_target_selector(id, notebook).await?;
        let target = self.note_target_for_selector(&nb, &selector).await?;
        let mut tx = self.transaction(Some(&nb)).await?;
        tx.move_note(target, destination)?;
        tx.commit().await
    }

    async fn note_target_for_selector(
        &self,
        notebook: &str,
        selector: &str,
    ) -> Result<NoteTarget, NbError> {
        let root = self.show_notebook_path_unguarded(Some(notebook)).await?;
        let stripped = selector
            .rsplit_once(':')
            .map(|(_, rest)| rest)
            .unwrap_or(selector);
        let direct = root.join(stripped);
        if direct.is_file() {
            return Ok(NoteTarget::path(stripped));
        }
        // Fall back to `nb show --path` for numeric ids / titles.
        let abs = self.resolve_item_path(selector).await?;
        let rel = path_relative_to(&root, &abs)?;
        Ok(NoteTarget::path(rel))
    }

    /// Creates a todo item (one-shot transaction; optional auto-name).
    pub async fn add_todo(
        &self,
        title: &str,
        description: Option<&str>,
        tasks: &[String],
        tags: &[String],
        folder: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        self.require_folder_for_new_note(folder)?;
        validate_folder_option(folder)?;
        let filename = transaction::auto_filename("todo.md");
        let path = transaction::join_folder_file(folder, &filename);
        let mut tx = self.transaction(notebook).await?;
        tx.add_todo(&path, title, description, tasks, tags)?;
        tx.commit().await
    }

    /// Marks a todo as done (one-shot transaction).
    pub async fn mark_task_done(
        &self,
        id: &str,
        task_number: Option<u32>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let (nb, selector) = self.resolve_target_selector(id, notebook).await?;
        let target = self.note_target_for_selector(&nb, &selector).await?;
        let mut tx = self.transaction(Some(&nb)).await?;
        tx.mark_task_done(target, task_number)?;
        tx.commit().await
    }

    /// Marks a todo as not done (one-shot transaction).
    pub async fn unmark_task_done(
        &self,
        id: &str,
        task_number: Option<u32>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let (nb, selector) = self.resolve_target_selector(id, notebook).await?;
        let target = self.note_target_for_selector(&nb, &selector).await?;
        let mut tx = self.transaction(Some(&nb)).await?;
        tx.unmark_task_done(target, task_number)?;
        tx.commit().await
    }

    /// Replace contiguous body bytes (one-shot).
    pub async fn replace_note_body(
        &self,
        target: NoteTarget,
        new_body: impl AsRef<[u8]>,
        fingerprint: Fingerprint,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let mut tx = self.transaction(notebook).await?;
        tx.replace_note_body(target, new_body, fingerprint)?;
        tx.commit().await
    }

    /// Substring edit on contiguous body (one-shot).
    #[allow(clippy::too_many_arguments)]
    pub async fn edit_note_substring(
        &self,
        target: NoteTarget,
        pattern: impl AsRef<[u8]>,
        replacement: impl AsRef<[u8]>,
        occurrence: Occurrence,
        expected_count: u32,
        fingerprint: Option<Fingerprint>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let mut tx = self.transaction(notebook).await?;
        tx.edit_note_substring(
            target,
            pattern,
            replacement,
            occurrence,
            expected_count,
            fingerprint,
        )?;
        tx.commit().await
    }

    /// Line-oriented body edit batch (one-shot).
    pub async fn edit_note_lines(
        &self,
        target: NoteTarget,
        edits: Vec<LineEdit>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let mut tx = self.transaction(notebook).await?;
        tx.edit_note_lines(target, edits)?;
        tx.commit().await
    }

    /// Retitle a note without changing path (one-shot).
    pub async fn retitle_note(
        &self,
        target: NoteTarget,
        title: impl AsRef<[u8]>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let mut tx = self.transaction(notebook).await?;
        tx.retitle_note(target, title)?;
        tx.commit().await
    }

    /// Add/remove tags (one-shot).
    pub async fn edit_note_tags(
        &self,
        target: NoteTarget,
        add: &[String],
        remove: &[String],
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        let mut tx = self.transaction(notebook).await?;
        tx.edit_note_tags(target, add, remove)?;
        tx.commit().await
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

    /// Creates a bookmark (one-shot transaction; optional auto-name).
    pub async fn add_bookmark(
        &self,
        url: &str,
        title: Option<&str>,
        tags: &[String],
        comment: Option<&str>,
        folder: Option<&str>,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        self.require_folder_for_new_note(folder)?;
        validate_folder_option(folder)?;
        let filename = transaction::auto_filename("bookmark.md");
        let path = transaction::join_folder_file(folder, &filename);
        let mut tx = self.transaction(notebook).await?;
        tx.add_bookmark(&path, url, title, tags, comment)?;
        tx.commit().await
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

    /// Creates a folder (one-shot transaction).
    pub async fn add_folder(
        &self,
        path: &str,
        notebook: Option<&str>,
    ) -> Result<CommitOutcome, NbError> {
        validate_folder_path(path)?;
        let mut tx = self.transaction(notebook).await?;
        tx.add_folder(path)?;
        tx.commit().await
    }

    /// Imports a file or URL into the notebook as a note.
    ///
    /// One-shot only under the process-shared notebook gate. Not a
    /// [`Transaction`] plan op in 0.3.0.
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
        self.with_notebook_gate(&notebook, async {
            let cmd = format!("{}:import", notebook);
            args.push(cmd);
            args.push(source.to_string());
            if convert {
                args.push("--convert".to_string());
            }
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
        })
        .await
    }
}

fn notebook_dir_from_env(notebook: &str) -> Option<PathBuf> {
    let nb_dir = std::env::var_os("NB_DIR")?;
    let path = PathBuf::from(nb_dir).join(notebook);
    if path.is_dir() && path.join(".git").exists() {
        Some(path)
    } else {
        None
    }
}

fn path_relative_to(root: &Path, path: &Path) -> Result<String, NbError> {
    let root = root.canonicalize().map_err(|e| NbError::Io {
        path: root.to_path_buf(),
        source: e.into(),
    })?;
    let path = path.canonicalize().map_err(|e| NbError::Io {
        path: path.to_path_buf(),
        source: e.into(),
    })?;
    let rel = path
        .strip_prefix(&root)
        .map_err(|_| NbError::ValidationError {
            reason: format!(
                "path {} is not under notebook root {}",
                path.display(),
                root.display()
            ),
            location: None,
        })?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}
