//! Shared byte-line helpers used by classify, assemble, and document accessors.

use std::ops::Range;

use crate::tokenizer::Line;

use super::document::TodoState;

pub(crate) fn consume_leading_blank_intervals(source: &[u8], mut pos: usize) -> usize {
    let line_end_at = |pos: usize| -> usize {
        let mut i = pos;
        while i < source.len() {
            match source[i] {
                b'\n' | b'\r' => break,
                _ => i += 1,
            }
        }
        i
    };
    let terminator_end_at = |pos: usize| -> usize {
        if pos >= source.len() {
            return pos;
        }
        match source[pos] {
            b'\n' => pos + 1,
            b'\r' if pos + 1 < source.len() && source[pos + 1] == b'\n' => pos + 2,
            b'\r' => pos + 1,
            _ => pos,
        }
    };
    while pos < source.len() {
        if source[pos] == b'\n' || source[pos] == b'\r' {
            pos = terminator_end_at(pos);
            continue;
        }
        let line_end = line_end_at(pos);
        if line_end == pos {
            break;
        }
        let line = &source[pos..line_end];
        if line.iter().all(|&b| b == b' ' || b == b'\t') {
            pos = terminator_end_at(line_end);
        } else {
            break;
        }
    }
    pos
}

/// Parse a Todo title line for checkbox state. Returns `None`
/// if no checkbox prefix is present.
pub(crate) fn parse_todo_state_from_title(line: &[u8]) -> Option<TodoState> {
    // Strip optional leading whitespace.
    let i = line.iter().position(|&b| b != b' ').unwrap_or(0);
    let rest = &line[i..];
    // Title must start with `# ` (ATX H1).
    let rest = rest.strip_prefix(b"#")?;
    let rest = rest.strip_prefix(b" ")?;
    // Optional checkbox prefix: `[ ] ` → Open, `[x] ` or `[X] ` →
    // Done. No checkbox prefix → caller returns None.
    if let Some(rest) = rest.strip_prefix(b"[ ] ") {
        let _ = rest;
        Some(TodoState::Open)
    } else if rest.strip_prefix(b"[x] ").is_some() || rest.strip_prefix(b"[X] ").is_some() {
        Some(TodoState::Done)
    } else {
        None
    }
}

/// Permissive: any line containing at least two `#`-prefixed
/// non-empty tokens separated by ASCII whitespace, with optional
/// trailing whitespace before the line terminator. Used by the
/// Note classifier to recognize a tags-prefix line.
pub(crate) fn is_tags_prefix_line(line: &[u8]) -> bool {
    if line.is_empty() {
        return false;
    }
    let mut tokens = 0usize;
    let mut i = 0;
    loop {
        if i >= line.len() || line[i] != b'#' {
            return false;
        }
        i += 1;
        let token_start = i;
        while i < line.len() && line[i] != b' ' && line[i] != b'\t' && line[i] != b'#' {
            i += 1;
        }
        if i == token_start {
            return false;
        }
        tokens += 1;
        if i >= line.len() {
            return tokens >= 2;
        }
        if line[i] != b' ' && line[i] != b'\t' {
            return false;
        }
        while i < line.len() && (line[i] == b' ' || line[i] == b'\t') {
            i += 1;
        }
        if i >= line.len() {
            return tokens >= 2;
        }
        if line[i] != b'#' {
            return false;
        }
    }
}

/// Permissive: any line matching `<URL>` followed by optional
/// ASCII whitespace. Leading whitespace is not allowed. Used by
/// the Bookmark classifier to recognize a URL line.
pub(crate) fn is_url_line(line: &[u8]) -> bool {
    if line.len() < 3 || line[0] != b'<' {
        return false;
    }
    let Some(close) = line.iter().position(|&b| b == b'>') else {
        return false;
    };
    if close == 1 {
        return false;
    }
    for &b in &line[close + 1..] {
        if b != b' ' && b != b'\t' {
            return false;
        }
    }
    true
}

pub(crate) fn strip_leading_spaces(line: &[u8]) -> Option<&[u8]> {
    let i = line.iter().take_while(|&&b| b == b' ').count();
    line.get(i..)
}

/// Compute tag token spans within a tag-range slice of source.
///
/// Token spans include EVERY byte after the leading `#` up to
/// the next whitespace or end-of-slice. Non-ASCII bytes
/// (including invalid UTF-8) are preserved so that the fallible
/// `tags_str()` iterator can surface per-item UTF-8 errors
/// without truncating the byte sequence.
///
/// Acceptance (whether the line is recognized as a tags-prefix
/// line at all) is determined by `is_tags_prefix_line`, which is
/// stricter: only ASCII tag chars count. The token-span
/// computation here is permissive on purpose: it surfaces the
/// underlying bytes for downstream consumers, which can decide
/// whether to treat invalid-UTF-8 tokens as errors.
pub(crate) fn tag_token_spans_in(source: &[u8], range: Option<&Range<usize>>) -> Vec<Range<usize>> {
    let Some(range) = range else {
        return Vec::new();
    };
    let slice = &source[range.clone()];
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < slice.len() {
        // Skip whitespace and line terminators.
        while i < slice.len()
            && (slice[i] == b' ' || slice[i] == b'\t' || slice[i] == b'\n' || slice[i] == b'\r')
        {
            i += 1;
        }
        if i >= slice.len() {
            break;
        }
        if slice[i] != b'#' {
            i += 1;
            continue;
        }
        // Skip the leading `#`; the token span starts AFTER it.
        i += 1;
        let token_start = i;
        // Capture every byte until the next whitespace,
        // terminator, or `#` (next token's prefix). This
        // preserves invalid UTF-8 sequences for `tags_str()`.
        while i < slice.len()
            && slice[i] != b' '
            && slice[i] != b'\t'
            && slice[i] != b'\n'
            && slice[i] != b'\r'
            && slice[i] != b'#'
        {
            i += 1;
        }
        if i > token_start {
            tokens.push((range.start + token_start)..(range.start + i));
        }
    }
    tokens
}

/// True iff `line_content` is an exact unindented
/// `## Tags` / `## Content` / `## Source` (level 2, no
/// leading indent, no tab after `##`, no closing-hash
/// sequence, no trailing whitespace).
pub(crate) fn is_exact_reserved_section(line_content: &[u8], level: u8) -> bool {
    level == 2
        && (is_exact_reserved_heading_text(line_content, "Tags")
            || is_exact_reserved_heading_text(line_content, "Content")
            || is_exact_reserved_heading_text(line_content, "Source"))
}

/// True iff `line_content` is an exact unindented
/// `## Content` or `## Source` (level 2, no leading
/// indent, no tab after `##`, no closing-hash
/// sequence, no trailing whitespace).
pub(crate) fn is_exact_content_or_source_heading(line_content: &[u8], level: u8) -> bool {
    level == 2
        && (is_exact_reserved_heading_text(line_content, "Content")
            || is_exact_reserved_heading_text(line_content, "Source"))
}

/// True iff `line.content` is **exactly** a level-2 ATX
/// heading whose heading text is **exactly** `expected`
/// after stripping the `## ` prefix. No leading whitespace,
/// no tab after `##`, no closing-hash sequence, no trailing
/// whitespace — strict match against `## <expected>`. Used
/// by the Todo and Bookmark classifiers to recognize
/// reserved metadata candidates.
pub(crate) fn is_exact_reserved_heading_text(line_content: &[u8], expected: &str) -> bool {
    let prefix: &[u8] = b"## ";
    if !line_content.starts_with(prefix) {
        return false;
    }
    let rest = &line_content[prefix.len()..];
    rest == expected.as_bytes()
}

/// True if `line` is blank: zero content OR content is entirely
/// ASCII whitespace (` `, tab).
pub(crate) fn is_blank_line(line: &Line<'_>) -> bool {
    line.content.is_empty() || line.content.iter().all(|&b| b == b' ' || b == b'\t')
}

/// Detect an ATX heading level (1..=6) at `line`. Returns the
/// level on a valid heading, `None` otherwise.
pub(crate) fn atx_heading_level(line: &[u8]) -> Option<u8> {
    let mut i = 0;
    while i < line.len() && line[i] == b' ' && i < 3 {
        i += 1;
    }
    if i >= line.len() || line[i] != b'#' {
        return None;
    }
    let hash_count = {
        let start = i;
        while i < line.len() && line[i] == b'#' {
            i += 1;
        }
        i - start
    };
    if hash_count > 6 {
        return None;
    }
    // Required space, tab, or EOL after the closing hash run.
    if i >= line.len() {
        // Bare `#`/`##`/... is an H1/H2 with empty heading text
        // (per CommonMark). Note: ATX H1 requires hash_count==1.
        return if hash_count == 1 {
            Some(1)
        } else if (2..=6).contains(&hash_count) {
            // Empty heading for H2-H6.
            Some(hash_count as u8)
        } else {
            None
        };
    }
    if line[i] != b' ' && line[i] != b'\t' {
        return None;
    }
    Some(hash_count as u8)
}

/// Extract the heading text from an ATX heading line.
/// Trims trailing whitespace AND an optional closing-hash
/// sequence (e.g., `# Title #` → `Title`).
pub(crate) fn heading_text(line: &[u8], level: u8) -> String {
    let mut i = 0;
    while i < line.len() && line[i] == b' ' && i < 3 {
        i += 1;
    }
    i += level as usize;
    if i < line.len() && (line[i] == b' ' || line[i] == b'\t') {
        i += 1;
    }
    let text_start = i;
    let mut text_end = line.len();
    while text_end > text_start && (line[text_end - 1] == b' ' || line[text_end - 1] == b'\t') {
        text_end -= 1;
    }
    // Optional closing-hash sequence: strip a trailing run of `#`
    // preceded by whitespace.
    if text_end > text_start {
        let mut trailing_hashes = 0;
        let mut j = text_end;
        while j > text_start && line[j - 1] == b'#' {
            j -= 1;
            trailing_hashes += 1;
        }
        if trailing_hashes > 0 && j > text_start && (line[j - 1] == b' ' || line[j - 1] == b'\t') {
            text_end = j - 1;
            while text_end > text_start
                && (line[text_end - 1] == b' ' || line[text_end - 1] == b'\t')
            {
                text_end -= 1;
            }
        }
    }
    String::from_utf8_lossy(&line[text_start..text_end]).into_owned()
}
