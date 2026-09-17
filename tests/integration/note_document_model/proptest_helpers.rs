//! Builder/expected-domain helpers for the document-model proptests.
//!
//! The `_v2`/`_v3` builders and `expected_*` values derive INDEPENDENTLY
//! from the generation inputs so parser bugs surface as mismatches.

use proptest::prelude::*;

/// A small printable body line. The regex requires at least
/// one non-whitespace character so the generator never
/// produces whitespace-only or empty bodies, which the parser
/// would not recognize as a body fragment.
pub(super) fn body_line() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9][a-zA-Z0-9 ,._-]{0,39}").unwrap()
}

/// A single tag token. We restrict to simple identifiers to
/// avoid whitespace and punctuation edge cases.
pub(super) fn tag_token() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-z][a-z0-9_-]{0,7}").unwrap()
}

/// A small list of tag tokens.
pub(super) fn tag_tokens() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(tag_token(), 1..4)
}

// ---------- Proptest v3 builders and expected helpers ----------
//
// The `_v3` builders and `expected_*_v3` helpers extend the
// original v1 (fixed LF, fixed double-blank) shapes with two
// randomized dimensions:
//
//   - `terminator` (LF / CR / CRLF): every line terminator in
//     the document uses the chosen terminator. The parser must
//     accept all three.
//   - `blank_count` (1..5): the number of blank-line separators
//     between sections. The parser must accept single, double,
//     triple, and quadruple blank lines.
//
// Every expected value (`expected_*_v3`) is derived
// INDEPENDENTLY from the generation inputs — terminator,
// blank_count, and the content strings. A bug in the parser's
// section/body selection that affects only some terminators
// or only some blank counts will be caught by the proptest.

/// Write `count` blank-line terminators as separators.
pub(super) fn write_blank_lines(bytes: &mut Vec<u8>, count: usize, terminator: &[u8]) {
    for _ in 0..count {
        bytes.extend_from_slice(terminator);
    }
}

/// Build a Note byte sequence with the chosen terminator and
/// blank-line separator count.
///
/// Layout:
/// ```
/// # <title>
/// <blank_count x terminator>
/// #<tag> #<tag> ...
/// <blank_count x terminator>
/// <body>
/// <terminator>
/// ```
pub(super) fn build_note_bytes_v2(
    title: &str,
    tags: &[String],
    body: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    if !title.is_empty() {
        bytes.extend_from_slice(b"# ");
        bytes.extend_from_slice(title.as_bytes());
        bytes.extend_from_slice(terminator);
        write_blank_lines(&mut bytes, blank_count, terminator);
        if !tags.is_empty() {
            for (i, tag) in tags.iter().enumerate() {
                if i > 0 {
                    bytes.push(b' ');
                }
                bytes.push(b'#');
                bytes.extend_from_slice(tag.as_bytes());
            }
            bytes.extend_from_slice(terminator);
        }
    }
    // If the title is empty, do not emit a tags-prefix line.
    // The parser treats a leading `#tag` (single-token, no
    // preceding H1) as body content, not as a tags prefix, so
    // including a tags-prefix line under an empty title would
    // shift the body selection unpredictably. The round-trip
    // is permitted when title is non-empty.
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Todo byte sequence with the chosen terminator and
/// blank-line separator count.
///
/// Layout:
/// ```
/// # [ ] <title>
/// <blank_count x terminator>
/// <body>
/// <terminator>
/// ```
pub(super) fn build_todo_bytes_v2(
    title: &str,
    body: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# [ ] ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Bookmark byte sequence with the chosen terminator
/// and blank-line separator count.
///
/// Layout:
/// ```
/// # <title>
/// <blank_count x terminator>
/// <URL>
/// <blank_count x terminator>
/// <body>
/// <terminator>
/// ```
pub(super) fn build_bookmark_bytes_v2(
    title: &str,
    body: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"<https://example.com>\n");
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    let _ = title;
    bytes
}

/// Build a Todo byte sequence with terminal `## Tags` and the
/// chosen terminator and blank-line separator count.
///
/// Layout:
/// ```
/// # [ ] <title>
/// <blank_count x terminator>
/// <body>
/// <terminator>
/// <blank_count x terminator>
/// ## Tags
/// <blank_count x terminator>
/// #<tag> #<tag> ...
/// <terminator>
/// ```
pub(super) fn build_todo_terminal_tags_bytes_v3(
    title: &str,
    body: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# [ ] ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Todo byte sequence where `## Tags` is NOT terminal
/// with the chosen terminator and blank-line separator count.
/// Per the spec, terminal Tags is the LAST H2 — when the last
/// H2 is something else, the Tags section is body content, not
/// metadata.
pub(super) fn build_todo_nonterminal_tags_bytes_v3(
    title: &str,
    body: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# [ ] ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Description");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Bookmark with `## Tags` before `## Content`, with the
/// chosen terminator and blank-line separator count. The
/// canonical-selection rule picks the FIRST Tags before
/// Content; the body fragment is the Content section.
pub(super) fn build_bookmark_tags_before_content_bytes_v3(
    title: &str,
    url: &str,
    tags: &[String],
    content: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.push(b'<');
    bytes.extend_from_slice(url.as_bytes());
    bytes.push(b'>');
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Content");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(content.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Bookmark with `## Tags` AFTER `## Content` (terminal),
/// with the chosen terminator and blank-line separator count.
/// The body fragment is the Content section only.
pub(super) fn build_bookmark_tags_after_content_terminal_bytes_v3(
    title: &str,
    url: &str,
    content: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.push(b'<');
    bytes.extend_from_slice(url.as_bytes());
    bytes.push(b'>');
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Content");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(content.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Bookmark with `## Source` (fenced HTML payload)
/// followed by terminal `## Tags`, with the chosen terminator
/// and blank-line separator count.
pub(super) fn build_bookmark_source_terminal_tags_bytes_v3(
    title: &str,
    url: &str,
    source_payload: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.push(b'<');
    bytes.extend_from_slice(url.as_bytes());
    bytes.push(b'>');
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Source");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"```html");
    bytes.extend_from_slice(terminator);
    bytes.extend_from_slice(source_payload.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes.extend_from_slice(b"```");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    bytes
}

/// Build a Bookmark with `## Content` (body section) directly
/// followed by terminal `## Tags` with NO blank-line
/// separator. This is the adjacent-section ownership regression
/// shape: the parser must preserve the body's trailing
/// terminator as part of the body, not strip it into a
/// separator.
pub(super) fn build_bookmark_tags_after_body_adjacent_bytes(
    title: &str,
    body: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"# ");
    bytes.extend_from_slice(title.as_bytes());
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(b"## Content");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    // No blank line between body and Tags — adjacent sections.
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    bytes
}

/// Expected `tag_section` bytes for a Bookmark with `## Tags`,
/// derived INDEPENDENTLY from the generation inputs.
///
/// `is_last_heading` distinguishes two cases:
///
/// - `true` (Tags is the LAST heading, no trailing section):
///   the section extends to `source.len()` and includes only
///   the heading + heading terminator + `blank_count` blank
///   line terminators (between heading and tag line) + tag
///   line + tag-line terminator. There are no blank line
///   terminators AFTER the tag line because the source ends
///   there.
/// - `false` (Tags is followed by another heading, e.g.
///   Content): the section's end is computed by stripping the
///   LAST blank-line terminator; only `blank_count - 1` blank
///   line terminators after the tag-token line are absorbed
///   into the section.
pub(super) fn expected_bookmark_tags_section_v3(
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
    is_last_heading: bool,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    // All `blank_count` blank-line terminators between heading
    // and tag line are absorbed.
    for _ in 0..blank_count {
        bytes.extend_from_slice(terminator);
    }
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    // Blank-line terminators after the tag line: 0 if Tags is
    // last (the source ends with the tag-line terminator);
    // (blank_count - 1) otherwise (the Nth is the separator,
    // stripped).
    let trailing_blanks = if is_last_heading {
        0
    } else {
        blank_count.saturating_sub(1)
    };
    for _ in 0..trailing_blanks {
        bytes.extend_from_slice(terminator);
    }
    bytes
}

/// Expected body bytes for a Bookmark with `## Content` as the
/// LAST heading, derived INDEPENDENTLY from the generation
/// inputs. The body starts at the `## Content` heading and
/// extends to the source end; ALL `blank_count` blank-line
/// terminators are absorbed into the section (no separator
/// stripping because Content is the last heading).
pub(super) fn expected_bookmark_content_section_v3(
    content: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"## Content");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    bytes.extend_from_slice(content.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes
}

/// Expected body bytes for a Bookmark with `## Content`
/// followed by terminal `## Tags`. The section absorbs ALL
/// `blank_count` blank-line terminators between the Content
/// heading and the content line, the content line, its
/// terminator, and `blank_count - 1` blank-line terminators
/// between the content line and the Tags heading (the LAST
/// one is stripped as separator).
pub(super) fn expected_bookmark_content_with_trailing_term_v3(
    content: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"## Content");
    bytes.extend_from_slice(terminator);
    // All `blank_count` blank-line terminators between Content
    // heading and content line are absorbed.
    for _ in 0..blank_count {
        bytes.extend_from_slice(terminator);
    }
    bytes.extend_from_slice(content.as_bytes());
    bytes.extend_from_slice(terminator);
    // (blank_count - 1) blank-line terminators after the
    // content line are absorbed; the LAST one is the separator.
    for _ in 0..blank_count.saturating_sub(1) {
        bytes.extend_from_slice(terminator);
    }
    bytes
}

/// Expected body bytes for a Bookmark with `## Source` and a
/// fenced HTML payload, followed by terminal `## Tags`. The
/// section absorbs `blank_count` blank-line terminators
/// between Source heading and the opening fence, all bytes
/// inside the fenced payload, and `blank_count - 1` blank-line
/// terminators between the closing fence and the Tags heading
/// (the LAST one is stripped as separator).
pub(super) fn expected_bookmark_source_section_v3(
    source_payload: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"## Source");
    bytes.extend_from_slice(terminator);
    for _ in 0..blank_count {
        bytes.extend_from_slice(terminator);
    }
    bytes.extend_from_slice(b"```html");
    bytes.extend_from_slice(terminator);
    bytes.extend_from_slice(source_payload.as_bytes());
    bytes.extend_from_slice(terminator);
    bytes.extend_from_slice(b"```");
    bytes.extend_from_slice(terminator);
    for _ in 0..blank_count.saturating_sub(1) {
        bytes.extend_from_slice(terminator);
    }
    bytes
}

/// Expected `tag_section` bytes for a Todo with terminal
/// `## Tags`, derived INDEPENDENTLY from the generation inputs.
pub(super) fn expected_todo_terminal_tags_v3(
    _body: &str,
    tags: &[String],
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"## Tags");
    bytes.extend_from_slice(terminator);
    write_blank_lines(&mut bytes, blank_count, terminator);
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            bytes.push(b' ');
        }
        bytes.push(b'#');
        bytes.extend_from_slice(t.as_bytes());
    }
    bytes.extend_from_slice(terminator);
    bytes
}

/// Expected body bytes for a Todo with terminal `## Tags`.
/// The parser's pre-Tags separator handling emits only ONE
/// separator (the LAST blank line terminator); the other
/// `blank_count - 1` blank line terminators are absorbed into
/// the body fragment. Expected behavior:
/// a "complete blank line" between body and Tags is a
/// separator, but the pre-Tags separator handling does not
/// split multiple blank lines into multiple separator ranges.
pub(super) fn expected_todo_terminal_body_v3(
    body: &str,
    terminator: &[u8],
    blank_count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(terminator);
    for _ in 0..blank_count.saturating_sub(1) {
        bytes.extend_from_slice(terminator);
    }
    bytes
}
