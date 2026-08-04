//! Format-agnostic tokenizer for `nb` source bytes.
//!
//! The tokenizer owns the **BOM-only
//! preamble** and emits one [`Line`] per physical line. It does
//! not look for `##`/`#`/`=` or any marker — it has no notion of
//! Markdown or any other format.
//!
//! Per the frozen private-intermediate requirement, the
//! tokenizer types and entry point are crate-private (`pub(crate)`).
//! They are visible only to the parser module within this crate;
//! library consumers see the byte-faithful `NoteDocument` surface
//! and nothing of the layered pipeline.
//!
//! Two crate-private types:
//!
//! - [`Preamble`] — a half-open byte range covering 0 or 3
//!   bytes (UTF-8 BOM) at the start of source.
//! - [`Line`] — a borrowed slice of source plus its terminator
//!   (`b""`, `b"\n"`, `b"\r\n"`, or `b"\r"`).
//!
//! The [`tokenize`] function returns `(Preamble, Vec<Line>)` and
//! is the only entry point.

use std::ops::Range;

/// The UTF-8 byte-order mark: `EF BB BF`.
const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

/// BOM-only preamble range.
///
/// Zero bytes (`0..0`) when the source begins with no BOM;
/// three bytes (`0..3`) when the source begins with the UTF-8
/// BOM. Leading whitespace
/// and leading blank lines do NOT belong to the preamble — they
/// are ordinary `Line` tokens emitted by [`tokenize`] and owned
/// later by `separator_ranges` / a body fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Preamble {
    pub(crate) range: Range<usize>,
}

impl Preamble {
    /// Detect a UTF-8 BOM at `source[0..]` and emit a
    /// corresponding half-open range. Returns `0..0` when no
    /// BOM is present.
    pub fn new(source: &[u8]) -> Self {
        if source.starts_with(UTF8_BOM) {
            Self {
                range: 0..UTF8_BOM.len(),
            }
        } else {
            Self { range: 0..0 }
        }
    }

    /// The byte offset at which physical lines begin — i.e.,
    /// the byte just past the BOM. Always equal to
    /// `range.end`; no whitespace folding.
    pub(crate) fn start_of_lines(&self) -> usize {
        self.range.end
    }
}

/// A physical line in the source.
///
/// `terminator` is one of:
///
/// - `b""` — the source ends without a line terminator (the
///   final line is unterminated).
/// - `b"\n"` — Unix LF terminator.
/// - `b"\r\n"` — Windows CRLF terminator.
/// - `b"\r"` — classic Mac CR terminator.
///
/// `content` is the bytes preceding the terminator on the same
/// line. For an empty line the content is `b""` and the
/// `range` covers only the terminator bytes.
///
/// `range` spans `[line_start, terminator_end)` — i.e.,
/// everything the line owns in the source. The assembler
/// reuses these ranges directly without re-deriving them from
/// byte cursors (tokens carry per-line
/// ranges; the assembler reads them as-is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line<'a> {
    pub(crate) range: Range<usize>,
    pub(crate) terminator: &'a [u8],
    pub(crate) content: &'a [u8],
}

/// Tokenize `source` into a (BOM preamble, physical lines)
/// pair. Format-agnostic: never inspects `##`/`#`/`=` or any
/// marker.
///
/// An EOF-unterminated final line is emitted with
/// `terminator = b""` and `range = content_range`. The
/// `terminator_end_at` helper returns `pos` unchanged when
/// `pos >= source.len()`, so a final line with content but no
/// terminator is distinguishable from a bare-terminator blank
/// line by `line.content.is_empty()` (zero vs non-zero).
pub(crate) fn tokenize(source: &[u8]) -> (Preamble, Vec<Line<'_>>) {
    let preamble = Preamble::new(source);
    let mut lines = Vec::new();
    let mut i = preamble.start_of_lines();
    while i < source.len() {
        let line_content_end = line_end_at(source, i);
        let has_content = line_content_end > i;
        if has_content {
            // Content line. Terminator may be empty if the line
            // is EOF-unterminated; otherwise it is the bytes
            // starting at line_content_end through terminator_end.
            let term_start = line_content_end;
            let term_end = terminator_end_at(source, term_start);
            let (range, terminator) = if term_start < source.len() {
                (i..term_end, &source[term_start..term_end][..])
            } else {
                // EOF-unterminated: range covers content only.
                (i..line_content_end, &[][..])
            };
            lines.push(Line {
                range,
                terminator,
                content: &source[i..line_content_end],
            });
            i = term_end.max(line_content_end);
        } else {
            // Bare-terminator line (zero-content line, e.g.,
            // `\n` at end of a blank line). `line_content_end
            // == i`.
            let term_end = terminator_end_at(source, i);
            lines.push(Line {
                range: i..term_end,
                terminator: &source[i..term_end],
                content: b"",
            });
            i = term_end;
        }
    }
    (preamble, lines)
}

/// Return the byte offset just past the line content at `pos`
/// (i.e., before any line terminator). If `pos` is already at or
/// past end, returns `pos` unchanged.
fn line_end_at(source: &[u8], pos: usize) -> usize {
    let mut i = pos;
    while i < source.len() {
        match source[i] {
            b'\n' | b'\r' => break,
            _ => i += 1,
        }
    }
    i
}

/// Return the byte offset just past the line terminator
/// starting at `pos` (which must be a `\n`, `\r`, or `\r\n`).
/// If `pos` is past end, returns `pos`.
///
/// A bare `\n` advances one byte. A bare `\r` advances one
/// byte. A `\r\n` pair advances two bytes. No other byte is
/// recognized as a terminator here.
fn terminator_end_at(source: &[u8], pos: usize) -> usize {
    if pos >= source.len() {
        return pos;
    }
    match source[pos] {
        b'\n' => pos + 1,
        b'\r' if pos + 1 < source.len() && source[pos + 1] == b'\n' => pos + 2,
        b'\r' => pos + 1,
        _ => pos,
    }
}
