//! Private pipeline types for the layered parser.

use std::ops::Range;

// =================================================================
// Private types for the layered pipeline (Decision 17 Revision 4)
// =================================================================

/// Per Decision 17 Revision 3, an H2-like heading inside a
/// Bookmark Content/Source body context can play one of two
/// roles:
///
/// - `SectionBoundary` — the heading opens a new top-level
///   section (`## Content`, `## Source`) or carries metadata
///   (`## Tags`); the body-fragmentation algorithm demarcates
///   a fragment at this position.
/// - `InternalBody` — the heading appears inside an existing
///   Content/Source body (e.g., frozen E10.1's `## Tags in
///   body`) and is treated as opaque body content; the
///   body-fragmentation algorithm absorbs it into the open
///   body fragment.
///
/// Per Decision 17 Revision 4, the classifier tags each
/// heading with a `HeadingRole`; the assembler is the only
/// component that consumes the role. The classifier never
/// selects canonical metadata (e.g., "which `## Tags` is the
/// canonical one"); the assembler does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeadingRole {
    SectionBoundary,
    InternalBody,
}

/// H2 section descriptor used by the bookmark assembler when
/// computing section extents. The text is the heading text
/// (trimmed) and the range is the full section extent
/// (heading + body up to the next heading or end-of-source).
#[derive(Debug, Clone)]
pub(crate) struct H2Section {
    pub(crate) text: String,
    pub(crate) range: Range<usize>,
    pub(crate) role: HeadingRole,
}

/// Index into the tokenizer's `Vec<Line>` for a given physical line.
pub(crate) type LineId = usize;

/// One token per physical line, emitted by the per-kind classifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarkdownToken {
    BlankLine(LineId),
    Heading {
        level: u8,
        text: String,
        line_id: LineId,
        role: HeadingRole,
    },
    TagsPrefix(LineId),
    Url(LineId),
    Body(LineId),
}

/// Narrow private parse failure. Translated to `NbError` once at
/// the public boundary in [`parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseFailure {
    MissingTitle,
}

/// Walk past leading blank-line intervals (whitespace-only lines
/// or bare line terminators) starting at `pos`, without pushing
/// any separator ranges. Returns the offset of the first
/// non-blank content (or `source.len()` if the source ends with
/// blank lines).
///
/// Used by the Bookmark assembler's tail handling: when the last
/// processed heading is Tags, the trailing content is split
/// into leading blank-line intervals (separators) and the
/// remaining body. This helper returns the boundary without
/// pushing separators (the caller does that).
/// Half-open byte range alias used by the layered pipeline.
pub(crate) type ByteRange = Range<usize>;
