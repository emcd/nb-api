//! Public wire types for body-aware reads, edits, and commit outcomes.
//!
//! 0.4.0 is text-first: every structured textual field is a JSON `String`.
//! The crate is base64-free; callers needing JSON transport base64 do it at
//! the wire layer. Non-UTF-8-but-text files surface as typed
//! [`crate::error::NbError::NonUtf8`]; the single raw-bytes escape hatch is
//! `NbClient::read_note_source_bytes`.

use serde::{Deserialize, Serialize};

use crate::error::NbError;
use crate::fingerprint::Fingerprint;
use crate::parser::{DocumentKind, TodoState};

/// Address an existing note by selector or notebook-relative path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NoteTarget {
    Selector { value: String },
    Path { value: String },
}

impl NoteTarget {
    pub fn selector(value: impl Into<String>) -> Self {
        Self::Selector {
            value: value.into(),
        }
    }

    pub fn path(value: impl Into<String>) -> Self {
        Self::Path {
            value: value.into(),
        }
    }

    pub fn value(&self) -> &str {
        match self {
            Self::Selector { value } | Self::Path { value } => value,
        }
    }
}

/// One body fragment exposed by structured show.
///
/// `bytes` is the fragment text (UTF-8); `start_byte`/`end_byte` are offsets
/// into `ShowNote.source`, so `body == concat(fragment.bytes)` and each
/// `source[start_byte..end_byte] == fragment.bytes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct BodyFragment {
    pub index: u32,
    pub bytes: String,
    pub start_byte: u32,
    pub end_byte: u32,
}

/// Structured `show_note` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ShowNote {
    pub selector: String,
    pub path: String,
    pub kind: DocumentKind,
    pub todo_state: Option<TodoState>,
    /// Full raw title line including trailing newline, as UTF-8.
    pub title: Option<String>,
    pub title_text: Option<String>,
    pub tags: Vec<String>,
    pub body_fragments: Vec<BodyFragment>,
    /// Derivable (`body_fragments.len() <= 1`); kept for wire convenience.
    pub body_contiguous: bool,
    /// Concatenated body-fragment bytes (excludes title H1 + tags).
    pub body: String,
    pub fingerprint: Fingerprint,
    /// Full file bytes verbatim, as UTF-8.
    pub source: String,
    /// Numeric `.index` id when resolvable via `.index` scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numeric_id: Option<u32>,
}

/// Versioned body-line authenticity token: `b3l1:<32 lowercase hex>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct LineAnchor(String);

impl LineAnchor {
    pub const PREFIX: &'static str = "b3l1:";

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_line_bytes(line_with_terminator_or_eof_marker: &[u8]) -> Self {
        let hash = blake3::hash(line_with_terminator_or_eof_marker);
        let hex = hash.to_hex();
        Self(format!("{}{}", Self::PREFIX, &hex[..32]))
    }

    pub fn parse(s: &str) -> Result<Self, NbError> {
        if !s.starts_with(Self::PREFIX) {
            return Err(NbError::ValidationError {
                reason: format!("unknown line anchor prefix in {s:?}"),
                location: None,
            });
        }
        let hex = &s[Self::PREFIX.len()..];
        if hex.len() != 32
            || !hex
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(NbError::ValidationError {
                reason: format!("invalid line anchor hex in {s:?}"),
                location: None,
            });
        }
        Ok(Self(s.to_string()))
    }
}

impl std::fmt::Display for LineAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Document-level end-of-line marker.
///
/// `Lf` for `\n`, `CrLf` for `\r\n`. Bare `\r` is not a supported EOL and is
/// preserved verbatim in line text; documents without a supported EOL report
/// `eol: None` on [`ShowNoteLines`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum LineEol {
    Lf,
    CrLf,
}

impl LineEol {
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Lf => b"\n",
            Self::CrLf => b"\r\n",
        }
    }
}

/// One enumerated body line (line text without terminator).
///
/// Unknown JSON fields (including the removed 0.3.x `terminator`) are ignored
/// on deserialization for forward compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct NoteLine {
    pub number: u32,
    pub anchor: LineAnchor,
    pub text: String,
}

/// Windowed body-line listing with a document-level EOL declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ShowNoteLines {
    pub selector: String,
    pub path: String,
    pub kind: DocumentKind,
    pub total_lines: u32,
    pub offset: u32,
    pub limit: u32,
    pub next_offset: Option<u32>,
    pub lines: Vec<NoteLine>,
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub body_fingerprint: Fingerprint,
    /// First-supported-EOL declaration; `None` for empty / single-line-no-EOL
    /// / CR-only bodies.
    pub eol: Option<LineEol>,
    /// Whether the body ends with `eol` bytes.
    pub has_final_eol: bool,
    /// Numeric `.index` id when resolvable via `.index` scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numeric_id: Option<u32>,
}

/// Number + anchor reference to a body line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct LineRef {
    pub number: u32,
    pub anchor: LineAnchor,
}

/// Insert position relative to a line or body boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LinePosition {
    Before { line: LineRef },
    After { line: LineRef },
    Boundary { at: BoundaryAt },
}

/// Virtual body boundaries (`^` / `$`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum BoundaryAt {
    Caret,
    Dollar,
}

/// One line-oriented edit in an `edit_note_lines` batch.
///
/// `content` is bare text (no terminator included; embedded `\r` preserved).
/// The library appends document `eol` bytes when materializing edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LineEdit {
    Insert {
        at: LinePosition,
        content: String,
    },
    Delete {
        start: LineRef,
        end: LineRef,
    },
    Replace {
        start: LineRef,
        end: LineRef,
        content: String,
    },
}

/// Substring occurrence selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Occurrence {
    First,
    All,
    Nth { n: u32 },
}

/// One search hit within a body line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct NoteLineHit {
    pub number: u32,
    pub anchor: LineAnchor,
    pub start_byte: u32,
    pub end_byte: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Result of `search_note_lines`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct SearchNoteLines {
    pub selector: String,
    pub path: String,
    pub kind: DocumentKind,
    pub hits: Vec<NoteLineHit>,
    pub body_fingerprint: Fingerprint,
}

/// Outcome of one plan op after a successful commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct OpOutcome {
    pub index: u32,
    pub path: Option<String>,
    pub selector: Option<String>,
    /// Numeric `.index` id when known (pinned `Option<u32>` type).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numeric_id: Option<u32>,
    pub noop: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
}

/// Structured result of [`crate::Transaction::commit`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct CommitOutcome {
    pub commit_created: bool,
    pub revision_id: Option<String>,
    pub pre_revision: String,
    pub ops: Vec<OpOutcome>,
}
