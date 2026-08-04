//! Structural parser for `nb` notes, todos, and bookmarks.
//!
//! Defines [`NoteDocument`] — the canonical byte-range partition
//! for any source file produced or accepted by `nb 7.24.0` — plus
//! the [`DocumentKind`], [`TodoState`], [`ParseContext`], and
//! accessor types required by the `add-note-document-model` (P1)
//! specification.
//!
//! Permissive acceptance is the contract: `parse` succeeds for
//! every input that can be losslessly partitioned into a
//! [`NoteDocument`], including the frozen W1 and E1-E11
//! noncanonical forms. The only refusal cases at the parse layer
//! are [`ParseErrorKind::MissingTitle`] for Todo/Bookmark files
//! with no non-blank line. Notes are permissive on empty source.
//!
//! Mandatory-title/URL/state enforcement belongs to a separate
//! canonical validator (P5+), not to the parser. Operation
//! boundaries (`do`/`undo` for Todo) enforce checkbox state at
//! the operation level, not the parse level.
//!
//! Format dispatch (R3 revision) precedes byte parsing on
//! `ParseContext::FromPath`. Files whose final filename matches
//! a recognized-but-unsupported dotted suffix (e.g., `.org`,
//! `.latex`, `.tex`, `.adoc`, `.asciidoc`) are rejected with
//! `NbError::UnsupportedDocumentFormat` *before* any byte parsing
//! runs. The list of supported extensions lives in the public
//! constant [`SUPPORTED_DOCUMENT_EXTENSIONS`].
//!
//! [`ParseErrorKind`]: crate::error::ParseErrorKind
//! [`NbError::ParseError`]: crate::error::NbError::ParseError

#![allow(clippy::useless_vec, clippy::single_range_in_vec_init)]

mod assemble;
mod classify;
mod dispatch;
mod document;
mod helpers;
mod types;

pub use dispatch::SUPPORTED_DOCUMENT_EXTENSIONS;
pub use document::{
    BodyFragments, DocumentKind, NoteDocument, ParseContext, TagsIter, TagsStrIter, TodoState,
};

use crate::error::NbError;
use crate::tokenizer::tokenize;

use assemble::{assemble_bookmark, assemble_note, assemble_todo, translate_parse_failure};
use classify::{classify_bookmark, classify_note, classify_todo};
use dispatch::{FormatDispatch, format_dispatch};
use document::Partition;

/// Parse bytes into a [`NoteDocument`].
///
/// Permissive acceptance: returns `Ok` for every input that can
/// be losslessly partitioned, including the frozen W1 and
/// E1-E11 noncanonical forms. The only refusal cases at the
/// parse layer are [`ParseErrorKind::MissingTitle`] for
/// Todo/Bookmark files with no non-blank line. Notes are
/// permissive on empty source.
pub fn parse(bytes: &[u8], context: ParseContext) -> Result<NoteDocument, NbError> {
    let kind = match context {
        ParseContext::FromPath(path) => match format_dispatch(&path) {
            FormatDispatch::SupportedKind(kind) => kind,
            FormatDispatch::Rejected(extension) => {
                // Format dispatch precedes byte parsing. The
                // owned `Vec<String>` for the supported list
                // ensures the resulting NbError round-trips
                // through serde cleanly.
                return Err(NbError::UnsupportedDocumentFormat {
                    extension,
                    supported: SUPPORTED_DOCUMENT_EXTENSIONS
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                });
            }
            FormatDispatch::MarkdownNote => DocumentKind::Note,
        },
        // `Explicit(DocumentKind)` bypasses path dispatch and
        // is treated as Markdown without going through
        // format-dispatch. It can NEVER produce
        // `UnsupportedDocumentFormat` per the spec, but remains
        // subject to the selected Markdown kind's ordinary
        // parse failures (e.g., `Explicit(Todo)` with empty
        // bytes returns `MissingTitle`).
        ParseContext::Explicit(kind) => kind,
    };
    let mut source = Vec::with_capacity(bytes.len());
    source.extend_from_slice(bytes);
    let (preamble, lines) = tokenize(&source);
    let partition = match kind {
        DocumentKind::Note => {
            let tokens = classify_note(&lines);
            match assemble_note(&source, &preamble, &lines, &tokens) {
                Ok(p) => Partition::Note(p),
                Err(failure) => return Err(translate_parse_failure(failure)),
            }
        }
        DocumentKind::Todo => {
            let tokens = classify_todo(&lines);
            match assemble_todo(&source, &preamble, &lines, &tokens) {
                Ok(p) => Partition::Todo(p),
                Err(failure) => return Err(translate_parse_failure(failure)),
            }
        }
        DocumentKind::Bookmark => {
            let tokens = classify_bookmark(&lines);
            match assemble_bookmark(&source, &preamble, &lines, &tokens) {
                Ok(p) => Partition::Bookmark(p),
                Err(failure) => return Err(translate_parse_failure(failure)),
            }
        }
    };
    let doc = NoteDocument { source, partition };
    // Sanity-check the partition in debug builds. Production
    // callers do not pay this cost; the panic on invariant
    // violation surfaces parser regressions during testing.
    debug_assert!(doc.verify_partition().is_ok());
    Ok(doc)
}
