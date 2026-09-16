<!-- nbspec: change=add-note-document-model notebook=nb-api note=proposals/add-note-document-model/designs/design__note-document-model.md hash=sha256:305c771a92c8d028b04dc72985cd5b92892621f612480f85819df2c1eb8042ca -->
## Decision 16: Title accessor returns raw bytes (no display transformations)

Accessors return raw bytes by default:

- `source() -> &[u8]`
- `title() -> Option<&[u8]>` (raw title line, including trailing
  newline; NO closing-hash stripping, NO trailing whitespace
  stripping)
- `tags() -> TagsIter<'_>` (iterator over `&[u8]`)
- `body() -> BodyFragments<'_>` (iterator over body byte
  ranges)

Decoded `&str` accessors are separate convenience methods:

- `title_str() -> Option<Result<&str, std::str::Utf8Error>>`
- `tags_str() -> TagsStrIter<'_>` (iterator over
  `Result<&str, std::str::Utf8Error>`)

Display transformations (stripping closing hashes, trimming
whitespace) are the caller's responsibility.

## Decision 16a: Todo body is one contiguous range; Bookmark body is fragmented

For Todo, the body is ONE contiguous range from `pos` to the
pre-Tags position (or `source.len()` if no Tags). All internal
H2 sections and the blanks between them are part of the body
range. The blank immediately before the last H2 Tags is a
separator.

For Bookmark, the body is fragmented at the canonical Tags
position. Each non-Tags H2 section contributes a body fragment
that includes the H2 heading, the blank after, the body text
after, and the trailing newline. Blank lines between fragments
are separators.

The unified algorithm: in the selected-Tags branch, FIRST
emit `prev_end..section.range.start` as a separator, then
advance `prev_end` past Tags. Apply analogous post-Tags
separator handling before the next body section.
