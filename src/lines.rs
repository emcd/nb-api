//! Body-line enumeration, anchors, and contiguous-body edits.
//!
//! 0.4.0 document-level EOL model: the body declares one `eol`
//! (`Some(Lf|CrLf)` or `None`) determined by the first *supported* EOL
//! occurrence scanning left-to-right. Bare `\r` is stray content — never a
//! terminator, never normalized, preserved verbatim in line text.

use std::ops::Range;

use crate::error::NbError;
use crate::parser::NoteDocument;
use crate::types::{
    BoundaryAt, LineAnchor, LineEdit, LineEol, LinePosition, LineRef, NoteLine, NoteLineHit,
    Occurrence,
};

/// A single body line with absolute offsets into the contiguous body bytes.
#[derive(Debug, Clone)]
pub struct BodyLine {
    pub number: u32,
    pub text_range: Range<usize>,
    pub full_range: Range<usize>,
    pub anchor: LineAnchor,
}

/// Detect the document EOL: first supported occurrence (`\r\n` or `\n`)
/// scanning left-to-right. Bare `\r` is ignored.
pub fn detect_eol(body: &[u8]) -> Option<LineEol> {
    let mut i = 0usize;
    while i < body.len() {
        if body[i] == b'\r' {
            if i + 1 < body.len() && body[i + 1] == b'\n' {
                return Some(LineEol::CrLf);
            }
            i += 1;
            continue;
        }
        if body[i] == b'\n' {
            return Some(LineEol::Lf);
        }
        i += 1;
    }
    None
}

/// Full document-line view: `(eol, has_final_eol, lines)`.
///
/// `has_final_eol` is true when the body ends with the declared `eol` bytes.
/// For `eol: None` it is always false (even for trailing bare `\r`).
pub fn document_lines(body: &[u8]) -> (Option<LineEol>, bool, Vec<BodyLine>) {
    if body.is_empty() {
        return (None, false, Vec::new());
    }
    let eol = detect_eol(body);
    let Some(eol) = eol else {
        // No supported EOL: single line, anchor over text + 0x00.
        let mut text = body.to_vec();
        text.push(0x00);
        return (
            None,
            false,
            vec![BodyLine {
                number: 1,
                text_range: 0..body.len(),
                full_range: 0..body.len(),
                anchor: LineAnchor::from_line_bytes(&text),
            }],
        );
    };
    let sep: &[u8] = eol.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut number = 1u32;
    let mut i = 0usize;
    while i < body.len() {
        if body[i..].starts_with(sep) {
            let end = i + sep.len();
            lines.push(BodyLine {
                number,
                text_range: start..i,
                full_range: start..end,
                anchor: LineAnchor::from_line_bytes(&body[start..end]),
            });
            number += 1;
            start = end;
            i = end;
            continue;
        }
        i += 1;
    }
    let has_final_eol = start == body.len();
    if start < body.len() {
        let mut tail = body[start..].to_vec();
        tail.push(0x00);
        lines.push(BodyLine {
            number,
            text_range: start..body.len(),
            full_range: start..body.len(),
            anchor: LineAnchor::from_line_bytes(&tail),
        });
    }
    (Some(eol), has_final_eol, lines)
}

pub fn require_contiguous_body(doc: &NoteDocument) -> Result<Vec<u8>, NbError> {
    let ranges = doc.body_ranges();
    if ranges.len() >= 2 {
        return Err(NbError::FragmentedBody {
            fragment_count: ranges.len() as u32,
            guidance: "body line/search/substring/replace require a contiguous body; use metadata ops or a future multi-fragment API".to_string(),
        });
    }
    Ok(doc.body_bytes())
}

pub fn note_line_from_body_line(line: &BodyLine, body: &[u8]) -> NoteLine {
    NoteLine {
        number: line.number,
        anchor: line.anchor.clone(),
        text: String::from_utf8_lossy(&body[line.text_range.clone()]).into_owned(),
    }
}

pub fn verify_line_ref(lines: &[BodyLine], reference: &LineRef) -> Result<usize, NbError> {
    let idx = reference
        .number
        .checked_sub(1)
        .ok_or_else(|| NbError::AnchorMismatch {
            target: format!("line {}", reference.number),
            number: reference.number,
            guidance: "line numbers are 1-based".to_string(),
        })? as usize;
    let line = lines.get(idx).ok_or_else(|| NbError::AnchorMismatch {
        target: format!("line {}", reference.number),
        number: reference.number,
        guidance: "line number out of range for current body".to_string(),
    })?;
    if line.anchor != reference.anchor {
        return Err(NbError::AnchorMismatch {
            target: format!("line {}", reference.number),
            number: reference.number,
            guidance: "line anchor does not match current body bytes; re-read lines and retry"
                .to_string(),
        });
    }
    Ok(idx)
}

/// Apply a batch of line edits to contiguous body bytes. Returns new body bytes.
///
/// `content` is bare text; the document `eol` (or adopted `Lf` when the
/// document had none) is appended when materializing inserts/replaces. Edits
/// that materialize a new boundary on an `eol: None` document adopt `Lf`.
/// Inserting at a position equal to end-of-body when the final line lacks a
/// terminator first terminates the previous final line.
pub fn apply_line_edits(body: &[u8], edits: &[LineEdit]) -> Result<Vec<u8>, NbError> {
    let (eol_opt, has_final_eol, lines) = document_lines(body);
    // Effective EOL for materialized boundaries: declared, else Lf when any
    // edit introduces a boundary. Pure-delete batches on eol-None stay boundary-free.
    let introduces_boundary = edits.iter().any(|e| match e {
        LineEdit::Insert { .. } => true,
        LineEdit::Replace { .. } => true,
        LineEdit::Delete { .. } => false,
    });
    let eff_eol = match eol_opt {
        Some(e) => Some(e),
        None if introduces_boundary => Some(LineEol::Lf),
        None => None,
    };
    let eol_bytes: &[u8] = match eff_eol {
        Some(e) => e.as_bytes(),
        None => b"",
    };

    #[derive(Clone)]
    struct Resolved {
        delete: Range<usize>,
        insert: Vec<u8>,
        edit_index: usize,
    }
    let mut resolved = Vec::with_capacity(edits.len());
    for (edit_index, edit) in edits.iter().enumerate() {
        match edit {
            LineEdit::Insert { at, content } => {
                let mut bytes = Vec::new();
                let pos = insert_offset(body, &lines, at, has_final_eol)?;
                // Terminate a final line that lacks EOL when appending at end.
                if !body.is_empty() && !has_final_eol && pos == body.len() {
                    bytes.extend_from_slice(eol_bytes);
                }
                // Empty body + Dollar is rejected in insert_offset; Caret works.
                bytes.extend_from_slice(content.as_bytes());
                // Only append EOL when we have an effective EOL. Pure
                // boundary-free path (delete-only on None) never reaches here
                // because this arm is an insert (boundary ⇒ eff Some).
                bytes.extend_from_slice(eol_bytes);
                resolved.push(Resolved {
                    delete: pos..pos,
                    insert: bytes,
                    edit_index,
                });
            }
            LineEdit::Delete { start, end } => {
                let span = inclusive_span(&lines, start, end)?;
                resolved.push(Resolved {
                    delete: span,
                    insert: Vec::new(),
                    edit_index,
                });
            }
            LineEdit::Replace {
                start,
                end,
                content,
            } => {
                let span = inclusive_span(&lines, start, end)?;
                let mut bytes = content.as_bytes().to_vec();
                // Normalize to trailing EOL: replaced span always ends with eol
                // when effective EOL exists (adopt Lf for None docs).
                bytes.extend_from_slice(eol_bytes);
                resolved.push(Resolved {
                    delete: span,
                    insert: bytes,
                    edit_index,
                });
            }
        }
    }
    for i in 0..resolved.len() {
        for j in (i + 1)..resolved.len() {
            let a = &resolved[i];
            let b = &resolved[j];
            if ranges_overlap_or_same_insert(&a.delete, &b.delete) {
                return Err(NbError::OverlappingEdits {
                    indices: vec![a.edit_index as u32, b.edit_index as u32],
                });
            }
        }
    }
    resolved.sort_by(|a, b| {
        b.delete
            .start
            .cmp(&a.delete.start)
            .then(b.edit_index.cmp(&a.edit_index))
    });
    let mut out = body.to_vec();
    for r in resolved {
        out.splice(r.delete, r.insert);
    }
    Ok(out)
}

fn ranges_overlap_or_same_insert(a: &Range<usize>, b: &Range<usize>) -> bool {
    if a.start == a.end && b.start == b.end {
        return a.start == b.start;
    }
    a.start < b.end && b.start < a.end
}

fn inclusive_span(
    lines: &[BodyLine],
    start: &LineRef,
    end: &LineRef,
) -> Result<Range<usize>, NbError> {
    let s = verify_line_ref(lines, start)?;
    let e = verify_line_ref(lines, end)?;
    if e < s {
        return Err(NbError::ValidationError {
            reason: "line edit end is before start".to_string(),
            location: None,
        });
    }
    Ok(lines[s].full_range.start..lines[e].full_range.end)
}

fn insert_offset(
    body: &[u8],
    lines: &[BodyLine],
    at: &LinePosition,
    has_final_eol: bool,
) -> Result<usize, NbError> {
    match at {
        LinePosition::Boundary {
            at: BoundaryAt::Caret,
        } => Ok(0),
        LinePosition::Boundary {
            at: BoundaryAt::Dollar,
        } => {
            if body.is_empty() {
                return Err(NbError::ValidationError {
                    reason: "empty body: Caret is the only valid boundary; Dollar requires at least one line".to_string(),
                    location: None,
                });
            }
            let _ = has_final_eol;
            Ok(body.len())
        }
        LinePosition::Before { line } => {
            let idx = verify_line_ref(lines, line)?;
            Ok(lines[idx].full_range.start)
        }
        LinePosition::After { line } => {
            let idx = verify_line_ref(lines, line)?;
            Ok(lines[idx].full_range.end)
        }
    }
}

/// Non-overlapping left-to-right substring matches in `body`.
pub fn find_matches(body: &[u8], pattern: &[u8]) -> Result<Vec<Range<usize>>, NbError> {
    if pattern.is_empty() {
        return Err(NbError::EmptySubstringPattern);
    }
    let mut matches = Vec::new();
    let mut start = 0usize;
    while start + pattern.len() <= body.len() {
        if &body[start..start + pattern.len()] == pattern {
            let end = start + pattern.len();
            matches.push(start..end);
            start = end;
        } else {
            start += 1;
        }
    }
    Ok(matches)
}

pub fn apply_substring(
    body: &[u8],
    pattern: &[u8],
    replacement: &[u8],
    occurrence: &Occurrence,
    expected_count: u32,
) -> Result<Vec<u8>, NbError> {
    let matches = find_matches(body, pattern)?;
    let actual = matches.len() as u32;
    if actual != expected_count {
        return Err(NbError::OccurrenceMismatch {
            expected: expected_count,
            actual,
        });
    }
    let selected: Vec<Range<usize>> = match occurrence {
        Occurrence::First => matches.into_iter().take(1).collect(),
        Occurrence::All => matches,
        Occurrence::Nth { n } => {
            if *n == 0 {
                return Err(NbError::ValidationError {
                    reason: "occurrence nth.n is 1-based; 0 is invalid".to_string(),
                    location: None,
                });
            }
            matches
                .into_iter()
                .nth((*n as usize) - 1)
                .into_iter()
                .collect()
        }
    };
    let mut out = body.to_vec();
    for m in selected.into_iter().rev() {
        out.splice(m, replacement.iter().copied());
    }
    Ok(out)
}

/// Search line texts for `pattern` (byte-level).
pub fn search_lines(body: &[u8], pattern: &[u8]) -> Result<Vec<NoteLineHit>, NbError> {
    if pattern.is_empty() {
        return Err(NbError::EmptySubstringPattern);
    }
    let (_, _, lines) = document_lines(body);
    let mut hits = Vec::new();
    for line in &lines {
        let text = &body[line.text_range.clone()];
        let mut start = 0usize;
        while start + pattern.len() <= text.len() {
            if &text[start..start + pattern.len()] == pattern {
                let end = start + pattern.len();
                hits.push(NoteLineHit {
                    number: line.number,
                    anchor: line.anchor.clone(),
                    start_byte: start as u32,
                    end_byte: end as u32,
                    text: Some(String::from_utf8_lossy(text).into_owned()),
                });
                start = end;
            } else {
                start += 1;
            }
        }
    }
    Ok(hits)
}

/// Splice `new_body` into a document that has a contiguous body domain.
pub fn splice_body(doc: &NoteDocument, new_body: &[u8]) -> Result<Vec<u8>, NbError> {
    let ranges = doc.body_ranges();
    if ranges.len() >= 2 {
        return Err(NbError::FragmentedBody {
            fragment_count: ranges.len() as u32,
            guidance: "replace_note_body requires a contiguous body".to_string(),
        });
    }
    let source = doc.source();
    if ranges.is_empty() {
        let mut out = source.to_vec();
        out.extend_from_slice(new_body);
        return Ok(out);
    }
    let span = ranges[0].clone();
    let mut out = Vec::with_capacity(source.len() - (span.end - span.start) + new_body.len());
    out.extend_from_slice(&source[..span.start]);
    out.extend_from_slice(new_body);
    out.extend_from_slice(&source[span.end..]);
    Ok(out)
}

/// Replace title line bytes (should include trailing newline when original had one).
pub fn splice_title(doc: &NoteDocument, new_title_line: &[u8]) -> Result<Vec<u8>, NbError> {
    let source = doc.source();
    match doc.title_byte_range() {
        Some(span) => {
            let mut title = new_title_line.to_vec();
            if !title.ends_with(b"\n") {
                title.push(b'\n');
            }
            let mut out = Vec::with_capacity(source.len() - (span.end - span.start) + title.len());
            out.extend_from_slice(&source[..span.start]);
            out.extend_from_slice(&title);
            out.extend_from_slice(&source[span.end..]);
            Ok(out)
        }
        None => {
            let mut title = new_title_line.to_vec();
            if !title.ends_with(b"\n") {
                title.push(b'\n');
            }
            let mut out = Vec::with_capacity(title.len() + 1 + source.len());
            out.extend_from_slice(&title);
            if !source.is_empty() && !source.starts_with(b"\n") {
                out.push(b'\n');
            }
            out.extend_from_slice(source);
            Ok(out)
        }
    }
}
