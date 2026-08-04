//! Per-kind assemblers and parse-failure translation.

use crate::error::NbError;
use crate::tokenizer::{Line, Preamble};

use super::document::{BookmarkPartition, NotePartition, TodoPartition};
use super::helpers::{consume_leading_blank_intervals, is_exact_reserved_heading_text};
use super::types::{ByteRange, H2Section, HeadingRole, MarkdownToken, ParseFailure};

// =================================================================
// Per-kind assemblers
// =================================================================

/// Note assembler: builds the Note partition from the classified
/// token stream.
///
/// Walk stages:
/// 1. **Before header**: each BlankLine is a separator; the
///    first Heading{level=1,...} is `title_range`; the first
///    TagsPrefix is `tags_prefix_range`; any other token
///    transitions directly into body.
/// 2. **Between title and tags (or body)**: BlankLines are
///    separators; TagsPrefix is `tags_prefix_range`; else
///    transition to body.
/// 3. **In body**: all subsequent tokens extend `body_end`. The
///    body is a single contiguous fragment from the first
///    in-body byte to the source end.
#[allow(clippy::useless_vec, clippy::single_range_in_vec_init)] // body_ranges is Vec by partition contract
pub(crate) fn assemble_note(
    source: &[u8],
    preamble: &Preamble,
    lines: &[Line<'_>],
    tokens: &[MarkdownToken],
) -> Result<NotePartition, ParseFailure> {
    let prefix_range = preamble.range.clone();
    let pos_after_prefix = preamble.start_of_lines();
    debug_assert!(pos_after_prefix <= source.len());

    #[derive(PartialEq)]
    enum Stage {
        BeforeHeader,
        AfterTitle,
        AfterTags,
        InBody,
    }

    let mut stage = Stage::BeforeHeader;
    let mut separator_ranges: Vec<ByteRange> = Vec::new();
    let mut title_range: Option<ByteRange> = None;
    let mut tags_prefix_range: Option<ByteRange> = None;
    let mut body_start: Option<usize> = None;
    let mut body_end: usize = 0;

    for token in tokens {
        let token_range: ByteRange = match token {
            MarkdownToken::BlankLine(line_id)
            | MarkdownToken::TagsPrefix(line_id)
            | MarkdownToken::Url(line_id)
            | MarkdownToken::Body(line_id) => lines[*line_id].range.clone(),
            MarkdownToken::Heading { line_id, .. } => lines[*line_id].range.clone(),
        };
        match (&stage, token) {
            (Stage::BeforeHeader, MarkdownToken::BlankLine(_)) => {
                separator_ranges.push(token_range.clone());
            }
            (Stage::BeforeHeader, MarkdownToken::Heading { level: 1, .. }) => {
                title_range = Some(token_range);
                stage = Stage::AfterTitle;
            }
            (Stage::BeforeHeader, MarkdownToken::TagsPrefix(_)) => {
                tags_prefix_range = Some(token_range);
                stage = Stage::AfterTags;
            }
            (Stage::BeforeHeader, _) => {
                body_start = Some(token_range.start);
                body_end = token_range.end;
                stage = Stage::InBody;
            }
            (Stage::AfterTitle, MarkdownToken::BlankLine(_)) => {
                separator_ranges.push(token_range.clone());
            }
            (Stage::AfterTitle, MarkdownToken::TagsPrefix(_)) => {
                tags_prefix_range = Some(token_range);
                stage = Stage::AfterTags;
            }
            (Stage::AfterTitle, _) => {
                body_start = Some(token_range.start);
                body_end = token_range.end;
                stage = Stage::InBody;
            }
            (Stage::AfterTags, MarkdownToken::BlankLine(_)) => {
                separator_ranges.push(token_range.clone());
            }
            (Stage::AfterTags, _) => {
                body_start = Some(token_range.start);
                body_end = token_range.end;
                stage = Stage::InBody;
            }
            (Stage::InBody, _) => {
                body_end = token_range.end;
            }
        }
    }
    let body_ranges: Vec<ByteRange> = match body_start {
        Some(start) => vec![start..body_end],
        None => Vec::new(),
    };
    Ok(NotePartition {
        prefix_range,
        title_range,
        tags_prefix_range,
        separator_ranges,
        body_ranges,
    })
}

/// Todo assembler: builds the Todo partition. The Todo title is
/// the FIRST non-blank line (regardless of H1 validity). The
/// Todo `tag_section_range` is the last H2 Tags section, set
/// only when the final H2 itself is Tags.
#[allow(clippy::useless_vec, clippy::single_range_in_vec_init)] // body_ranges is Vec by partition contract
pub(crate) fn assemble_todo(
    source: &[u8],
    preamble: &Preamble,
    lines: &[Line<'_>],
    tokens: &[MarkdownToken],
) -> Result<TodoPartition, ParseFailure> {
    let prefix_range = preamble.range.clone();

    #[derive(PartialEq)]
    enum Stage {
        BeforeTitle,
        AfterTitle,
        InBody,
    }

    let mut stage = Stage::BeforeTitle;
    let mut separator_ranges: Vec<ByteRange> = Vec::new();
    let mut title_range: Option<ByteRange> = None;
    let mut body_start: Option<usize> = None;
    let mut body_end: usize = 0;
    let mut sections: Vec<(ByteRange, String, HeadingRole)> = Vec::new();

    for token in tokens {
        let token_range: ByteRange = match token {
            MarkdownToken::BlankLine(line_id)
            | MarkdownToken::TagsPrefix(line_id)
            | MarkdownToken::Url(line_id)
            | MarkdownToken::Body(line_id) => lines[*line_id].range.clone(),
            MarkdownToken::Heading {
                line_id,
                text,
                role,
                ..
            } => {
                sections.push((lines[*line_id].range.clone(), text.clone(), *role));
                lines[*line_id].range.clone()
            }
        };
        match (&stage, token) {
            (Stage::BeforeTitle, _) if is_todo_title_relevant(token) => {
                stage = Stage::AfterTitle;
                title_range = Some(token_range);
            }
            (Stage::BeforeTitle, MarkdownToken::BlankLine(_)) => {
                separator_ranges.push(token_range.clone());
            }
            (Stage::BeforeTitle, _) => {
                // Empty source for Todo.
                return Err(ParseFailure::MissingTitle);
            }
            (Stage::AfterTitle, MarkdownToken::BlankLine(_)) => {
                separator_ranges.push(token_range.clone());
            }
            (Stage::AfterTitle, _) => {
                stage = Stage::InBody;
                body_start = Some(token_range.start);
                body_end = token_range.end;
            }
            (Stage::InBody, _) => {
                body_end = token_range.end;
            }
        }
    }
    let _ = stage; // suppress unused mutation warning
    let _ = source;

    // Refusal contract: empty source (no non-blank line at all).
    // The unified classifier returns no tokens for empty source;
    // we still need to enforce the refusal.
    if title_range.is_none() && preamble.start_of_lines() >= source.len() {
        return Err(ParseFailure::MissingTitle);
    }
    // Compute full section extents (heading + body). A trailing
    // blank-line terminator is stripped from a section only when a
    // complete blank line precedes the next heading.
    let h2_sections: Vec<H2Section> = compute_section_extents(source, &sections);
    // Compute the tag_section_range. Per the spec, the
    // `tag_section_range` SHALL be set only when the FINAL H2
    // itself is Tags — not when ANY H2 happens to be Tags.
    // The role check enforces the exact-form rule: role is
    // SectionBoundary ONLY for the literal exact `## Tags`
    // (no leading indent, no tab after `##`, no closing
    // hashes, no trailing whitespace). Non-exact headings
    // such as `### Tags`, ` ## Tags`, `##\tTags`,
    // `## Tags ##` carry `role = InternalBody` from
    // `classify_todo` and cannot be canonical terminal Tags.
    let tag_section_range: Option<ByteRange> = h2_sections
        .last()
        .filter(|sec| sec.text == "Tags" && sec.role == HeadingRole::SectionBoundary)
        .map(|sec| sec.range.clone());
    let body_ranges = if let Some(tag) = &tag_section_range {
        if let Some(bs) = body_start {
            if bs < tag.start {
                // Walk back over a COMPLETE blank line (CRLF or LF
                // or CR) immediately before tag.start. Mirrors the
                // build_todo_partition logic.
                let mut sep_start = tag.start;
                if sep_start > bs && source[sep_start - 1] == b'\n' {
                    sep_start -= 1;
                    if sep_start > bs && source[sep_start - 1] == b'\r' {
                        sep_start -= 1;
                    }
                } else if sep_start > bs && source[sep_start - 1] == b'\r' {
                    sep_start -= 1;
                }
                let is_complete_blank_line = sep_start < tag.start
                    && (sep_start == bs
                        || (sep_start > 0
                            && (source[sep_start - 1] == b'\n' || source[sep_start - 1] == b'\r')));
                if is_complete_blank_line {
                    separator_ranges.push(sep_start..tag.start);
                    if bs < sep_start {
                        vec![bs..sep_start]
                    } else {
                        Vec::new()
                    }
                } else {
                    vec![bs..tag.start]
                }
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    } else if let Some(start) = body_start {
        vec![start..body_end]
    } else {
        Vec::new()
    };
    Ok(TodoPartition {
        prefix_range,
        title_range,
        separator_ranges,
        tag_section_range,
        body_ranges,
    })
}

/// True if `token` should be considered a candidate Todo title
/// (i.e., the first non-blank line of any kind).
pub(crate) fn is_todo_title_relevant(token: &MarkdownToken) -> bool {
    !matches!(token, MarkdownToken::BlankLine(_))
}

/// Bookmark assembler: builds the Bookmark partition with the
/// dual-cursor body-fragmentation algorithm. The classifier
/// emits the appropriate `Heading { role }` per line; this
/// assembler owns section extents, canonical Tags selection,
/// separator ownership, and final partition ranges.
pub(crate) fn assemble_bookmark(
    source: &[u8],
    preamble: &Preamble,
    lines: &[Line<'_>],
    tokens: &[MarkdownToken],
) -> Result<BookmarkPartition, ParseFailure> {
    let prefix_range = preamble.range.clone();
    let pos_after_prefix = preamble.start_of_lines();

    #[derive(PartialEq)]
    enum Stage {
        BeforeTitle,
        AfterTitle,
        AfterUrl,
    }

    let mut stage = Stage::BeforeTitle;
    let mut metadata_done = false;
    let mut separator_ranges: Vec<ByteRange> = Vec::new();
    let mut title_range: Option<ByteRange> = None;
    let mut url_range: Option<ByteRange> = None;
    let mut cursor_after_metadata: usize = pos_after_prefix;
    let mut body_ranges: Vec<ByteRange> = Vec::new();
    let mut collected_headings: Vec<(ByteRange, String, HeadingRole)> = Vec::new();

    // Walk tokens for title + URL + build heading list. Body
    // fragmentation runs after this initial pass against the
    // collected headings + the post-metadata cursor.
    for token in tokens {
        let token_range: ByteRange = match token {
            MarkdownToken::BlankLine(line_id)
            | MarkdownToken::TagsPrefix(line_id)
            | MarkdownToken::Url(line_id)
            | MarkdownToken::Body(line_id) => lines[*line_id].range.clone(),
            MarkdownToken::Heading {
                level,
                line_id,
                text,
                role,
            } => {
                // Bookmark sections are H2+. H1 titles are metadata,
                // not section starts.
                if *level >= 2 {
                    collected_headings.push((lines[*line_id].range.clone(), text.clone(), *role));
                }
                lines[*line_id].range.clone()
            }
        };
        if !metadata_done {
            match (&stage, token) {
                (Stage::BeforeTitle, MarkdownToken::BlankLine(_)) => {
                    separator_ranges.push(token_range.clone());
                    cursor_after_metadata = token_range.end;
                }
                (Stage::BeforeTitle, MarkdownToken::Heading { level: 1, .. }) => {
                    title_range = Some(token_range.clone());
                    cursor_after_metadata = token_range.end;
                    stage = Stage::AfterTitle;
                }
                (Stage::BeforeTitle, MarkdownToken::Url(_)) => {
                    // Titleless Bookmark: URL is the first non-blank
                    // line. Set url_range and treat as AfterUrl.
                    url_range = Some(token_range.clone());
                    cursor_after_metadata = token_range.end;
                    stage = Stage::AfterUrl;
                }
                (Stage::BeforeTitle, _) => {
                    // No valid H1 title. cursor_after_metadata =
                    // token.start (first non-blank).
                    cursor_after_metadata = token_range.start;
                    metadata_done = true;
                }
                (Stage::AfterTitle, MarkdownToken::BlankLine(_)) => {
                    separator_ranges.push(token_range.clone());
                    cursor_after_metadata = token_range.end;
                }
                (Stage::AfterTitle, MarkdownToken::Url(_)) => {
                    url_range = Some(token_range.clone());
                    cursor_after_metadata = token_range.end;
                    stage = Stage::AfterUrl;
                }
                (Stage::AfterTitle, _) => {
                    // No URL. cursor_after_metadata = token.start.
                    cursor_after_metadata = token_range.start;
                    metadata_done = true;
                }
                (Stage::AfterUrl, MarkdownToken::BlankLine(_)) => {
                    separator_ranges.push(token_range.clone());
                    cursor_after_metadata = token_range.end;
                }
                (Stage::AfterUrl, _) => {
                    // First non-blank after URL: H2/body region
                    // starts here.
                    cursor_after_metadata = token_range.start;
                    metadata_done = true;
                }
            }
        }
    }
    let _ = stage; // suppress unused warning

    // Refusal contract: empty source (no non-blank line at all).
    // The unified classifier returns no tokens for empty source;
    // the bookmark refusal kicks in when pos >= source.len() AND
    // no title/url has been collected.
    if title_range.is_none()
        && url_range.is_none()
        && collected_headings.is_empty()
        && pos_after_prefix >= source.len()
    {
        return Err(ParseFailure::MissingTitle);
    }

    // Compute heading section extents via the inclusive
    // contract: a trailing blank-line terminator is stripped
    // only when the preceding byte is also a line terminator.
    let sections: Vec<H2Section> = compute_section_extents(source, &collected_headings);

    // Canonical Tags selection: first H2 Tags (with the
    // exact-form rule) before any Content/Source, else the
    // last H2 Tags (exact-form rule). Non-exact forms such as
    // `### Tags`, ` ## Tags`, `##\tTags`, `## Tags ##` are
    // body content (or H3+ InternalBody) and are NOT picked
    // even when their normalized heading text is `Tags`.
    let mut canonical_tags_idx: Option<usize> = None;
    for (i, section) in sections.iter().enumerate() {
        if section_is_exact_tags(section, source, lines) {
            let has_later_content_source = sections[i + 1..]
                .iter()
                .any(|s| s.text == "Content" || s.text == "Source");
            if has_later_content_source {
                canonical_tags_idx = Some(i);
                break;
            }
        }
    }
    if canonical_tags_idx.is_none() {
        for (i, section) in sections.iter().enumerate().rev() {
            if section_is_exact_tags(section, source, lines) {
                canonical_tags_idx = Some(i);
                break;
            }
        }
    }
    let tag_section_range = canonical_tags_idx.map(|i| sections[i].range.clone());

    // Body fragmentation: dual-cursor algorithm mirroring
    // build_bookmark_partition.
    let mut cursor = cursor_after_metadata;
    let mut open_body: Option<ByteRange> = None;
    let mut seen_section = false;

    let flush = |open_body: &mut Option<ByteRange>, body_ranges: &mut Vec<ByteRange>| {
        if let Some(r) = open_body.take()
            && r.start < r.end
        {
            body_ranges.push(r);
        }
    };

    for (i, section) in sections.iter().enumerate() {
        debug_assert!(cursor <= section.range.start);
        let gap = cursor..section.range.start;
        if Some(i) == canonical_tags_idx {
            flush(&mut open_body, &mut body_ranges);
            if gap.start < gap.end {
                separator_ranges.push(gap);
            }
            cursor = section.range.end;
            seen_section = true;
            continue;
        }
        if section.role == HeadingRole::InternalBody {
            let body = open_body
                .as_mut()
                .expect("inner-body H2 without an open body fragment");
            debug_assert_eq!(body.end, cursor);
            body.end = section.range.end;
            cursor = section.range.end;
            seen_section = true;
            continue;
        }
        flush(&mut open_body, &mut body_ranges);
        if !seen_section && gap.start < gap.end {
            open_body = Some(gap.start..section.range.end);
        } else {
            if gap.start < gap.end {
                separator_ranges.push(gap);
            }
            open_body = Some(section.range.clone());
        }
        cursor = section.range.end;
        seen_section = true;
    }

    if sections.is_empty() {
        if cursor < source.len() {
            body_ranges.push(cursor..source.len());
        }
    } else if let Some(body) = open_body.as_mut() {
        body.end = source.len();
        flush(&mut open_body, &mut body_ranges);
    } else if cursor < source.len() {
        let tail_body_start = consume_leading_blank_intervals(source, cursor);
        if cursor < tail_body_start {
            separator_ranges.push(cursor..tail_body_start);
        }
        if tail_body_start < source.len() {
            body_ranges.push(tail_body_start..source.len());
        }
    }

    Ok(BookmarkPartition {
        prefix_range,
        title_range,
        url_range,
        separator_ranges,
        tag_section_range,
        body_ranges,
    })
}

/// Compute H2 section extents (heading + content up to next
/// heading or end-of-source), with the blank-line
/// terminator stripping rule.
pub(crate) fn compute_section_extents(
    source: &[u8],
    headings: &[(ByteRange, String, HeadingRole)],
) -> Vec<H2Section> {
    let mut sections: Vec<H2Section> = Vec::with_capacity(headings.len());
    for (idx, (heading_range, text, role)) in headings.iter().enumerate() {
        let next_start = headings.get(idx + 1).map(|(r, _, _)| r.start);
        let end = match next_start {
            Some(next) => {
                let mut end = next;
                if end > heading_range.end && end <= source.len() {
                    let last = source[end - 1];
                    if last == b'\n' {
                        let is_crlf = end >= 2 && source[end - 2] == b'\r';
                        let (term_len, check_pos) = if is_crlf {
                            (2usize, end - 3)
                        } else {
                            (1usize, end - 2)
                        };
                        let prev_is_term = if check_pos >= heading_range.end {
                            let prev = source[check_pos];
                            prev == b'\n' || prev == b'\r'
                        } else {
                            true
                        };
                        if prev_is_term {
                            end -= term_len;
                        }
                    } else if last == b'\r' {
                        let prev_is_term = if end >= heading_range.end + 2 {
                            let prev = source[end - 2];
                            prev == b'\n' || prev == b'\r'
                        } else {
                            true
                        };
                        if prev_is_term {
                            end -= 1;
                        }
                    }
                }
                end
            }
            None => source.len(),
        };
        sections.push(H2Section {
            text: text.clone(),
            range: heading_range.start..end,
            role: *role,
        });
    }
    sections
}

// =================================================================
// Public boundary: translate `ParseFailure` → `NbError` once.
// =================================================================

pub(crate) fn translate_parse_failure(failure: ParseFailure) -> NbError {
    match failure {
        ParseFailure::MissingTitle => NbError::ParseError {
            kind: crate::error::ParseErrorKind::MissingTitle,
            location: 0..0,
        },
    }
}

/// True iff the line whose heading is `section.range.start` is
/// **exactly** `## Tags` (no leading indent, no tab after `##`,
/// no closing hashes, no trailing whitespace). This is the
/// canonical-Tags candidate rule — the heading TEXT alone is
/// not enough because `### Tags`, ` ## Tags`, `##\tTags`,
/// `## Tags ##` all normalize to `Tags` but are NOT canonical.
pub(crate) fn section_is_exact_tags(
    section: &H2Section,
    _source: &[u8],
    lines: &[Line<'_>],
) -> bool {
    if section.text != "Tags" {
        return false;
    }
    let heading_line = lines
        .iter()
        .find(|line| line.range.start == section.range.start);
    let Some(heading_line) = heading_line else {
        return false;
    };
    is_exact_reserved_heading_text(heading_line.content, "Tags")
}
