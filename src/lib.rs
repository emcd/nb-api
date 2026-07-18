//! Typed Rust interface to the `nb` note-taking CLI.
//!
//! Handles notebook qualification, escaping, and output parsing.
//! Wraps the `nb` CLI as a subprocess, providing async methods for
//! all note-taking operations.

mod git;
mod git_env;
mod output;

#[cfg(feature = "testing")]
pub mod testing;

pub use git::{derive_git_notebook_name, git_rev_parse};
pub use git_env::{leaked_git_names, scrub_git_env, scrub_git_env_std};

use std::{collections::VecDeque, path::PathBuf, process::Stdio, sync::LazyLock};

use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

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

/// Errors from nb CLI invocation.
#[derive(Debug, thiserror::Error)]
pub enum NbError {
    #[error("nb command failed: {0}")]
    CommandFailed(String),

    #[error(
        "nb not found in PATH; install via: brew install xwmx/taps/nb (macOS) or see https://github.com/xwmx/nb#installation"
    )]
    NotFound,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// `nb show` was invoked on a selector whose type is not
    /// classified as text by `nb` itself (per
    /// `add-0-2-0-foundation` public-API spec). Folders, archives,
    /// audio, video, image, and any other non-textual type reach
    /// this path. The classification delegates to
    /// `nb show <selector> --type text` so forward compatibility
    /// is automatic when `nb` adds new textual types. Probe
    /// failure (e.g., selector not found) does NOT route here;
    /// it falls through to the original `CommandFailed` error
    /// from the content-read path.
    #[error(
        "selector `{selector}` resolved to non-textual type `{actual_type}`; \
         `nb show` does not display non-textual content"
    )]
    UnsupportedShowTarget {
        selector: String,
        actual_type: String,
    },

    /// `nb add` was called with a `title` and `content` where the
    /// first nonblank line of `content` is an exact Markdown ATX H1
    /// duplicating the title. The validation runs in the caller
    /// process before any subprocess invocation or notebook side
    /// effect (including `resolve_notebook`); the rejection
    /// happens entirely in-process. `heading` carries the exact
    /// detected source line (including the leading `#` and any
    /// surrounding whitespace) for actionable diagnostics. See
    /// `add-0-2-0-foundation` public-API specification.
    #[error(
        "title `{title}` duplicates the first H1 in content (`{heading}`); \
         remove the duplicate heading to avoid double-rendering"
    )]
    DuplicateTitleHeading { title: String, heading: String },
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

/// Configuration for constructing an [`NbClient`].
///
/// Contains only nb-relevant fields. MCP-specific fields
/// (e.g., `show_paths`) remain in the server's config.
#[derive(Clone, Debug)]
pub struct Config {
    /// Default notebook name (overrides Git-derived fallback).
    pub notebook: Option<String>,
    /// Automatically create missing notebooks.
    pub create_notebook: bool,
    /// Allow new notes to be created at notebook root.
    pub allow_top_level_notes: bool,
    /// Disable Git commit and tag signing for `nb` subprocesses.
    pub disable_git_signing: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            notebook: None,
            create_notebook: true,
            allow_top_level_notes: false,
            disable_git_signing: false,
        }
    }
}

/// Client for invoking nb commands.
#[derive(Clone)]
pub struct NbClient {
    /// Default notebook to use if not specified per-command.
    default_notebook: Option<String>,
    /// Automatically create missing notebooks.
    create_notebook: bool,
    /// Disable Git commit and tag signing for `nb` subprocesses.
    disable_git_signing: bool,
    /// Allow new notes to be created at notebook root.
    allow_top_level_notes: bool,
}

const FOLDER_REQUIRED_MESSAGE: &str = "This server is configured to require `folder` for new notes. Use the `nb.mkdir` tool to create new folders and the `nb.folders` tool to list existing folders.";

const NOTEBOOK_FIELD_MESSAGE: &str = "Invalid `notebook`: use a bare notebook name only. Use `folder` for folder paths and `id`/`selector` for note selectors.";

const FOLDER_FIELD_MESSAGE: &str = "Invalid folder path: use `folder` for folder paths only, not notebook-qualified selectors. To choose a notebook, use the separate `notebook` field.";

/// Behavior mode for [`NbClient::edit_note`] content updates.
///
/// ## Vocabulary
///
/// The variant previously named `Replace` is now `Overwrite` to
/// remove the vocabulary trap at the root of `nb-api:issues/api/6`:
/// callers reading `mode: "replace"` reasonably expected a
/// substring-style replacement (analogous to
/// [`str::replace`]), but `nb edit --overwrite`
/// is destructive — it replaces every byte of the note body.
/// Renaming the variant to `Overwrite` makes the destructive
/// intent unambiguous at the call site.
///
/// The legacy string `"replace"` is accepted as a serde alias for
/// backward compatibility with payloads produced before this
/// rename. The alias is **not** advertised in the derived
/// [`schemars`](https://docs.rs/schemars) JSON Schema — only the
/// canonical `"overwrite"` is exposed to MCP tool consumers.
///
/// ## Mapping
///
/// | Variant | Canonical serialization | `nb edit` flag(s) | Effect |
/// |---------|------------------------|-------------------|--------|
/// | [`EditMode::Overwrite`] | `"overwrite"` | `--overwrite --content <content>` | Replace **every byte** of the note body with `<content>`. Destructive: any existing content is lost. |
/// | [`EditMode::Append`] | `"append"` | `--content <content>` | Append `<content>` after the existing note body. |
/// | [`EditMode::Prepend`] | `"prepend"` | `--prepend --content <content>` | Prepend `<content>` before the existing note body. |
///
/// ## Default
///
/// `EditMode` derives `Default` with `EditMode::Overwrite` as the
/// default variant. This is the **`nb-api` default**, chosen for
/// compatibility with the current API contract and the documented
/// destructive default on `nb_api`'s edit API. Note that this is
/// distinct from the `nb` CLI's native no-flag behavior (`nb edit
/// --content` without `--overwrite` appends; see the mapping
/// table above). Requiredness on the consumer side (e.g., the
/// `mode` field on `nb-mcp-server`'s `EditArgs`) is a
/// **consumer-layer concern**, not enforced here. Downstream
/// consumers that want to require `mode` explicitly should drop
/// `#[serde(default)]` from their containing struct.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum EditMode {
    /// Replace **every byte** of the note body with the provided
    /// content. Destructive: any existing content is lost. Maps to
    /// `nb edit --overwrite --content <content>`.
    ///
    /// Accepts legacy `"replace"` as a serde alias for backward
    /// compatibility with payloads produced before the variant
    /// rename. The alias is not advertised in the derived JSON
    /// Schema; only `"overwrite"` is exposed to schema consumers.
    #[default]
    #[serde(alias = "replace")]
    Overwrite,
    /// Append the provided content after the existing note body.
    /// Maps to `nb edit --content <content>` (the `nb` default
    /// content-mode behavior).
    Append,
    /// Prepend the provided content before the existing note body.
    /// Maps to `nb edit --prepend --content <content>`.
    Prepend,
}

/// Matching mode for `nb search` query terms.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Match any query term (`OR` semantics).
    #[default]
    Any,
    /// Require all query terms (`AND` semantics).
    All,
}

/// Status filter for `nb tasks`.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Return open tasks.
    Open,
    /// Return closed tasks.
    Closed,
}

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
        Err(NbError::CommandFailed(FOLDER_REQUIRED_MESSAGE.to_string()))
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
                        return Err(NbError::CommandFailed(format!(
                            "ambiguous selector: id targets notebook `{embedded_notebook}`, but notebook field is `{value}`"
                        )));
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
        Err(NbError::CommandFailed(
            "notebook not configured; set --notebook or NB_MCP_NOTEBOOK".to_string(),
        ))
    }

    async fn resolve_notebook(&self, notebook: Option<&str>) -> Result<String, NbError> {
        let name = self.resolve_notebook_name(notebook)?;
        self.ensure_notebook(&name).await?;
        Ok(name)
    }

    async fn ensure_notebook(&self, notebook: &str) -> Result<(), NbError> {
        match self.check_notebook(notebook).await {
            Ok(()) => Ok(()),
            Err(_) => {
                if !self.create_notebook {
                    return Err(NbError::CommandFailed(format!(
                        "notebook not found; create it with the nb CLI (`nb notebooks add {}`) \
                         or remove --no-create-notebook",
                        notebook
                    )));
                }
                self.exec_vec(vec![
                    "notebooks".to_string(),
                    "add".to_string(),
                    notebook.to_string(),
                ])
                .await?;
                Ok(())
            }
        }
    }

    async fn ensure_existing_notebook(&self, notebook: &str) -> Result<(), NbError> {
        self.check_notebook(notebook).await.map_err(|_| {
            NbError::CommandFailed(format!(
                "notebook not found: `{notebook}`. Use a copied selector only for an existing notebook."
            ))
        })
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
                    return Err(NbError::CommandFailed(
                        "nb notebooks path output was empty".to_string(),
                    ));
                }
                Ok(())
            }
            Err(_) => Err(NbError::CommandFailed(format!(
                "notebook not found: `{notebook}`"
            ))),
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
        let output = command
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    NbError::NotFound
                } else {
                    NbError::Io(e)
                }
            })?
            .wait_with_output()
            .await?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(strip_ansi(&stdout))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            // nb sometimes writes errors to stdout
            let msg = if stderr.is_empty() {
                strip_ansi(&stdout)
            } else {
                strip_ansi(&stderr)
            };
            Err(NbError::CommandFailed(msg))
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
                notebook,
                "--path".to_string(),
            ])
            .await?;
        let path = output.trim();
        if path.is_empty() {
            return Err(NbError::CommandFailed(
                "nb notebooks path output was empty".to_string(),
            ));
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
        self.exec_vec(vec![
            "show".to_string(),
            selector,
            "--print".to_string(),
            "--no-color".to_string(),
        ])
        .await
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
            .map(|output| output::strip_empty_result_hint(&output))
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
            return Err(NbError::CommandFailed(
                "at least one search query is required".to_string(),
            ));
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
                Err(NbError::CommandFailed(message)) if is_empty_tasks_error(&message) => {
                    saw_empty = true;
                }
                Err(err) => return Err(err),
            }
        }
        if outputs.is_empty() && saw_empty {
            return Err(NbError::CommandFailed(empty_tasks_message(status)));
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
            .map(|output| output::strip_empty_result_hint(&output))
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

fn append_warning(mut output: String, warning: String) -> String {
    if !output.trim().is_empty() {
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push('\n');
    }
    output.push_str(&warning);
    output
}

fn validate_notebook_name(name: &str) -> Result<(), NbError> {
    if name.trim().is_empty() || name.contains(':') || name.contains('/') || name.contains('\\') {
        return Err(NbError::CommandFailed(NOTEBOOK_FIELD_MESSAGE.to_string()));
    }
    Ok(())
}

fn validate_folder_option(folder: Option<&str>) -> Result<(), NbError> {
    if let Some(path) = folder {
        validate_folder_path(path)?;
    }
    Ok(())
}

fn validate_folder_path(path: &str) -> Result<(), NbError> {
    if path.trim().is_empty() || path.contains(':') {
        return Err(NbError::CommandFailed(FOLDER_FIELD_MESSAGE.to_string()));
    }
    Ok(())
}

fn validate_destination(destination: &str) -> Result<(), NbError> {
    if destination.trim().is_empty() || destination.contains(':') {
        return Err(NbError::CommandFailed(
            "Invalid destination: use a folder path or filename only, not a notebook-qualified selector."
                .to_string(),
        ));
    }
    Ok(())
}

fn parse_qualified_selector(selector: &str) -> Result<Option<(&str, &str)>, NbError> {
    let Some((notebook, path)) = selector.split_once(':') else {
        return Ok(None);
    };
    validate_notebook_name(notebook)?;
    if path.trim().is_empty() || path.contains(':') {
        return Err(NbError::CommandFailed(
            "Invalid selector: use at most one notebook qualifier, as `<notebook>:<folder>/<id>`."
                .to_string(),
        ));
    }
    Ok(Some((notebook, path)))
}

/// Detect a duplicate-title H1 in `content`.
///
/// Returns `Some(<exact source line>)` when `content`'s first
/// nonblank line is a CommonMark ATX H1 and the trimmed heading
/// text equals the trimmed `title`.
///
/// Per CommonMark, an ATX H1 is:
/// - 0 to 3 leading spaces (4 or more is an indented code block);
/// - exactly one opening `#` (2+ is H2 or lower);
/// - a required space, tab, or end-of-line after the opening hash
///   (`#Title` is NOT an H1);
/// - the heading text (inline content);
/// - an optional closing hash sequence: a run of `#`s preceded
///   by a space or tab and followed only by spaces or tabs to
///   end of line. Literal trailing hashes with no preceding
///   whitespace (e.g., `# C#`) are part of the heading text, NOT
///   a closing sequence.
///
/// Returns `None` when the title is empty/None, the content has
/// no nonblank line, or the first nonblank line is not a
/// duplicate H1. Used by [`NbClient::add_note`](crate::NbClient::add_note)
/// to detect the common agent-side mistake of including the title
/// H1 inside the body content.
fn detect_duplicate_title_heading(title: &str, content: &str) -> Option<String> {
    let trimmed_title = title.trim();
    if trimmed_title.is_empty() {
        return None;
    }
    let first_nonblank = content.lines().find(|line| !line.trim().is_empty())?;
    let bytes = first_nonblank.as_bytes();

    // 0-3 leading spaces. 4 or more is an indented code block.
    let leading_spaces = bytes.iter().take_while(|&&b| b == b' ').count();
    if leading_spaces >= 4 {
        return None;
    }
    let after_indent = &first_nonblank[leading_spaces..];

    // Exactly one opening `#`. `##` and beyond are H2+.
    if !after_indent.starts_with('#') {
        return None;
    }
    if after_indent.starts_with("##") {
        return None;
    }
    let after_hash = &after_indent[1..];

    // Required space, tab, or EOL after the opening hash. `#Title`
    // is NOT an H1 (no delimiter). A bare `#` IS an H1 with empty
    // heading text per CommonMark; the title comparison below
    // filters out the false-positive case (empty heading text
    // never matches a non-empty title).
    let after_hash_bytes = after_hash.as_bytes();
    let rest = match after_hash_bytes.first() {
        Some(b' ') | Some(b'\t') => &after_hash[1..],
        None => "",
        Some(_) => return None,
    };

    // Optional closing hash sequence per CommonMark.
    let heading_text = strip_atx_closing_hashes(rest);

    if heading_text.trim() == trimmed_title {
        Some(first_nonblank.to_string())
    } else {
        None
    }
}

/// Strip an optional CommonMark ATX closing hash sequence from
/// the end of a heading body. The closing sequence is a run of
/// one or more `#`s preceded by a space or tab and followed only
/// by spaces or tabs to end of string. If no valid closing
/// sequence is present, returns the input unchanged. Literal
/// trailing hashes with no preceding whitespace (e.g., `C#`) are
/// NOT a closing sequence and are preserved.
///
/// A closing sequence can consume the entire post-delimiter
/// content (e.g., `# #`, `# ###`); in that case the heading
/// text becomes empty per CommonMark and never matches a
/// non-empty title.
fn strip_atx_closing_hashes(s: &str) -> String {
    let bytes = s.as_bytes();
    let n = bytes.len();

    // Find where trailing whitespace ends.
    let mut end = n;
    while end > 0 && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t') {
        end -= 1;
    }
    if end == 0 || bytes[end - 1] != b'#' {
        return s.to_string();
    }

    // Find the start of the trailing-hash sequence.
    let mut hash_start = end;
    while hash_start > 0 && bytes[hash_start - 1] == b'#' {
        hash_start -= 1;
    }

    // The closing sequence must be preceded by a space or tab
    // within the heading body. If the run of `#`s is preceded by
    // a non-whitespace char (e.g., `C#`), it is part of the
    // heading text, NOT a closing sequence; preserve the input.
    //
    // If the run is at offset 0 of the post-delimiter body, it
    // is preceded by the delimiter space (which is NOT in the
    // body); this is a valid closing sequence that consumes the
    // entire body. The heading text becomes empty.
    if hash_start == 0 {
        return s[..0].to_string();
    }
    let prev = bytes[hash_start - 1];
    if prev != b' ' && prev != b'\t' {
        return s.to_string();
    }

    s[..hash_start].to_string()
}

fn edit_args(selector: String, content: &str, mode: EditMode) -> Vec<String> {
    let mut args = vec!["edit".to_string(), selector];
    match mode {
        EditMode::Overwrite => args.push("--overwrite".to_string()),
        EditMode::Append => {}
        EditMode::Prepend => args.push("--prepend".to_string()),
    }
    args.push("--content".to_string());
    args.push(content.to_string());
    args
}

fn task_command_args(action: &str, selector: String, task_number: Option<u32>) -> Vec<String> {
    let mut args = vec![action.to_string(), selector];
    if let Some(number) = task_number {
        args.push(number.to_string());
    }
    args
}

fn todo_command_args(
    notebook: &str,
    title: &str,
    description: Option<&str>,
    tasks: &[String],
    tags: &[String],
    folder: Option<&str>,
) -> Vec<String> {
    let mut args = vec![format!("{notebook}:todo"), "add".to_string()];

    // Folder path comes as a positional argument before the title.
    if let Some(folder) = folder {
        args.push(folder_scope(folder));
    }

    args.push(title.to_string());

    if let Some(description) = description {
        args.push("--description".to_string());
        args.push(description.to_string());
    }

    for task in tasks {
        args.push("--task".to_string());
        args.push(task.to_string());
    }

    for tag in tags {
        args.push("--tags".to_string());
        args.push(normalize_tag(tag));
    }

    args
}

fn folder_scope(folder: &str) -> String {
    if folder.ends_with('/') {
        folder.to_string()
    } else {
        format!("{folder}/")
    }
}

fn normalize_tag(tag: &str) -> String {
    if tag.starts_with('#') {
        tag.to_string()
    } else {
        format!("#{tag}")
    }
}

fn normalize_folder(folder: &str) -> String {
    folder.trim_matches('/').to_string()
}

fn mkdir_selector(notebook: &str, path: &str) -> String {
    let normalized = normalize_folder(path);
    format!("{}:{}", notebook, normalized)
}

fn tasks_scope(notebook: &str, folder: Option<&str>) -> String {
    match folder {
        Some(path) if !path.is_empty() => format!("{}:{}/", notebook, path),
        _ => format!("{}:", notebook),
    }
}

fn tasks_command_args(scope: String, status: Option<TaskStatus>) -> Vec<String> {
    let mut args = vec!["tasks".to_string(), scope];
    if let Some(filter) = status {
        let status = match filter {
            TaskStatus::Open => "open",
            TaskStatus::Closed => "closed",
        };
        args.push(status.to_string());
    }
    args.push("--no-color".to_string());
    args
}

fn search_command_args(
    scope: String,
    queries: &[String],
    mode: SearchMode,
    tags: &[String],
) -> Vec<String> {
    let mut args = vec!["search".to_string(), scope];
    let mut terms = queries.iter();
    if let Some(first) = terms.next() {
        args.push(first.to_string());
    }
    match mode {
        SearchMode::Any => {
            for query in terms {
                args.push("--or".to_string());
                args.push(query.to_string());
            }
        }
        SearchMode::All => {
            for query in terms {
                args.push(query.to_string());
            }
        }
    }
    for tag in tags {
        args.push("--tag".to_string());
        args.push(normalize_tag(tag));
    }
    args.push("--no-color".to_string());
    args
}

fn is_empty_tasks_error(message: &str) -> bool {
    message.trim_start().starts_with("! 0 ") && message.contains(" tasks.")
}

fn empty_tasks_message(status: Option<TaskStatus>) -> String {
    match status {
        Some(TaskStatus::Open) => "! 0 open tasks.".to_string(),
        Some(TaskStatus::Closed) => "! 0 closed tasks.".to_string(),
        None => "! 0 tasks.".to_string(),
    }
}

fn child_folder_names(path: &std::path::Path) -> Result<Vec<String>, NbError> {
    let read_dir = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(NbError::Io(err)),
    };

    let mut names = Vec::new();
    for entry in read_dir {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(|value| value.to_string()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(NbError::Io(err)),
        };
        if meta.is_dir() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

const GIT_SIGNING_OVERRIDES: [(&str, &str); 2] =
    [("commit.gpgsign", "false"), ("tag.gpgsign", "false")];

fn git_signing_env_vars(start_index: usize) -> Vec<(String, String)> {
    let total = start_index.saturating_add(GIT_SIGNING_OVERRIDES.len());
    let mut env_vars = Vec::with_capacity(1 + GIT_SIGNING_OVERRIDES.len() * 2);
    env_vars.push(("GIT_CONFIG_COUNT".to_string(), total.to_string()));
    for (offset, (key, value)) in GIT_SIGNING_OVERRIDES.iter().enumerate() {
        let index = start_index + offset;
        env_vars.push((format!("GIT_CONFIG_KEY_{index}"), (*key).to_string()));
        env_vars.push((format!("GIT_CONFIG_VALUE_{index}"), (*value).to_string()));
    }
    env_vars
}

fn apply_git_signing_env(command: &mut Command) {
    // Always start at index 0. `scrub_git_env` has already removed
    // every `GIT_CONFIG_*` from the spawn-time `Command` env (the
    // blast-by-prefix defense-in-depth pattern, mirrored from
    // `nbspec:71f369e`), so the spawn-time `GIT_CONFIG_COUNT` is
    // 0. The parent's pre-scrub `GIT_CONFIG_COUNT` does not flow
    // through to the child and must not influence the index.
    // (Pre-patch behavior read the parent count via
    // `std::env::var("GIT_CONFIG_COUNT")`, which produced a gap
    // in the emitted indices whenever the parent had set
    // `GIT_CONFIG_*` — see the regression test in
    // `tests/integration/git_signing_overrides.rs`.)
    for (name, value) in git_signing_env_vars(0) {
        command.env(name, value);
    }
}
