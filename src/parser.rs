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

// The P1 spec mandates `body_ranges: Vec<Range<usize>>` even for
// kinds where the body is always a single contiguous fragment
// (Note and Todo). This uniformity lets `body()` return an
// iterator of fragments across all three kinds without special
// casing. Suppress the `useless_vec` and
// `single_range_in_vec_init` lints accordingly.
#![allow(clippy::useless_vec, clippy::single_range_in_vec_init)]

use std::ops::Range;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::NbError;
use crate::tokenizer::{Line, Preamble, tokenize};

// --- Format-dispatch constants (R3 revision) ---

/// Source-of-truth list of supported document extensions for
/// format dispatch. The constant is **declared in the `parser`
/// module and re-exported at the crate root** as
/// `nb_api::SUPPORTED_DOCUMENT_EXTENSIONS` per the spec.
///
/// Iteration order in this array is **not** the matching
/// precedence — the matcher sorts by length descending for the
/// longest-first precedence required by the R3 spec. The
/// owned-string form is populated from this constant at
/// `NbError::UnsupportedDocumentFormat` construction time so the
/// error's JSON round-trip is independent of internal ordering.
///
/// The list is intentionally narrow; future formats (Org,
/// AsciiDoc, LaTeX) land in P3+ as separate parser modules per
/// `nb-api:todos/format/{1,2,3}`.
pub const SUPPORTED_DOCUMENT_EXTENSIONS: &[&str] = &["md", "markdown", "todo.md", "bookmark.md"];

/// Recognized-but-unsupported dotted suffixes (R3 revision).
/// Files whose final filename ends with one of these are
/// rejected via `NbError::UnsupportedDocumentFormat` BEFORE
/// byte parsing runs. The set is disjoint from
/// [`SUPPORTED_DOCUMENT_EXTENSIONS`].
///
/// The set coincides with `nb` CLI's format recognition
/// (regexes `(\.latex$|\.tex$)`, `\.org$`,
/// `(\.adoc$|\.asciidoc$)`) — these are formats `nb` itself
/// recognizes but P1 cannot parse.
const REJECTED_DOCUMENT_EXTENSIONS: &[&str] = &["org", "latex", "tex", "adoc", "asciidoc"];

/// Result of format dispatch on `ParseContext::FromPath`.
///
/// Format dispatch precedes byte parsing per the R3 spec:
/// `Rejected(_)` short-circuits `parse` with
/// `NbError::UnsupportedDocumentFormat`; `Supported(_)` and
/// `MarkdownNote` proceed to byte parsing with the dispatched
/// `DocumentKind`.
enum FormatDispatch {
    /// Final filename matched a supported suffix; byte parsing
    /// proceeds with this kind.
    SupportedKind(DocumentKind),
    /// Final filename matched a recognized-but-unsupported
    /// suffix; `parse` returns `UnsupportedDocumentFormat`
    /// without consuming any bytes. The string is the matched
    /// lowercase dotted suffix WITHOUT the leading dot.
    Rejected(String),
    /// Final filename is absent, non-UTF-8, or matched no
    /// recognized suffix; permissive Markdown Note fallback per
    /// the spec.
    MarkdownNote,
}

/// Perform format dispatch on `path`.
///
/// The spec mandates ASCII case-insensitive matching against
/// the lowercased final filename with literal dotted
/// boundaries and longest-first precedence. The matcher
/// delegates the precedence by selecting the longest matching
/// suffix across both the supported and rejected sets rather
/// than sorting a list (the supported and rejected sets are
/// disjoint by construction, so a single pass is sufficient).
///
/// Falls through to `MarkdownNote` for:
/// - paths with no final filename component (e.g., root path);
/// - paths whose final filename is not valid UTF-8;
/// - paths whose final filename has no recognized suffix.
///
/// Neither `Explicit(DocumentKind)` nor paths that match a
/// supported suffix reach this function.
fn format_dispatch(path: &std::path::Path) -> FormatDispatch {
    let Some(name) = path.file_name() else {
        return FormatDispatch::MarkdownNote;
    };
    let Some(name) = name.to_str() else {
        return FormatDispatch::MarkdownNote;
    };
    let lower = name.to_ascii_lowercase();

    // First pass: find the LONGEST supported suffix matching
    // this filename. Multi-dot suffixes (e.g., `.bookmark.md`)
    // must beat single-dot suffixes (`.md`) so that
    // `foo.bookmark.md` resolves to Bookmark, not Note.
    let mut best_supported: Option<(usize, DocumentKind)> = None;
    for ext in SUPPORTED_DOCUMENT_EXTENSIONS {
        let suffix_len = ext.len() + 1; // include the leading dot
        if lower.len() < suffix_len {
            continue;
        }
        if !lower.ends_with(&format!(".{ext}")) {
            continue;
        }
        let kind = match *ext {
            "md" | "markdown" => DocumentKind::Note,
            "todo.md" => DocumentKind::Todo,
            "bookmark.md" => DocumentKind::Bookmark,
            _ => continue,
        };
        if best_supported.is_none() || suffix_len > best_supported.unwrap().0 {
            best_supported = Some((suffix_len, kind));
        }
    }
    if let Some((_, kind)) = best_supported {
        return FormatDispatch::SupportedKind(kind);
    }

    // Second pass: recognized-but-unsupported suffixes. We test
    // all rejected suffixes and pick the longest match. In
    // practice this matters for compound inputs like
    // `xxx.latex.tex` (would be `Rejected("tex")` if the user
    // uses an unusual filename); supported-then-rejected
    // precedence ensures no overlap.
    let mut best_rejected: Option<(usize, &str)> = None;
    for ext in REJECTED_DOCUMENT_EXTENSIONS {
        let suffix_len = ext.len() + 1;
        if lower.len() < suffix_len {
            continue;
        }
        if !lower.ends_with(&format!(".{ext}")) {
            continue;
        }
        if best_rejected.is_none() || suffix_len > best_rejected.unwrap().0 {
            best_rejected = Some((suffix_len, ext));
        }
    }
    if let Some((_, ext)) = best_rejected {
        return FormatDispatch::Rejected(ext.to_string());
    }

    FormatDispatch::MarkdownNote
}

/// Distinguishes the three document kinds recognized by the parser.
///
/// [`DocumentKind::Note`] is the default for `.md` files and any
/// path whose extension is not `.todo.md` or `.bookmark.md`.
///
/// [`DocumentKind::Todo`] is for `.todo.md` files. The Todo
/// [`TodoState`] is derived from the title line via
/// [`NoteDocument::todo_state`].
///
/// [`DocumentKind::Bookmark`] is for `.bookmark.md` files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum DocumentKind {
    Note,
    Todo,
    Bookmark,
}

/// The parsed checkbox state of a Todo title.
///
/// Surfaced via [`NoteDocument::todo_state`] as
/// `Option<TodoState>`; `None` when the title has no checkbox
/// marker (permissive acceptance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum TodoState {
    Open,
    Done,
}

/// How the parser determines the [`DocumentKind`] for an input.
///
/// [`ParseContext::FromPath`] infers from the file extension:
/// `.todo.md` → Todo, `.bookmark.md` → Bookmark, `.md` (or other)
/// → Note. The bare `.todo` extension (without `.md`) maps to
/// Note deterministically — the `.todo` extension is a `show
/// --type` classification only, not mutation-authoritative.
///
/// [`ParseContext::Explicit`] sets the kind directly, overriding
/// any inference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ParseContext {
    FromPath(PathBuf),
    Explicit(DocumentKind),
}

/// A parsed `nb` note, todo, or bookmark.
///
/// `NoteDocument` owns the complete original source as
/// `Vec<u8>`. The source is byte-identical to the input passed
/// to [`parse`]. The kind-specific ownership partition is
/// exposed via accessor methods (e.g., [`NoteDocument::title`],
/// [`NoteDocument::body`], [`NoteDocument::url`]) and the
/// [`NoteDocument::tag_token_spans`] view.
///
/// See the P1 specification for the full contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteDocument {
    source: Vec<u8>,
    partition: Partition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Partition {
    Note(NotePartition),
    Todo(TodoPartition),
    Bookmark(BookmarkPartition),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NotePartition {
    prefix_range: Range<usize>,
    title_range: Option<Range<usize>>,
    tags_prefix_range: Option<Range<usize>>,
    separator_ranges: Vec<Range<usize>>,
    body_ranges: Vec<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TodoPartition {
    prefix_range: Range<usize>,
    title_range: Option<Range<usize>>,
    separator_ranges: Vec<Range<usize>>,
    tag_section_range: Option<Range<usize>>,
    body_ranges: Vec<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BookmarkPartition {
    prefix_range: Range<usize>,
    title_range: Option<Range<usize>>,
    url_range: Option<Range<usize>>,
    separator_ranges: Vec<Range<usize>>,
    tag_section_range: Option<Range<usize>>,
    body_ranges: Vec<Range<usize>>,
}

impl NoteDocument {
    /// The original source bytes, byte-identical to the input
    /// passed to [`parse`].
    pub fn source(&self) -> &[u8] {
        &self.source
    }

    /// Returns a slice into the retained `source` bytes,
    /// byte-identical to the original input. Equivalent to
    /// [`NoteDocument::source`].
    pub fn emit(&self) -> &[u8] {
        &self.source
    }

    /// Verify the ownership partition satisfies the P1
    /// invariants: union covers `[0, source.len())` exactly,
    /// ranges are pairwise disjoint, each range is half-open
    /// and within bounds. Returns `Ok(())` on success or an
    /// `Err(&'static str)` describing the first violation.
    ///
    /// Used by integration tests and by `parse` itself when
    /// the `debug_assertions` cfg flag is enabled. Production
    /// callers do not need to invoke this; it exists to surface
    /// parser regressions during testing.
    pub fn verify_partition(&self) -> Result<(), &'static str> {
        let source_len = self.source.len();
        let mut covered: Vec<(usize, usize)> = Vec::new();
        let mut check = |r: &std::ops::Range<usize>, name: &str| -> Result<(), &'static str> {
            if r.start > r.end {
                return Err("range not half-open (start > end)");
            }
            if r.end > source_len {
                return Err("range extends past source.len()");
            }
            covered.push((r.start, r.end));
            let _ = name;
            Ok(())
        };
        match &self.partition {
            Partition::Note(note) => {
                check(&note.prefix_range, "prefix")?;
                if let Some(r) = &note.title_range {
                    check(r, "title")?;
                }
                if let Some(r) = &note.tags_prefix_range {
                    check(r, "tags_prefix")?;
                }
                for r in &note.separator_ranges {
                    check(r, "separator")?;
                }
                for r in &note.body_ranges {
                    check(r, "body")?;
                }
            }
            Partition::Todo(todo) => {
                check(&todo.prefix_range, "prefix")?;
                if let Some(r) = &todo.title_range {
                    check(r, "title")?;
                }
                for r in &todo.separator_ranges {
                    check(r, "separator")?;
                }
                if let Some(r) = &todo.tag_section_range {
                    check(r, "tag_section")?;
                }
                for r in &todo.body_ranges {
                    check(r, "body")?;
                }
            }
            Partition::Bookmark(bookmark) => {
                check(&bookmark.prefix_range, "prefix")?;
                if let Some(r) = &bookmark.title_range {
                    check(r, "title")?;
                }
                if let Some(r) = &bookmark.url_range {
                    check(r, "url")?;
                }
                for r in &bookmark.separator_ranges {
                    check(r, "separator")?;
                }
                if let Some(r) = &bookmark.tag_section_range {
                    check(r, "tag_section")?;
                }
                for r in &bookmark.body_ranges {
                    check(r, "body")?;
                }
            }
        }
        covered.sort_by_key(|&(s, _)| s);
        // Adjacency check: ranges must touch end-to-end
        // (previous.end == next.start) with no overlap AND no
        // gap. A gap means a byte was not assigned to any
        // partition region.
        for window in covered.windows(2) {
            if window[0].1 > window[1].0 {
                return Err("ranges overlap");
            }
            if window[0].1 < window[1].0 {
                return Err("partition leaves a gap (internal unassigned bytes)");
            }
        }
        if covered.first().map(|r| r.0) != Some(0) {
            return Err("partition does not start at byte 0");
        }
        if covered.last().map(|r| r.1) != Some(source_len) {
            return Err("partition does not cover to source.len()");
        }
        Ok(())
    }

    /// The [`DocumentKind`] of this document.
    pub fn kind(&self) -> DocumentKind {
        match &self.partition {
            Partition::Note(_) => DocumentKind::Note,
            Partition::Todo(_) => DocumentKind::Todo,
            Partition::Bookmark(_) => DocumentKind::Bookmark,
        }
    }

    /// For Todo documents, the parsed checkbox state. `None` for
    /// non-Todo documents and for Todo documents whose title has
    /// no `[ ]`/`[x]` prefix (permissive acceptance).
    pub fn todo_state(&self) -> Option<TodoState> {
        match &self.partition {
            Partition::Todo(todo) => {
                let title = todo.title_range.as_ref()?;
                let line = self.source.get(title.start..title.end)?;
                parse_todo_state_from_title(line)
            }
            _ => None,
        }
    }

    /// The raw title line bytes (including trailing newline);
    /// `None` if no valid ATX H1 title.
    pub fn title(&self) -> Option<&[u8]> {
        let range = self.title_range()?;
        Some(&self.source[range])
    }

    /// `Option<Result<&str, std::str::Utf8Error>>` view of the
    /// title. `None` for no title; `Some(Ok(_))` for valid UTF-8;
    /// `Some(Err(_))` for invalid UTF-8.
    pub fn title_str(&self) -> Option<Result<&str, std::str::Utf8Error>> {
        self.title().map(std::str::from_utf8)
    }

    /// For Note documents, the raw tags prefix line bytes
    /// (including trailing newline); `None` otherwise.
    pub fn tags_prefix(&self) -> Option<&[u8]> {
        let NotePartition {
            tags_prefix_range, ..
        } = match &self.partition {
            Partition::Note(note) => note,
            _ => return None,
        };
        Some(&self.source[tags_prefix_range.clone()?])
    }

    /// For Todo/Bookmark documents, the raw tags-section bytes
    /// (the entire H2 Tags section including its trailing
    /// newline); `None` otherwise.
    pub fn tag_section(&self) -> Option<&[u8]> {
        let range = self.tag_section_range()?;
        Some(&self.source[range])
    }

    /// Iterator over tag token byte spans (`&[u8]`). Tokens come
    /// from `tags_prefix_range` for Note documents and from
    /// `tag_section_range` for Todo/Bookmark documents. Spans
    /// reference bytes owned by the source.
    pub fn tags(&self) -> TagsIter<'_> {
        TagsIter {
            doc: self,
            token_spans: self.tag_token_spans().into_iter(),
        }
    }

    /// Iterator yielding `Result<&str, std::str::Utf8Error>` per
    /// tag token.
    pub fn tags_str(&self) -> TagsStrIter<'_> {
        TagsStrIter {
            doc: self,
            token_spans: self.tag_token_spans().into_iter(),
        }
    }

    /// All tag token spans across the document. For Note,
    /// tokens come from `tags_prefix_range`; for Todo/Bookmark,
    /// from `tag_section_range`. Spans reference bytes owned by
    /// the source and SHALL NOT themselves own bytes.
    pub fn tag_token_spans(&self) -> Vec<Range<usize>> {
        match &self.partition {
            Partition::Note(note) => {
                tag_token_spans_in(&self.source, note.tags_prefix_range.as_ref())
            }
            Partition::Todo(todo) => {
                tag_token_spans_in(&self.source, todo.tag_section_range.as_ref())
            }
            Partition::Bookmark(bookmark) => {
                tag_token_spans_in(&self.source, bookmark.tag_section_range.as_ref())
            }
        }
    }

    /// Iterator over body byte ranges (in source order).
    pub fn body(&self) -> BodyFragments<'_> {
        let ranges = match &self.partition {
            Partition::Note(note) => note.body_ranges.clone(),
            Partition::Todo(todo) => todo.body_ranges.clone(),
            Partition::Bookmark(bookmark) => bookmark.body_ranges.clone(),
        };
        BodyFragments {
            source: &self.source,
            ranges: ranges.into_iter(),
        }
    }

    /// For Bookmark documents, the raw URL line bytes (including
    /// trailing newline); `None` for non-Bookmark documents or
    /// Bookmarks without a `<URL>` line.
    pub fn url(&self) -> Option<&[u8]> {
        let BookmarkPartition { url_range, .. } = match &self.partition {
            Partition::Bookmark(bookmark) => bookmark,
            _ => return None,
        };
        url_range.as_ref().map(|r| &self.source[r.clone()])
    }

    /// `Option<Result<&str, std::str::Utf8Error>>` view of the
    /// URL.
    pub fn url_str(&self) -> Option<Result<&str, std::str::Utf8Error>> {
        self.url().map(std::str::from_utf8)
    }

    fn title_range(&self) -> Option<Range<usize>> {
        match &self.partition {
            Partition::Note(note) => note.title_range.clone(),
            Partition::Todo(todo) => todo.title_range.clone(),
            Partition::Bookmark(bookmark) => bookmark.title_range.clone(),
        }
    }

    fn tag_section_range(&self) -> Option<Range<usize>> {
        match &self.partition {
            Partition::Note(_) => None,
            Partition::Todo(todo) => todo.tag_section_range.clone(),
            Partition::Bookmark(bookmark) => bookmark.tag_section_range.clone(),
        }
    }
}

/// Iterator over tag token byte slices.
pub struct TagsIter<'a> {
    doc: &'a NoteDocument,
    token_spans: std::vec::IntoIter<Range<usize>>,
}

impl<'a> Iterator for TagsIter<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<Self::Item> {
        self.token_spans.next().map(|r| &self.doc.source[r])
    }
}

/// Iterator over tag token string slices, surfacing UTF-8 errors.
pub struct TagsStrIter<'a> {
    doc: &'a NoteDocument,
    token_spans: std::vec::IntoIter<Range<usize>>,
}

impl<'a> Iterator for TagsStrIter<'a> {
    type Item = Result<&'a str, std::str::Utf8Error>;
    fn next(&mut self) -> Option<Self::Item> {
        self.token_spans
            .next()
            .map(|r| std::str::from_utf8(&self.doc.source[r]))
    }
}

/// Iterator over body byte ranges.
pub struct BodyFragments<'a> {
    source: &'a [u8],
    ranges: std::vec::IntoIter<Range<usize>>,
}

impl<'a> Iterator for BodyFragments<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<Self::Item> {
        self.ranges.next().map(|r| &self.source[r])
    }
}

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
enum HeadingRole {
    SectionBoundary,
    InternalBody,
}

/// H2 section descriptor used by the bookmark assembler when
/// computing section extents. The text is the heading text
/// (trimmed) and the range is the full section extent
/// (heading + body up to the next heading or end-of-source).
#[derive(Debug, Clone)]
struct H2Section {
    text: String,
    range: Range<usize>,
    role: HeadingRole,
}

/// Index into the tokenizer's `Vec<Line>` for a given physical line.
type LineId = usize;

/// One token per physical line, emitted by the per-kind classifier.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MarkdownToken {
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
enum ParseFailure {
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
fn consume_leading_blank_intervals(source: &[u8], mut pos: usize) -> usize {
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
fn parse_todo_state_from_title(line: &[u8]) -> Option<TodoState> {
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
fn is_tags_prefix_line(line: &[u8]) -> bool {
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
fn is_url_line(line: &[u8]) -> bool {
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

fn strip_leading_spaces(line: &[u8]) -> Option<&[u8]> {
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
fn tag_token_spans_in(source: &[u8], range: Option<&Range<usize>>) -> Vec<Range<usize>> {
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

// =================================================================
// New layered pipeline (Decision 17 Revision 4) — tokenizer +
// classifier + per-kind assembler. Replaces the byte-cursor
// `consume_*` / `scan_h2_*` / `sections_from_headings` /
// `build_*_partition` helpers above. The interfaces (`parse` +
// the `NoteDocument` accessors) are unchanged.
// =================================================================

/// Half-open byte range alias used by the layered pipeline.
type ByteRange = Range<usize>;

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
fn classify_note(lines: &[Line<'_>]) -> Vec<MarkdownToken> {
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
fn classify_todo(lines: &[Line<'_>]) -> Vec<MarkdownToken> {
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
fn classify_bookmark(lines: &[Line<'_>]) -> Vec<MarkdownToken> {
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

/// True iff `line_content` is an exact unindented
/// `## Tags` / `## Content` / `## Source` (level 2, no
/// leading indent, no tab after `##`, no closing-hash
/// sequence, no trailing whitespace).
fn is_exact_reserved_section(line_content: &[u8], level: u8) -> bool {
    level == 2
        && (is_exact_reserved_heading_text(line_content, "Tags")
            || is_exact_reserved_heading_text(line_content, "Content")
            || is_exact_reserved_heading_text(line_content, "Source"))
}

/// True iff `line_content` is an exact unindented
/// `## Content` or `## Source` (level 2, no leading
/// indent, no tab after `##`, no closing-hash
/// sequence, no trailing whitespace).
fn is_exact_content_or_source_heading(line_content: &[u8], level: u8) -> bool {
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
fn is_exact_reserved_heading_text(line_content: &[u8], expected: &str) -> bool {
    let prefix: &[u8] = b"## ";
    if !line_content.starts_with(prefix) {
        return false;
    }
    let rest = &line_content[prefix.len()..];
    rest == expected.as_bytes()
}

/// True if `line` is blank: zero content OR content is entirely
/// ASCII whitespace (` `, tab).
fn is_blank_line(line: &Line<'_>) -> bool {
    line.content.is_empty() || line.content.iter().all(|&b| b == b' ' || b == b'\t')
}

/// Detect an ATX heading level (1..=6) at `line`. Returns the
/// level on a valid heading, `None` otherwise.
fn atx_heading_level(line: &[u8]) -> Option<u8> {
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
fn heading_text(line: &[u8], level: u8) -> String {
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

// =================================================================
// Per-kind assemblers (Decision 17 Revision 4)
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
fn assemble_note(
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
fn assemble_todo(
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
    // Compute full section extents (heading + body) via the
    // C4B-P1-1 blank-line terminator stripping rule.
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
fn is_todo_title_relevant(token: &MarkdownToken) -> bool {
    !matches!(token, MarkdownToken::BlankLine(_))
}

/// Bookmark assembler: builds the Bookmark partition with the
/// dual-cursor body-fragmentation algorithm. The classifier
/// emits the appropriate `Heading { role }` per line; this
/// assembler owns section extents, canonical Tags selection,
/// separator ownership, and final partition ranges.
fn assemble_bookmark(
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

    // Compute Heading section extents via the C4B-P1-1 inclusive
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
/// heading or end-of-source), with the C4B-P1-1 blank-line
/// terminator stripping rule.
fn compute_section_extents(
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

fn translate_parse_failure(failure: ParseFailure) -> NbError {
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
fn section_is_exact_tags(section: &H2Section, _source: &[u8], lines: &[Line<'_>]) -> bool {
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
