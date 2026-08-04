//! Public document model: kinds, partitions, accessors, iterators.

use std::ops::Range;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::helpers::{parse_todo_state_from_title, tag_token_spans_in};

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
    pub(crate) source: Vec<u8>,
    pub(crate) partition: Partition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Partition {
    Note(NotePartition),
    Todo(TodoPartition),
    Bookmark(BookmarkPartition),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotePartition {
    pub(crate) prefix_range: Range<usize>,
    pub(crate) title_range: Option<Range<usize>>,
    pub(crate) tags_prefix_range: Option<Range<usize>>,
    pub(crate) separator_ranges: Vec<Range<usize>>,
    pub(crate) body_ranges: Vec<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TodoPartition {
    pub(crate) prefix_range: Range<usize>,
    pub(crate) title_range: Option<Range<usize>>,
    pub(crate) separator_ranges: Vec<Range<usize>>,
    pub(crate) tag_section_range: Option<Range<usize>>,
    pub(crate) body_ranges: Vec<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BookmarkPartition {
    pub(crate) prefix_range: Range<usize>,
    pub(crate) title_range: Option<Range<usize>>,
    pub(crate) url_range: Option<Range<usize>>,
    pub(crate) separator_ranges: Vec<Range<usize>>,
    pub(crate) tag_section_range: Option<Range<usize>>,
    pub(crate) body_ranges: Vec<Range<usize>>,
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
