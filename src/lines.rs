//! Body-line enumeration, anchors, and contiguous-body edits.

use std::ops::Range;

use crate::error::NbError;
use crate::parser::NoteDocument;
use crate::types::{
    BoundaryAt, ByteString, LineAnchor, LineEdit, LinePosition, LineRef, LineTerminator, NoteLine,
    NoteLineHit, Occurrence,
};

/// A single body line with absolute offsets into the contiguous body bytes.
#[derive(Debug, Clone)]
pub struct BodyLine {
    pub number: u32,
    pub text_range: Range<usize>,
    pub full_range: Range<usize>,
    pub terminator: LineTerminator,
    pub anchor: LineAnchor,
}

/// Split contiguous body bytes into lines (terminator-preserving).
///
/// Empty body yields zero lines. A final segment without a terminator is one
/// line with [`LineTerminator::None`].
pub fn split_body_lines(body: &[u8]) -> Vec<BodyLine> {
    if body.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut number = 1u32;
    let mut i = 0usize;
    while i < body.len() {
        if body[i] == b'\r' {
            let (term_end, terminator) = if i + 1 < body.len() && body[i + 1] == b'\n' {
                (i + 2, LineTerminator::Crlf)
            } else {
                (i + 1, LineTerminator::Cr)
            };
            lines.push(make_line(
                number,
                body,
                start..i,
                start..term_end,
                terminator,
            ));
            number += 1;
            start = term_end;
            i = term_end;
            continue;
        }
        if body[i] == b'\n' {
            lines.push(make_line(
                number,
                body,
                start..i,
                start..i + 1,
                LineTerminator::Lf,
            ));
            number += 1;
            start = i + 1;
            i += 1;
            continue;
        }
        i += 1;
    }
    if start < body.len() {
        lines.push(make_line(
            number,
            body,
            start..body.len(),
            start..body.len(),
            LineTerminator::None,
        ));
    }
    lines
}

fn make_line(
    number: u32,
    body: &[u8],
    text_range: Range<usize>,
    full_range: Range<usize>,
    terminator: LineTerminator,
) -> BodyLine {
    let hash_input = match terminator {
        LineTerminator::None => {
            let mut v = body[text_range.clone()].to_vec();
            v.push(0x00);
            v
        }
        _ => body[full_range.clone()].to_vec(),
    };
    BodyLine {
        number,
        text_range,
        full_range,
        terminator,
        anchor: LineAnchor::from_line_bytes(&hash_input),
    }
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
        text: ByteString::from_bytes(&body[line.text_range.clone()]),
        terminator: line.terminator,
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
pub fn apply_line_edits(body: &[u8], edits: &[LineEdit]) -> Result<Vec<u8>, NbError> {
    let lines = split_body_lines(body);
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
                let bytes = content.as_bytes()?;
                let pos = insert_offset(body, &lines, at)?;
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
                resolved.push(Resolved {
                    delete: span,
                    insert: content.as_bytes()?,
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

fn insert_offset(body: &[u8], lines: &[BodyLine], at: &LinePosition) -> Result<usize, NbError> {
    match at {
        LinePosition::Boundary {
            at: BoundaryAt::Caret,
        } => Ok(0),
        LinePosition::Boundary {
            at: BoundaryAt::Dollar,
        } => Ok(body.len()),
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
    let lines = split_body_lines(body);
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
                    text: Some(ByteString::from_bytes(text)),
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
