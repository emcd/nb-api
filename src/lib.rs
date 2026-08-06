//! Typed Rust interface to the `nb` note-taking CLI.
//!
//! Handles notebook qualification, escaping, and output parsing.
//! Wraps the `nb` CLI as a subprocess, providing async methods for
//! all note-taking operations.
//!
//! # Concurrency (0.3.0)
//!
//! Notebook-scoped reads and [`Transaction::commit`] are serialized by a
//! **process-shared**, **in-process** gate registry keyed by the realpath of
//! the notebook Git common directory. Independently constructed [`NbClient`]
//! values that resolve to the same repository share one gate. Cross-process
//! `index.lock` wait is deferred.

mod argv;
mod client;
mod diagnostics;
mod error;
pub mod fingerprint;
mod gate;
mod git;
mod git_env;
mod git_signing;
mod lines;
mod output;
pub mod parser;
pub(crate) mod tokenizer;
mod transaction;
mod types;
mod validate;

#[cfg(feature = "testing")]
pub mod testing;

#[cfg(feature = "testing")]
pub use gate::registry_len;

pub use error::{IoError, IoErrorKind, NbError, ParseErrorKind};
pub use fingerprint::{Fingerprint, fingerprint as compute_fingerprint};
pub use git::{derive_git_notebook_name, git_rev_parse};
pub use git_env::{leaked_git_names, scrub_git_env, scrub_git_env_std};
pub use parser::{
    BodyFragments, DocumentKind, NoteDocument, ParseContext, SUPPORTED_DOCUMENT_EXTENSIONS,
    TagsIter, TagsStrIter, TodoState, parse,
};
pub use transaction::Transaction;
pub use types::{
    BodyFragment, BoundaryAt, ByteString, CommitOutcome, LineAnchor, LineEdit, LinePosition,
    LineRef, LineTerminator, NoteLine, NoteLineHit, NoteTarget, Occurrence, OpOutcome,
    SearchNoteLines, ShowNote, ShowNoteLines,
};

use serde::Deserialize;
use std::time::Duration;

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
    /// Maximum time to wait on the process-shared gate queue.
    pub gate_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            notebook: None,
            create_notebook: true,
            allow_top_level_notes: false,
            disable_git_signing: false,
            gate_timeout: gate::DEFAULT_GATE_TIMEOUT,
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
    /// Gate queue timeout.
    gate_timeout: Duration,
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
