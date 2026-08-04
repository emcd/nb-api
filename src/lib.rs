//! Typed Rust interface to the `nb` note-taking CLI.
//!
//! Handles notebook qualification, escaping, and output parsing.
//! Wraps the `nb` CLI as a subprocess, providing async methods for
//! all note-taking operations.

mod argv;
mod client;
mod diagnostics;
mod error;
pub mod fingerprint;
mod git;
mod git_env;
mod git_signing;
mod output;
pub mod parser;
pub(crate) mod tokenizer;
mod validate;

#[cfg(feature = "testing")]
pub mod testing;

pub use error::{IoError, IoErrorKind, NbError, ParseErrorKind};
pub use fingerprint::{Fingerprint, fingerprint as compute_fingerprint};
pub use git::{derive_git_notebook_name, git_rev_parse};
pub use git_env::{leaked_git_names, scrub_git_env, scrub_git_env_std};
pub use parser::{
    BodyFragments, DocumentKind, NoteDocument, ParseContext, SUPPORTED_DOCUMENT_EXTENSIONS,
    TagsIter, TagsStrIter, TodoState, parse,
};

use serde::{Deserialize, Serialize};

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
