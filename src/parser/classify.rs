//! Per-kind classifiers: physical lines to MarkdownToken streams.

use crate::tokenizer::Line;

use super::helpers::{
    atx_heading_level, heading_text, is_blank_line, is_exact_content_or_source_heading,
    is_exact_reserved_heading_text, is_exact_reserved_section, is_tags_prefix_line, is_url_line,
    strip_leading_spaces,
};
use super::types::{HeadingRole, MarkdownToken};

/// Run the **Note** classifier over `lines`, emitting one
/// `MarkdownToken` per line. The classifier is purely
/// lexical/structural-recognition — never canonical selection.
///
/// Note's heading taxonomy per Decision 17 Revision 4: H1 is a
/// structural title candidate (the Note assembler uses it iff
/// it is a valid ATX H1; the role carries no meaning for the
/// Note assembler). All H2+ headings are `InternalBody` —
/// Notes have no Tags/Content/Source section model, so
/// `## Tags`, `## Content`, etc. are ordinary body content.
pub(crate) fn classify_note(lines: &[Line<'_>]) -> Vec<MarkdownToken> {
    let mut tokens = Vec::with_capacity(lines.len());
    for (idx, line) in lines.iter().enumerate() {
        let line_id = idx;
        let content = line.content;
        if is_blank_line(line) {
            tokens.push(MarkdownToken::BlankLine(line_id));
            continue;
        }
        if is_tags_prefix_line(content) {
            tokens.push(MarkdownToken::TagsPrefix(line_id));
            continue;
        }
        if let Some(level) = atx_heading_level(content) {
            let text = heading_text(content, level);
            let role = if level == 1 {
                HeadingRole::SectionBoundary
            } else {
                HeadingRole::InternalBody
            };
            tokens.push(MarkdownToken::Heading {
                level,
                text,
                line_id,
                role,
            });
            continue;
        }
        tokens.push(MarkdownToken::Body(line_id));
    }
    tokens
}

/// Run the **Todo** classifier. The classifier is purely
/// lexical/structural-recognition — never canonical selection.
///
/// Todo's heading taxonomy per Decision 17 Revision 4: only
/// exact `## Tags` (level=2, no leading indent, no tab,
/// no closing-hash sequence, no trailing whitespace) is a
/// `SectionBoundary` candidate. The Todo assembler then
/// picks the last such heading as the metadata tag_section.
/// All other H2+ headings are `InternalBody`. H1 is a
/// structural title candidate (the Todo assembler takes
/// the first non-blank line regardless of H1 validity).
pub(crate) fn classify_todo(lines: &[Line<'_>]) -> Vec<MarkdownToken> {
    let mut tokens = Vec::with_capacity(lines.len());
    for (idx, line) in lines.iter().enumerate() {
        let line_id = idx;
        let content = line.content;
        if is_blank_line(line) {
            tokens.push(MarkdownToken::BlankLine(line_id));
            continue;
        }
        if let Some(level) = atx_heading_level(content) {
            let text = heading_text(content, level);
            let role = if level == 2 && is_exact_reserved_heading_text(content, "Tags") {
                HeadingRole::SectionBoundary
            } else {
                HeadingRole::InternalBody
            };
            tokens.push(MarkdownToken::Heading {
                level,
                text,
                line_id,
                role,
            });
            continue;
        }
        tokens.push(MarkdownToken::Body(line_id));
    }
    tokens
}

/// Run the **Bookmark** classifier. The classifier is purely
/// lexical/structural-recognition — never canonical selection.
///
/// Bookmark's heading taxonomy per Decision 17 Revision 4:
/// ONLY exact unindented `## Tags`, `## Content`, `## Source`
/// (level=2, no leading indent, no tab after `##`, no
/// closing-hash sequence, no trailing whitespace) are
/// `SectionBoundary` candidates. The Bookmark assembler then
/// picks canonical Tags via the dual-cursor algorithm. All
/// other H2+ headings are `InternalBody` (frozen E10.1:
/// `## Tags in body` inside Content/Source is body content).
///
/// Fence awareness: lines inside a fenced code block are
/// emitted as `Body` regardless of structural markers (per
/// frozen E10.2).
pub(crate) fn classify_bookmark(lines: &[Line<'_>]) -> Vec<MarkdownToken> {
    let mut tokens = Vec::with_capacity(lines.len());
    let mut fence_char: Option<u8> = None;
    let mut fence_run: usize = 0;
    let mut in_body_context = false;
    for (idx, line) in lines.iter().enumerate() {
        let line_id = idx;
        let content = line.content;
        if let Some(stripped) = strip_leading_spaces(content) {
            let is_backtick = stripped.starts_with(b"```");
            let is_tilde = !is_backtick && stripped.starts_with(b"~~~");
            if is_backtick || is_tilde {
                let fc = if is_backtick { b'`' } else { b'~' };
                let run = stripped.iter().take_while(|&&b| b == fc).count();
                match fence_char {
                    None => {
                        fence_char = Some(fc);
                        fence_run = run;
                    }
                    Some(open_fc) if fc == open_fc && run >= fence_run => {
                        fence_char = None;
                        fence_run = 0;
                    }
                    _ => {}
                }
                tokens.push(MarkdownToken::Body(line_id));
                continue;
            }
        }
        if fence_char.is_some() {
            tokens.push(MarkdownToken::Body(line_id));
            continue;
        }
        if is_blank_line(line) {
            tokens.push(MarkdownToken::BlankLine(line_id));
            continue;
        }
        if is_url_line(content) {
            tokens.push(MarkdownToken::Url(line_id));
            continue;
        }
        if let Some(level) = atx_heading_level(content) {
            let text = heading_text(content, level);
            let is_exact_reserved = is_exact_reserved_section(content, level);
            // Role policy for bookmark body fragmentation:
            // - Exact reserved `## Tags` / `## Content` / `## Source`:
            //   always `SectionBoundary` (regardless of context).
            // - Any other heading outside body context: `SectionBoundary`.
            // - Any other heading inside body context:
            //   `InternalBody`. This is the frozen E10.1 case
            //   (`## Tags in body` etc.).
            //
            // The canonical-Tags selection adds an exact-form
            // filter on top of the role, so `### Tags`,
            // ` ## Tags`, `##\tTags`, `## Tags ##` are
            // SectionBoundary for body fragmentation but are
            // NOT picked as canonical tag_section.
            let role = if is_exact_reserved || !in_body_context {
                HeadingRole::SectionBoundary
            } else {
                HeadingRole::InternalBody
            };
            if is_exact_content_or_source_heading(content, level) {
                in_body_context = false;
            }
            tokens.push(MarkdownToken::Heading {
                level,
                text: text.clone(),
                line_id,
                role,
            });
            if is_exact_content_or_source_heading(content, level) {
                in_body_context = true;
            }
            continue;
        }
        tokens.push(MarkdownToken::Body(line_id));
    }
    tokens
}
