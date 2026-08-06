//! Public wire types for body-aware reads, edits, and commit outcomes.

use serde::{Deserialize, Serialize};

use crate::error::NbError;
use crate::fingerprint::Fingerprint;
use crate::parser::{DocumentKind, TodoState};

/// Arbitrary file bytes on the wire as standard base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ByteString {
    pub base64: String,
}

impl ByteString {
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        use base64::Engine;
        Self {
            base64: base64::engine::general_purpose::STANDARD.encode(bytes.as_ref()),
        }
    }

    pub fn as_bytes(&self) -> Result<Vec<u8>, NbError> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(self.base64.as_bytes())
            .map_err(|e| NbError::ValidationError {
                reason: format!("invalid ByteString base64: {e}"),
                location: None,
            })
    }
}

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct BodyFragment {
    pub index: u32,
    pub bytes: ByteString,
}

/// Structured `show_note` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ShowNote {
    pub selector: String,
    pub path: String,
    pub kind: DocumentKind,
    pub todo_state: Option<TodoState>,
    pub title: Option<ByteString>,
    pub title_text: Option<String>,
    pub tags: Vec<String>,
    pub body_fragments: Vec<BodyFragment>,
    pub body_contiguous: bool,
    pub body: ByteString,
    pub fingerprint: Fingerprint,
    pub source: ByteString,
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

/// Line terminator as stored in the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum LineTerminator {
    Lf,
    Crlf,
    Cr,
    None,
}

/// One enumerated body line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct NoteLine {
    pub number: u32,
    pub anchor: LineAnchor,
    pub text: ByteString,
    pub terminator: LineTerminator,
}

/// Windowed body-line listing.
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
    pub title: Option<ByteString>,
    pub tags: Vec<String>,
    pub body_fingerprint: Fingerprint,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LineEdit {
    Insert {
        at: LinePosition,
        content: ByteString,
    },
    Delete {
        start: LineRef,
        end: LineRef,
    },
    Replace {
        start: LineRef,
        end: LineRef,
        content: ByteString,
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
    pub text: Option<ByteString>,
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
