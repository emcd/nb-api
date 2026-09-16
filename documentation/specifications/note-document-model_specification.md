<!-- nbspec: change=add-note-document-model notebook=nb-api note=proposals/add-note-document-model/specifications/note-document-model_specification.md hash=sha256:7e343a29631e9fb044448452d9d77a12f66328a60b8bec7567e43ee64e9fa0d8 -->
# Note document model and fingerprint scheme specification

## ADDED Requirements

### Requirement: NoteDocument SHALL own lossless source bytes

The `NoteDocument` type SHALL own the complete original source as
`source: Vec<u8>`. The source SHALL be byte-identical to the bytes
passed to `parse`.

#### Scenario: Source preserved

GIVEN bytes B passed to `parse`

THEN `doc.source() == B`.

### Requirement: NoteDocument SHALL have a kind-specific ownership partition

The `NoteDocument` type SHALL expose the following typed half-open
byte ranges as private fields, **kind-specific**:

For `kind = Note`:

- `prefix_range: Range<usize>` — leading UTF-8 BOM (typically 0
  or 3 bytes); no whitespace.
- `title_range: Option<Range<usize>>` — the ATX H1 line
  (including trailing newline); `None` if the first non-blank
  line is not a valid ATX H1.
- `tags_prefix_range: Option<Range<usize>>` — a tags-only line
  (`#tag1 #tag2`, including trailing newline); `None` if no
  such line.
- `separator_ranges: Vec<Range<usize>>` — blank-line separator(s).
- `body_ranges: Vec<Range<usize>>` — body content (single
  fragment for Note).

For `kind = Todo`:

- `prefix_range: Range<usize>` — leading UTF-8 BOM.
- `title_range: Option<Range<usize>>` — the first non-blank line
  (including trailing newline); `None` only if there is no
  non-blank line at all.
- `separator_ranges: Vec<Range<usize>>` — blank-line separator(s).
- `tag_section_range: Option<Range<usize>>` — terminal Tags header
  section (the final H2 itself has heading text "Tags",
  including trailing newline); `None` otherwise.
- `body_ranges: Vec<Range<usize>>` — body content (single
  contiguous fragment from `pos` to the pre-Tags position or
  `source.len()`).

For `kind = Bookmark`:

- `prefix_range: Range<usize>` — leading UTF-8 BOM.
- `title_range: Option<Range<usize>>` — the ATX H1 line
  (including trailing newline); `None` if no valid H1.
- `url_range: Option<Range<usize>>` — the URL line (including
  trailing newline); `None` if no `<URL>` line.
- `separator_ranges: Vec<Range<usize>>` — blank-line separator(s).
- `tag_section_range: Option<Range<usize>>` — Tags header section
  at the canonical position (before Content/Source if present,
  including trailing newline); `None` if no Tags.
- `body_ranges: Vec<Range<usize>>` — body fragments (one per
  non-Tags H2 section, plus post-Tags content).

All ranges within a kind SHALL be pairwise disjoint and their union
SHALL equal `0..source.len()`. Each metadata region (title,
tags_prefix, url, tag_section) INCLUDES its own trailing
newline; blank lines BETWEEN regions are in
`separator_ranges`.

#### Scenario: Partition covers source exactly for Note

GIVEN a parsed Note of source bytes S

THEN the union of `prefix_range`, `title_range` (if present),
`tags_prefix_range` (if present), `separator_ranges`, and
`body_ranges` SHALL equal `0..S.len()` with no overlaps and no
gaps.

#### Scenario: Partition covers source exactly for Todo

GIVEN a parsed Todo of source bytes S

THEN the union of `prefix_range`, `title_range` (if present),
`separator_ranges`, `tag_section_range` (if present), and
`body_ranges` SHALL equal `0..S.len()` with no overlaps and no
gaps.

#### Scenario: Partition covers source exactly for Bookmark

GIVEN a parsed Bookmark of source bytes S

THEN the union of `prefix_range`, `title_range` (if present),
`url_range` (if present), `separator_ranges`,
`tag_section_range` (if present), and `body_ranges` SHALL
equal `0..S.len()` with no overlaps and no gaps.

### Requirement: Per-kind cursor construction algorithm

The partition SHALL be constructed by per-kind cursor
algorithms that advance monotonically and emit every exact
interval exactly once. Each metadata region (title, tags_prefix,
url, tag_section) emits its full content INCLUDING the trailing
newline; blank lines between regions are emitted as
`separator_ranges` intervals.

#### Note cursor pseudocode

```
pos = 0
if source starts with UTF-8 BOM:
    prefix_range = 0..BOM_LEN  (3 for UTF-8 BOM; 0 otherwise)
    pos = BOM_LEN
# Consume leading blank lines (before title candidate)
while pos < source.len() and source[pos] is blank-line-terminated:
    next_blank = position of byte after this blank line
    separator_ranges.push(pos..next_blank)
    pos = next_blank
# Title candidate: first non-blank line
if pos < source.len():
    line_end = position after first line (pos + line_length + 1)
    if is_valid_atx_h1(source[pos..line_end]):
        title_range = Some(pos..line_end)
        pos = line_end
# Consume blank lines BETWEEN title and tags prefix
while pos < source.len() and source[pos] is blank-line-terminated:
    next_blank = position of byte after this blank line
    separator_ranges.push(pos..next_blank)
    pos = next_blank
# Tags prefix candidate: next non-blank line (only if it matches
# the prefix-tag pattern)
if pos < source.len():
    line_end = position after this line
    if source[pos..line_end] matches "^#[a-zA-Z0-9_-]+(\s+#[a-zA-Z0-9_-]+)+\s*$":
        # The tags line itself, including its trailing newline
        tags_prefix_range = Some(pos..line_end)
        pos = line_end
# Consume remaining blank lines
while pos < source.len() and source[pos] is blank-line-terminated:
    next_blank = position of byte after this blank line
    separator_ranges.push(pos..next_blank)
    pos = next_blank
# Body: everything remaining
if pos < source.len():
    body_ranges.push(pos..source.len())
```

#### Scenario: Note cursor visits every byte exactly once

GIVEN a Note's source bytes S

WHEN `parse(S, FromPath("x.md"))` is called

THEN the Note cursor visits each byte exactly once and assigns it
to exactly one ownership range (prefix, title, tags_prefix,
separator, or body).

#### Todo cursor pseudocode

```
pos = 0
if source starts with UTF-8 BOM:
    prefix_range = 0..BOM_LEN
    pos = BOM_LEN
# Consume leading blank lines
while pos < source.len() and source[pos] is blank-line-terminated:
    next_blank = position of byte after this blank line
    separator_ranges.push(pos..next_blank)
    pos = next_blank
# Title: first non-blank line (required for Todo)
if pos < source.len():
    line_end = position after this line
    title_range = Some(pos..line_end)
    pos = line_end
else:
    return Err(ParseError { kind: MissingTitle, location: 0..0 })
# Consume blank lines
while pos < source.len() and source[pos] is blank-line-terminated:
    next_blank = position of byte after this blank line
    separator_ranges.push(pos..next_blank)
    pos = next_blank
# Scan H2 sections
h2_sections = scan_h2_sections_at_or_after(pos)
# Identify the final H2. If the final H2 itself has heading text
# "Tags", it is the metadata tag_section. Otherwise, no metadata
# tag section.
tag_section_range = None
if let Some(last) = h2_sections.last():
    if last.text == "Tags":
        tag_section_range = Some(last.range.clone())
# Build body and separator ranges.
# Todo body is ONE contiguous range from pos to (pre-Tags position
# or end of file). Internal H2 sections and the blanks between them
# are part of the body range (not separate fragments); only the
# blank immediately before Tags is sep.
if let Some(tag) = tag_section_range.clone() {
    # The pre-Tags blank is at position (tag.start - 1) for canonical output
    body_end = tag.start - 1
    # The pre-Tags blank is a separator
    separator_ranges.push(body_end..tag.start)
} else {
    body_end = source.len()
}
body_ranges = [pos..body_end]
```

#### Scenario: Todo cursor visits every byte exactly once

GIVEN a Todo's source bytes S

WHEN `parse(S, FromPath("x.todo.md"))` is called

THEN the Todo cursor visits each byte exactly once.

#### Bookmark cursor pseudocode

```
pos = 0
if source starts with UTF-8 BOM:
    prefix_range = 0..BOM_LEN
    pos = BOM_LEN
# Consume leading blank lines
while pos < source.len() and source[pos] is blank-line-terminated:
    next_blank = position of byte after this blank line
    separator_ranges.push(pos..next_blank)
    pos = next_blank
# Title: first non-blank line if valid ATX H1 (else None)
if pos < source.len():
    line_end = position after this line
    if is_valid_atx_h1(source[pos..line_end]):
        title_range = Some(pos..line_end)
        pos = line_end
# Consume blank lines BETWEEN title and URL
while pos < source.len() and source[pos] is blank-line-terminated:
    next_blank = position of byte after this blank line
    separator_ranges.push(pos..next_blank)
    pos = next_blank
# URL: first non-blank line after title (if matches <URL> pattern)
if pos < source.len():
    line_end = position after this line
    if source[pos..line_end] matches "^<[^>]+>\s*$":
        url_range = Some(pos..line_end)
        pos = line_end
# Consume blank lines
while pos < source.len() and source[pos] is blank-line-terminated:
    next_blank = position of byte after this blank line
    separator_ranges.push(pos..next_blank)
    pos = next_blank
# Scan H2 sections with fence awareness (respect ``` ``` ``` ``` fences
# inside Source sections)
h2_sections = scan_h2_sections_at_or_after_with_fence_awareness(pos)
# Find canonical Tags: first H2 Tags before first Content or Source
tag_section_range = None
canonical_tags_idx = None
for (i, section) in h2_sections.iter().enumerate():
    if section.text == "Tags":
        # Check if any later section is Content or Source
        has_later_content_source = h2_sections[i+1..].iter()
            .any(|s| s.text == "Content" || s.text == "Source")
        if has_later_content_source:
            tag_section_range = Some(section.range.clone())
            canonical_tags_idx = Some(i)
            break
if tag_section_range.is_none():
    # If no Tags before Content/Source, the LAST H2 Tags is canonical
    for (i, section) in h2_sections.iter().enumerate().rev():
        if section.text == "Tags":
            tag_section_range = Some(section.range.clone())
            canonical_tags_idx = Some(i)
            break
# Build body fragments and separator ranges.
# Use after_selected_tags state so the immediate next gap after
# the selected Tags is a separator, not body. Before Tags (or
# with no Tags at all), gaps between non-Tags H2 sections are part
# of the body fragment.
prev_end = pos
after_selected_tags = false
for (i, section) in h2_sections.iter().enumerate():
    if Some(i) == canonical_tags_idx:
        # Selected Tags: emit pre-Tags separator
        if prev_end < section.range.start:
            separator_ranges.push(prev_end..section.range.start)
        # Don't push Tags to body
        # Advance past Tags
        prev_end = section.range.end
        after_selected_tags = true
    elif after_selected_tags:
        # First non-Tags after Tags: emit post-Tags separator first
        if prev_end < section.range.start:
            separator_ranges.push(prev_end..section.range.start)
        body_ranges.push(section.range.clone())
        prev_end = section.range.end
        after_selected_tags = false
    else:
        # Non-Tags before Tags (or no Tags in file)
        if prev_end < section.range.start:
            body_ranges.push(prev_end..section.range.start)
        body_ranges.push(section.range.clone())
        prev_end = section.range.end
# After loop: post-Tags content (if Tags was not the last H2) is body
if prev_end < source.len():
    body_ranges.push(prev_end..source.len())
```

#### Scenario: Bookmark cursor visits every byte exactly once

GIVEN a Bookmark's source bytes S

WHEN `parse(S, FromPath("x.bookmark.md"))` is called

THEN the Bookmark cursor visits each byte exactly once.

### Requirement: NoteDocument SHALL have semantic spans

The `NoteDocument` type SHALL expose `tag_token_spans:
Vec<Range<usize>>` as a derived view. For Notes, tag tokens
live in `tags_prefix_range`; for Todos/Bookmarks, they live in
`tag_section_range`. `tag_token_spans` SHALL reference bytes
owned by the respective range and SHALL NOT themselves own
bytes.

#### Scenario: tag_token_spans reference tag ranges

GIVEN any token span in `tag_token_spans`

THEN the span SHALL be contained within either
`tags_prefix_range` (for Note) or `tag_section_range` (for
Todo/Bookmark).

### Requirement: Parse is permissive for frozen cases; refuses only on no-nonblank-line

The `parse` function SHALL accept (return `Ok`) for any input
that can be losslessly parsed into a `NoteDocument` with a
valid ownership partition, including the **frozen W1 and
E1-E11 noncanonical forms**. `parse` SHALL refuse (return
`Err(NbError::ParseError)`) only on the explicitly scoped
no-nonblank-line case for Todo and Bookmark.

Refusal cases:

- Todo with no non-blank line at all: `MissingTitle`.
- Bookmark with no non-blank line at all: `MissingTitle`.
- Note: no refusal on empty source (permissive).

Mandatory-title/URL/state enforcement belongs to a separate
canonical validator (P5+), not to `parse`. Operation boundaries
(`do`/`undo` for Todo) enforce checkbox state at the operation
level, not the parse level.

#### Scenario: parse accepts Note with first-line Markdown body

GIVEN bytes `b"Just content.\n"` (no title)

WHEN `parse(bytes, FromPath("note.md"))` is called

THEN the result SHALL be `Ok(NoteDocument)` with
`title_range = None` and `body_ranges = [0..14]`.

#### Scenario: parse accepts Note with non-H1 first line

GIVEN bytes `b"#Title\n\nBody\n"` (no delimiter, not a valid H1)

WHEN `parse(bytes, FromPath("note.md"))` is called

THEN the result SHALL be `Ok(NoteDocument)` with
`title_range = None` and `body_ranges = [0..13]`.

#### Scenario: parse accepts Note with Setext-style title

GIVEN bytes `b"Title\n=====\n\nBody\n"` (Setext, not ATX H1)

WHEN `parse(bytes, FromPath("note.md"))` is called

THEN the result SHALL be `Ok(NoteDocument)` with
`title_range = None` and `body_ranges = [0..18]`.

#### Scenario: parse accepts Note with 4-space indent first line

GIVEN bytes `b"    # Title\n\nBody\n"` (indented code, not H1)

WHEN `parse(bytes, FromPath("note.md"))` is called

THEN the result SHALL be `Ok(NoteDocument)` with
`title_range = None` and `body_ranges = [0..18]`.

#### Scenario: parse accepts titleless Bookmark

GIVEN bytes `b"<https://example.com>\n"`

WHEN `parse(bytes, FromPath("x.bookmark.md"))` is called

THEN the result SHALL be `Ok(NoteDocument)` with
`title_range = None` and `url_range = Some(0..22)`.

#### Scenario: parse accepts Bookmark with missing URL

GIVEN bytes `b"# Bookmark\n\nBody\n"`

WHEN `parse(bytes, FromPath("x.bookmark.md"))` is called

THEN the result SHALL be `Ok(NoteDocument)` with
`title_range = Some(0..11)`, `url_range = None`, and
`body_ranges = [12..17]`.

#### Scenario: parse accepts checkbox-less Todo

GIVEN bytes `b"# Task\n\nBody\n"` (no `[ ]` or `[x]`)

WHEN `parse(bytes, FromPath("x.todo.md"))` is called

THEN the result SHALL be `Ok(NoteDocument)` with
`title_range = Some(0..7)`, `todo_state = None`, and
`body_ranges = [8..13]`.

### Requirement: Parse shall fail only on the scoped no-nonblank-line refusal

The `parse` function SHALL return `Err(NbError::ParseError { kind,
location })` for the no-nonblank-line case for Todo and Bookmark
(where there is no structural anchor to recognize the kind).
No other input is rejected at the parse level.

#### Scenario: parse fails for empty Todo

GIVEN bytes `b""`

WHEN `parse(b"", FromPath("x.todo.md"))` is called

THEN the result SHALL be
`Err(NbError::ParseError { kind: MissingTitle, location: 0..0 })`.

#### Scenario: parse fails for empty Bookmark

GIVEN bytes `b""`

WHEN `parse(b"", FromPath("x.bookmark.md"))` is called

THEN the result SHALL be
`Err(NbError::ParseError { kind: MissingTitle, location: 0..0 })`.

#### Scenario: parse accepts empty Note

GIVEN bytes `b""`

WHEN `parse(b"", FromPath("x.md"))` is called

THEN the result SHALL be `Ok(NoteDocument)` with all ranges
empty.

### Requirement: NoteDocument SHALL be constructed via parse

```rust
pub fn parse(bytes: &[u8], context: ParseContext)
    -> Result<Self, NbError>;
```

#### Scenario: Parse succeeds for valid input

GIVEN valid bytes B for any kind

WHEN `parse(B, ParseContext::FromPath(path))` is called

THEN the result SHALL be `Ok(NoteDocument)` with the partition
covering B.

### Requirement: DocumentKind SHALL distinguish Note, Todo, Bookmark

```rust
pub enum DocumentKind { Note, Todo, Bookmark }
```

For Todo notes, the `TodoState` SHALL be derived from the title
line and exposed via `doc.todo_state() -> Option<TodoState>`.
State is NOT carried on the `DocumentKind` variant.

```rust
pub enum TodoState { Open, Done }
```

#### Scenario: Todo state derived from title

GIVEN bytes with `# [ ] Buy milk`

THEN `kind == DocumentKind::Todo` and `todo_state() ==
Some(TodoState::Open)`.

GIVEN bytes with `# [x] Done task`

THEN `kind == DocumentKind::Todo` and `todo_state() ==
Some(TodoState::Done)`.

#### Scenario: Todo state None for checkbox-less title

GIVEN bytes with `# Task` (no `[ ]`/`[x]`)

THEN `kind == DocumentKind::Todo` and `todo_state() == None`.

### Requirement: ParseContext SHALL determine DocumentKind by file extension

```rust
pub enum ParseContext {
    FromPath(PathBuf),
    Explicit(DocumentKind),
}
```

- `FromPath(p)`: kind inferred from file extension:
  - `.todo.md` → `DocumentKind::Todo`
  - `.bookmark.md` → `DocumentKind::Bookmark`
  - `.md` (or other) → `DocumentKind::Note`
  - `.todo` (without `.md`) → `DocumentKind::Note`
    deterministically; the `.todo` extension is `show --type`
    classification only, not mutation-authoritative.

- `Explicit(k)`: kind set explicitly by caller.
- `FromBytes` inference is NOT supported.

#### Scenario: FromPath infers Todo from .todo.md

GIVEN a path ending in `.todo.md`

THEN `ParseContext::FromPath(p)` SHALL yield kind `Todo`.

#### Scenario: FromPath infers Note from .todo (no .md)

GIVEN a path ending in `.todo`

THEN `ParseContext::FromPath(p)` SHALL yield kind `Note`
(`.todo` is not mutation-authoritative Todo).

#### Scenario: FromPath infers Bookmark

GIVEN a path ending in `.bookmark.md`

THEN `ParseContext::FromPath(p)` SHALL yield kind `Bookmark`.

#### Scenario: FromPath infers Note

GIVEN a path ending in `.md`

THEN `ParseContext::FromPath(p)` SHALL yield kind `Note`.

#### Scenario: Explicit overrides inference

GIVEN `ParseContext::Explicit(DocumentKind::Bookmark)`
regardless of content

THEN kind SHALL be `Bookmark`.

### Requirement: Note tags are a prefix line

For `kind = Note`, tags SHALL be recognized as a **prefix
line** matching the pattern `#tag1 #tag2` (whitespace-separated
`#`-prefixed tokens, including trailing newline). The tags
prefix line SHALL appear:

- After the title (if any) and its trailing separator, OR
- At the start of a titleless Note (byte 0), if the first
  non-blank line is a tags-only line.

A single `#Tag` (no space) is NOT a tags prefix line; it is
body content (matches titleless body or non-H1 first line).

#### Scenario: Note with title and tags

GIVEN bytes `b"# Writer Note\n\n#alpha #beta\n\nWriter body\nsecond line\n\n"` (writer-canonical form)

THEN `title_range = Some(0..14)`, `tags_prefix_range =
Some(15..28)`, and `body_ranges = [29..54]`.

#### Scenario: Titleless tagged Note

GIVEN bytes `b"#alpha #beta\n\nWriter titleless body\n\n"`

THEN `title_range = None` and `tags_prefix_range = Some(0..13)`
and `body_ranges = [14..37]`.

#### Scenario: Note with title only

GIVEN bytes `b"# Title\n\nbody\n"`

THEN `title_range = Some(0..8)` and `tags_prefix_range = None`
and `body_ranges = [9..14]`.

#### Scenario: Note without title and without tags

GIVEN bytes `b"body\n"`

THEN `title_range = None` and `tags_prefix_range = None` and
`body_ranges = [0..5]`.

### Requirement: Todo title grammar

For `kind = Todo`, the title SHALL be the first non-blank line.
The title line MAY begin with:

- `# [ ] <title>` → state = Open
- `# [x] <title>` → state = Done
- `# <title>` (no checkbox) → state = None

#### Scenario: Open todo with checkbox

GIVEN bytes `b"# [ ] Task\n\nbody\n"`

THEN `title_range = Some(0..11)` and `todo_state = Some(Open)`.

#### Scenario: Done todo with checkbox

GIVEN bytes `b"# [x] Task\n\nbody\n"`

THEN `title_range = Some(0..11)` and `todo_state = Some(Done)`.

#### Scenario: Todo without checkbox

GIVEN bytes `b"# Task\n\nbody\n"`

THEN `title_range = Some(0..7)` and `todo_state = None`.

### Requirement: Todo terminal Tags requires the final H2 to be Tags

For `kind = Todo`, the `tag_section_range` SHALL be the **last**
H2 section in the file whose heading text is "Tags". The
requirement is that the **final H2 itself is Tags** — if the
final H2 has a different heading (e.g., Description, Source,
Content), the parser does NOT treat any other H2 Tags as
metadata; earlier H2 Tags sections are body content.

#### Scenario: Todo with terminal Tags (33-byte local-prose example)

GIVEN bytes `b"# [ ] Task\n\nbody\n\n## Tags\n\n#a #b\n"`

THEN `tag_section_range = Some(18..33)` (the `## Tags\n\n#a #b\n`
section).

#### Scenario: Nonterminal Tags Todo is body (frozen E7.1)

GIVEN bytes `b"# [ ] Task\n\n## Tags\n\n#alpha\n\n## Description\n\nBody\n"`

THEN `tag_section_range = None` (the final H2 is Description,
not Tags, so earlier Tags is body) and the H2 Tags section
is part of `body_ranges = [12..50]`.

#### Scenario: Todo with duplicate Tags: last is canonical (frozen E7.2)

GIVEN bytes `b"# [ ] Task\n\n## Tags\n\n#first\n\n## Description\n\nBody\n\n## Tags\n\n#last\n"`

THEN `tag_section_range = Some(51..66)` (the LAST `## Tags\n\n#last\n`)
and earlier H2 Tags section is part of `body_ranges = [12..50]`.

### Requirement: Bookmark title is optional valid ATX H1

For `kind = Bookmark`, the title is an optional CommonMark ATX
H1 line. If the first non-blank line is not a valid ATX H1
(e.g., it is a URL line), `title_range = None`. The title rule
does NOT say "title is the first non-blank line regardless of
H1 validity" — for Bookmark, title is specifically the ATX H1
or absent.

#### Scenario: Bookmark with title and URL

GIVEN bytes `b"# Bookmark\n\n<https://example.com>\n"`

THEN `title_range = Some(0..11)`, `sep 11..12`,
`url_range = Some(12..34)`, and `body empty`.

#### Scenario: Bookmark with title only

GIVEN bytes `b"# Bookmark\n\nbody\n"`

THEN `title_range = Some(0..11)`, `url_range = None`, and
`body_ranges = [12..17]`.

#### Scenario: Titleless Bookmark with URL

GIVEN bytes `b"<https://example.com>\n"`

THEN `title_range = None` and `url_range = Some(0..22)`.

### Requirement: Bookmark Tags canonical position is before Content/Source

For `kind = Bookmark`, the `tag_section_range` SHALL be the
H2 Tags section that appears **before** the first Content or
Source section (if either is present). If no Content or Source
section is present, `tag_section_range` SHALL be the LAST H2
Tags section in the file. If no H2 Tags section is present,
`tag_section_range = None`.

#### Scenario: Bookmark with Tags before Content (frozen E9)

GIVEN bytes `b"# Bookmark\n\n<URL>\n\n## Description\n\nDesc\n\n## Tags\n\n#alpha\n\n## Content\n\nContent body\n"`

THEN `title_range = Some(0..11)`, `sep 11..12`,
`url_range = Some(12..18)`, `sep 18..19`,
`body_ranges = [19..40]` (Description+Desc), `sep 40..41`,
`tag_section_range = Some(41..57)` (Tags), `sep 57..58`,
`body_ranges` includes `[58..83]` (Content+Content body).

#### Scenario: Bookmark with Tags terminal (no Content/Source)

GIVEN bytes `b"# Bookmark\n\n<URL>\n\n## Tags\n\n#a\n"`

THEN `title_range = Some(0..11)`, `sep 11..12`,
`url_range = Some(12..18)`, `sep 18..19`,
`tag_section_range = Some(19..30)`, and `body empty`.

#### Scenario: Bookmark without Tags

GIVEN bytes `b"# Bookmark\n\n<URL>\n\nbody\n"`

THEN `title_range = Some(0..11)`, `sep 11..12`,
`url_range = Some(12..18)`, `sep 18..19`,
`tag_section_range = None`, and `body_ranges = [19..24]`.

### Requirement: H2-looking bytes inside Bookmark Source/Content fence are body

For `kind = Bookmark`, H2-looking bytes (lines matching
`^## <text>$`) that appear inside the Content section or
inside a fenced Source payload SHALL be treated as body content,
NOT as metadata. The scanner SHALL respect fenced-code-block
boundaries (``` ``` ``` ```) when scanning Source's payload.

#### Scenario: H2 in Content is body (frozen E10.1)

GIVEN bytes `b"# Bookmark\n\n<URL>\n\n## Tags\n\n#official\n\n## Content\n\n## Tags in body\n"`

THEN `tag_section_range = Some(19..38)` (official Tags) and
`body_ranges` includes `[39..67]` (Content with inner H2 "## Tags in body").

#### Scenario: H2 in Source fence is body (frozen E10.2)

GIVEN bytes `b"# Bookmark\n\n<URL>\n\n## Tags\n\n#official\n\n## Source\n\n```html\n## Tags\n<p>raw</p>\n```\n"`

THEN `tag_section_range = Some(19..38)` (official Tags) and
`body_ranges` includes `[39..81]` (Source with fenced
payload including inner H2 "## Tags" and HTML).

### Requirement: Title detection SHALL align with CommonMark

When a title is present, the first non-blank line SHALL be
detected as a CommonMark ATX H1 per `src/lib.rs:1083-1200`:

- 0 to 3 leading spaces (4 or more is an indented code block).
- Exactly one opening `#`.
- Required space/tab/EOL after `#`.
- Optional closing-hash sequence (preceded by space/tab,
  followed by spaces/tabs to EOL).

For Note kind, an invalid H1 first line is treated as body
(no title). For Todo/Bookmark, the title is the first
non-blank line (if it is a valid ATX H1; else `title_range =
None` for Bookmark permissive, or ParseError for Todo if
there is no non-blank line at all).

#### Scenario: ATX H1 with closing hashes

GIVEN line `# Title #`

THEN heading text SHALL be `Title`.

#### Scenario: `#Title` not an H1

GIVEN line `#Title`

THEN this SHALL NOT be detected as an ATX H1.

#### Scenario: 4-space indent is not an H1

GIVEN line `    # Title` (4 leading spaces)

THEN this SHALL NOT be detected as an ATX H1 (indented code
block per CommonMark).

### Requirement: NoteDocument SHALL emit lossless bytes

```rust
pub fn emit(&self) -> &[u8];
```

`emit` SHALL return a slice into the retained `source` bytes,
byte-identical to the original input.

**Non-canonicalization:** `parse` does NOT select among
alternative valid forms for the same input. When the input
contains a structural marker that has multiple valid
classifications (for example, a `## Tags` section appearing
mid-document versus terminal), `parse` records the partition
that the bytes actually exhibit and `emit` returns the
original bytes verbatim. There is no canonical-form rewriting
and no body validation against an "illegal section" whitelist.
Permissive acceptance per Postel's Law (per the `nb
consistency shall be scoped to three evidence levels`
requirement) means that a body edit that adds a `## Tags`
heading is accepted on the next parse; reclassification of
that section between body and metadata is silent and does NOT
change the body fingerprint unless the body bytes themselves
change. Structural-validity enforcement belongs to a separate
canonical validator (P5+), not to `parse`.

#### Scenario: Round-trip identity for successful parses

GIVEN bytes B that `parse(B, ctx)` accepts (returns Ok)

THEN `parse(B, ctx).unwrap().emit() == B`.

#### Scenario: Round-trip identity over corpus

GIVEN `include_bytes!` fixtures from pinned `nb 7.24.0`

THEN for each fixture F, `parse(F, FromPath(...)).emit() == F`.

### Requirement: Fingerprint SHALL be a versioned public token

```rust
pub struct Fingerprint(String);
```

Canonical form: `b3:<64 lowercase hex>` (BLAKE3-256).

```rust
pub fn fingerprint(doc: &NoteDocument) -> Fingerprint;
```

The Fingerprint SHALL be computed by hashing body_ranges bytes
in source order.

#### Scenario: Fingerprint format

GIVEN body bytes `"line1\nline2\n"`

THEN the fingerprint SHALL be `b3:` followed by the BLAKE3-256
hex digest of those bytes.

#### Scenario: BLAKE3 of empty body

GIVEN empty body bytes

THEN the fingerprint SHALL equal
`b3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262`.

#### Scenario: Fragmented body hashes in source order

GIVEN a NoteDocument with body_ranges [bytes A, bytes B]

THEN `fingerprint(doc)` SHALL equal `BLAKE3(A || B)`.

### Requirement: Fingerprint SHALL have full FromStr rejection cases

`Fingerprint::from_str` SHALL reject empty string, leading or
trailing whitespace, wrong prefix, wrong total length, non-hex
characters, and mixed-case or uppercase hex. Each rejection
SHALL return `NbError::InvalidFingerprint { reason: String }`.

#### Scenario: Empty string rejected

GIVEN input `""`

THEN `Fingerprint::from_str(s)` SHALL fail with reason
`"empty"`.

#### Scenario: Wrong prefix rejected

GIVEN input `"sha3:af1349b9..."`

THEN `Fingerprint::from_str(s)` SHALL fail with reason
`"unknown_algorithm_prefix"`.

#### Scenario: Uppercase hex rejected

GIVEN input `"b3:AF1349B9..."`

THEN `Fingerprint::from_str(s)` SHALL fail with reason
`"uppercase_hex"`.

### Requirement: Fingerprint shall have from_json associated method returning NbError

`Fingerprint::from_json(s: &str) -> Result<Fingerprint, NbError>`
(associated method) SHALL provide typed deserialization from a
JSON string. JSON parse errors SHALL map to
`NbError::JsonParseError`; format errors SHALL map to
`NbError::InvalidFingerprint`. The standard `Deserialize` impl
rejects via `D::Error`; only `from_json` returns `NbError` typed.

#### Scenario: from_json returns NbError on malformed input

GIVEN input `"b3:afg349b9..."`

WHEN `Fingerprint::from_json(s)` is called

THEN the result SHALL be
`Err(NbError::InvalidFingerprint { reason: "invalid_hex" })`.

### Requirement: Body fingerprint authenticates body_ranges bytes

A body fingerprint authenticates exactly the concatenated bytes
in `body_ranges` (in source order) and establishes nothing
about excluded partition ranges (prefix, title, tags_prefix,
url, tag_section, separators). The implementation of body-byte
replacement (e.g., `replace_note_body`) belongs to P5; P1 only
specifies the invariant. The invariant holds for canonical
`nb 7.24.0` output; for non-canonical input, consumers verify
before mutating.

#### Scenario: Identical body domains produce equal fingerprints

GIVEN two `NoteDocument` values with identical body_ranges
bytes (in the same order)

WHEN `fingerprint(doc1)` and `fingerprint(doc2)` are computed

THEN the results SHALL be equal.

#### Scenario: Different metadata produces equal fingerprints when body is same

GIVEN two `NoteDocument` values with different title or
tags_prefix ranges but identical body_ranges bytes

WHEN `fingerprint(doc1)` and `fingerprint(doc2)` are computed

THEN the results SHALL be equal (the fingerprint depends only
on body_ranges bytes).

### Requirement: Parser shall operate on direct &[u8] input

The `parse` function SHALL accept `&[u8]` directly.

#### Scenario: Parse preserves invalid UTF-8

GIVEN bytes containing invalid UTF-8 sequences

THEN `parse` SHALL succeed (permissive acceptance)

AND `source()` SHALL return the exact bytes.

#### Scenario: Parse preserves CR-only terminators

GIVEN bytes with `\r` line terminators

THEN `parse` SHALL succeed

AND `body_ranges` SHALL span the full body including `\r`
terminators.

**`nb 7.24.0` honors CR-only terminators verbatim.** Empirical
evidence: `nb show <sel>` on a CR-only-terminated file
returns the exact input bytes (round-trip verified by the
`note-cr-only` probe in `tests/integration/probe_empirical.rs`
at `tests/integration/probe_empirical.rs:note-cr-only`). The
CR-only case is therefore a writer-canonical form for `nb
7.24.0`, not just an exotic edge case.

#### Scenario: Parse preserves BOM

GIVEN file bytes starting with UTF-8 BOM (`\xEF\xBB\xBF`)

THEN `parse` SHALL succeed

AND `source()` SHALL return the exact bytes including the BOM.

### Requirement: tags() shall return an iterator

`NoteDocument::tags()` SHALL return a `TagsIter<'_>` iterator
that yields `&[u8]` per tag token in document order. For
Note, tokens come from `tags_prefix_range`; for Todo/Bookmark,
from `tag_section_range`.

#### Scenario: tags() yields token bytes

GIVEN a NoteDocument with tag tokens `"alpha"`, `"beta"`

THEN the `tags()` iterator SHALL yield `b"alpha"`, `b"beta"` in
that order.

### Requirement: title() shall return raw bytes

`NoteDocument::title()` SHALL return `Option<&[u8]>` of the raw
title line bytes (including the trailing newline, excluding any
absorbed separator); `None` if no title.

#### Scenario: title returns raw bytes for Todo

GIVEN a Todo with title `# [ ] Buy milk\n`

THEN `title()` SHALL return `Some(b"# [ ] Buy milk\n")` (raw
bytes including the `# [ ]` prefix and trailing newline).

#### Scenario: title returns raw bytes for Bookmark

GIVEN a Bookmark with title `# My Bookmark\n`

THEN `title()` SHALL return `Some(b"# My Bookmark\n")`.

#### Scenario: title returns None for untitled Note

GIVEN a Note with no title

THEN `title()` SHALL return `None`.

### Requirement: body() shall be public

`NoteDocument::body()` SHALL be a public method returning a
`BodyFragments<'_>` iterator over body byte ranges.

#### Scenario: body() yields body fragments

GIVEN a parsed `NoteDocument` with body_ranges `[10..15, 20..25]`

WHEN `body()` is called

THEN it SHALL yield `&source[10..15]`, `&source[20..25]` in
order.

### Requirement: url() shall be a public raw/fallible accessor for Bookmark

`NoteDocument::url()` SHALL return `Option<&[u8]>` of the raw
URL line bytes (including the trailing newline); `None` if
no URL.

`NoteDocument::url_str()` SHALL return
`Option<Result<&str, std::str::Utf8Error>>`.

#### Scenario: url returns raw bytes for Bookmark

GIVEN a Bookmark with URL `<https://example.com>\n`

THEN `url()` SHALL return `Some(b"<https://example.com>\n")`.

#### Scenario: url returns None for Bookmark without URL

GIVEN a Bookmark without URL

THEN `url()` SHALL return `None`.

### Requirement: title_str() shall signal UTF-8 errors

`NoteDocument::title_str()` SHALL return
`Option<Result<&str, std::str::Utf8Error>>`. `None` for no
title; `Some(Ok(_))` for valid UTF-8; `Some(Err(_))` for
invalid UTF-8.

#### Scenario: title_str signals invalid UTF-8

GIVEN title bytes with invalid UTF-8

THEN `title_str()` SHALL return `Some(Err(Utf8Error))`.

### Requirement: tags_str() shall signal UTF-8 errors per item

`NoteDocument::tags_str()` SHALL return an iterator that yields
`Result<&str, std::str::Utf8Error>` per token.

#### Scenario: tags_str signals invalid UTF-8

GIVEN a NoteDocument with one valid token and one invalid UTF-8
token

THEN the `tags_str()` iterator SHALL yield
`Ok("valid")`, `Err(Utf8Error)`.

### Requirement: NbError shall have full structured variants with serde

`NbError` SHALL be redesigned to support structured variants
with serde across the board. All variants derive
`Serialize, Deserialize`. `NbError` also derives `Display` and
implements `std::error::Error`.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
pub enum NbError {
    #[error("command {command:?} failed (exit {exit_code:?}): {stderr}")]
    CommandFailed {
        command: String,
        stderr: String,
        exit_code: Option<i32>,
    },
    #[error("`nb` executable not found at path {path:?}")]
    ExecutableNotFound {
        path: String,
    },
    #[error("not found: {selector:?}")]
    NotFound { selector: String },
    #[error("I/O error at {path:?}: {source}")]
    Io { path: PathBuf, source: IoError },
    #[error("unsupported show target: {selector:?} (actual type: {actual_type:?})")]
    UnsupportedShowTarget {
        selector: String,
        actual_type: String,
    },
    #[error("duplicate title H1: title={title:?} heading={heading:?}")]
    DuplicateTitleHeading {
        title: String,
        heading: String,
    },
    #[error("parse error: {kind:?} at {location:?}")]
    ParseError {
        kind: ParseErrorKind,
        location: Range<usize>,
    },
    #[error("invalid fingerprint: {reason:?}")]
    InvalidFingerprint { reason: String },
    #[error("JSON parse error: {source:?}")]
    JsonParseError { source: String },
    #[error("validation error: {reason:?}")]
    ValidationError {
        reason: String,
        location: Option<Range<usize>>,
    },
}
```

`IoError` is a **serializable snapshot** that captures the
`std::io::Error` chain STRUCTURE (as a tree of snapshots) but
does NOT preserve original source identity/semantics.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct IoError {
    pub kind: IoErrorKind,
    pub message: String,
    pub os_error: Option<i32>,
    pub source: Option<Box<IoError>>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IoErrorKind {
    NotFound, PermissionDenied, ConnectionRefused,
    ConnectionReset, ConnectionAborted, NotConnected,
    AddrInUse, AddrNotAvailable, BrokenPipe, AlreadyExists,
    WouldBlock, InvalidInput, InvalidData, TimedOut,
    WriteZero, Interrupted, UnexpectedEof, OutOfMemory,
    Other,
}
```

`ParseErrorKind` has the narrow refusal contract:
`MissingTitle` (only for the scoped no-nonblank-line refusal).
Title-shape errors belong to a later strict validator (P5+).

#### Scenario: NbError serde round-trip preserves all variants

GIVEN `err` of any `NbError` variant

WHEN serialized to JSON and deserialized back

THEN the result SHALL equal `err` (modulo the lossy `IoError`
reverse conversion).

### Requirement: IoError reverse conversion is lossy

The `From<IoError> for std::io::Error` conversion is **explicitly
lossy**. It does NOT preserve the original `std::io::Error`
chain.

The conversion prefers `from_raw_os_error(code)` when a
`raw_os_error()` exists on the snapshot, which LOSES the
snapshot message and source chain. Otherwise, the conversion
constructs a new `std::io::Error::new(kind, message)`, which
loses `raw_os_error()`.

The forward conversion `From<std::io::Error> for IoError`
captures the chain as a tree of serializable snapshots
(preserves the chain STRUCTURE, not original source
identity/semantics). It walks `Error::source()` and captures
the chain as `Box<IoError>` snapshots. Nested non-`io::Error`
sources are stringified with `kind = Other`.

#### Scenario: IoError with raw_os_error converts via from_raw_os_error

GIVEN `IoError { kind: NotFound, message: "file not found",
os_error: Some(2), source: None }`

WHEN converted to `std::io::Error` via `From`

THEN the result SHALL be constructed via
`std::io::Error::from_raw_os_error(2)`, losing the snapshot
message and any source chain.

#### Scenario: IoError without raw_os_error converts via from_raw_os_error fallback

GIVEN `IoError { kind: NotFound, message: "file not found",
os_error: None, source: None }`

WHEN converted to `std::io::Error` via `From`

THEN the result SHALL be constructed via
`std::io::Error::new(NotFound, "file not found")`, losing any
`raw_os_error()` value.

### Requirement: nb consistency shall be scoped to three evidence levels

The proposal SHALL NOT claim that "inputs `nb` accepts have
the same structure" or "inputs `nb` rejects are also rejected."
`nb 7.24.0` does not expose a structural parser contract.

Instead, three evidence levels SHALL be documented:

1. **Writer-produced canonical corpus**: P1 is precise for
   `nb 7.24.0` writer output.
2. **Permissive `show`/`type` behavior**: P1 is permissive
   for non-canonical input (Postel's Law).
3. **Operation-specific validators**: Todo `do`/`undo` apply
   a narrower filename/content check.

#### Scenario: Writer-canonical output parses precisely

GIVEN bytes produced by `nb 7.24.0` writer

WHEN `parse(bytes, FromPath(path))` is called

THEN the partition SHALL match the writer-canonical structure.

### Requirement: Public API shall expose complete types

The public NbApi surface SHALL expose:

- `pub struct NoteDocument` with accessor methods
  `source`, `title`, `title_str`, `tags`, `tags_str`, `body`,
  `kind`, `todo_state`, `url`, `url_str`.
- `pub enum DocumentKind { Note, Todo, Bookmark }`.
- `pub enum TodoState { Open, Done }`.
- `pub enum ParseContext { FromPath(PathBuf),
  Explicit(DocumentKind) }`.
- `pub struct TagsIter<'a>`, `pub struct TagsStrIter<'a>`,
  `pub struct BodyFragments<'a>`.
- `pub struct Fingerprint(String)` — with Display, FromStr, Eq,
  Hash, Serialize, Deserialize (custom), optional JsonSchema,
  and `from_json` associated method returning `NbError`.
- `pub enum NbError` (structured variants with Display and
  `std::error::Error`).
- `pub enum ParseErrorKind { MissingTitle }`.
- `pub struct IoError`, `pub enum IoErrorKind`.
- `pub mod parser` with `pub fn parse(&[u8], ParseContext) ->
  Result<NoteDocument, NbError>`.
- `pub fn NoteDocument::emit(&self) -> &[u8]`.
- `pub mod fingerprint` with `pub fn fingerprint(&NoteDocument)
  -> Fingerprint`.

#### Scenario: Public types accessible

WHEN a downstream consumer imports the public types

THEN the types SHALL be in scope and constructible per their
public API.

### Requirement: Test harness shall be wired in Cargo.toml (runs by default)

```toml
[[test]]
name = "parser"
path = "tests/unit/parser.rs"

[[test]]
name = "emitter"
path = "tests/unit/emitter.rs"

[[test]]
name = "fingerprint"
path = "tests/unit/fingerprint.rs"

[[test]]
name = "note_document_model"
path = "tests/integration/note_document_model.rs"
```

The `testing` feature enables `NBTestEnv` for downstream
consumers, not these test binaries.

#### Scenario: Test entries present in Cargo.toml

WHEN `cargo test` runs (no feature flag)

THEN the four test binaries SHALL be built and executed.

### Requirement: Resolution table for W1 and E1-E11

The proposal SHALL include a resolution table covering W1
(canonical writer baselines) and E1-E11 (frozen bounded
empirical enumeration) with exact byte offsets.

### Resolution Table

| Case | Length | Partition |
|------|--------|-----------|
| W1.1: titled tagged Note `b"# Writer Note\n\n#alpha #beta\n\nWriter body\nsecond line\n\n"` | 54 | `title 0..14; sep 14..15; tags 15..28; sep 28..29; body 29..54` |
| W1.2: titleless tagged Note `b"#alpha #beta\n\nWriter titleless body\n\n"` | 37 | `tags 0..13; sep 13..14; body 14..37` |
| W1.3: tagged Todo `b"# [ ] Writer Todo\n\n## Description\n\nWriter description\n\n## Tags\n\n#alpha #beta\n\n"` | 78 | `title 0..18; sep 18..19; body 19..54; sep 54..55; tags 55..78` |
| W1.4: tagged offline Bookmark `b"# Writer Bookmark (example.com)\n\n<https://example.com>\n\n## Tags\n\n#beta\n"` | 71 | `title 0..32; sep 32..33; URL 33..55; sep 55..56; tags 56..71; body empty` |
| W1.5: offline Bookmark without title `b"# (example.org)\n\n<https://example.org/no-title>\n"` | 48 | `title 0..16; sep 16..17; URL 17..48; body empty` |
| E1.1: `# Title\n\nBody\n` | 14 | `title 0..8; sep 8..9; body 9..14` |
| E1.2: `Just content.\n` | 14 | `body 0..14` |
| E1.3: `#Title\n\nBody\n` | 13 | `body 0..13; no separator ownership` |
| E1.4: `    # Title\n\nBody\n` | 18 | `body 0..18; no separator ownership` |
| E1.5: `Title\n=====\n\nBody\n` | 18 | `body 0..18; no separator ownership` |
| E2: `\n\n# Title\n\nBody\n` | 16 | `sep 0..2; title 2..10; sep 10..11; body 11..16` |
| E3.1: titled tagged Note | 54 | (same as W1.1) |
| E3.2: titleless tagged Note | 37 | (same as W1.2) |
| E3.3: hand-written Note with `## Tags` in body `b"Text\n\n## Tags\n\n#alpha #beta\n"` | 28 | `body 0..28` |
| E4.1: BOM `b"\xef\xbb\xbf# Title\n\nBody\n"` | 17 | `prefix 0..3; title 3..11; sep 11..12; body 12..17` |
| E4.2: CR-only `b"# Title\r\rBody\rSecond\r"` | 21 | `title 0..8; sep 8..9; body 9..21` |
| E4.3: invalid UTF-8 body `b"# Title\n\nBody \xff\xfe\n"` | 17 | `title 0..8; sep 8..9; body 9..17` |
| E5.1: `.todo` `b"# [ ] Task\n\n## Description\n\nBody\n\n## Tags\n\n#alpha #beta\n"` | 56 | `title 0..11; sep 11..12; body 12..56` |
| E5.2: `.todo.md` | 78 | (same as W1.3) |
| E6: checkbox-less Todo `b"# Task\n\nBody\n"` | 13 | `title 0..7; sep 7..8; body 8..13` |
| E7.1: nonterminal Tags Todo `b"# [ ] Task\n\n## Tags\n\n#alpha\n\n## Description\n\nBody\n"` | 50 | `title 0..11; sep 11..12; body 12..50; no tag metadata` |
| E7.2: duplicate Tags Todo `b"# [ ] Task\n\n## Tags\n\n#first\n\n## Description\n\nBody\n\n## Tags\n\n#last\n"` | 66 | `title 0..11; sep 11..12; body 12..50; sep 50..51; tags 51..66` |
| E8.1: minimal Bookmark `b"# Bookmark\n\n<https://example.com>\n"` | 34 | `title 0..11; sep 11..12; URL 12..34; body empty` |
| E8.2: titleless Bookmark `b"<https://example.com>\n"` | 22 | `URL 0..22; body empty` |
| E8.3: missing URL Bookmark `b"# Bookmark\n\nBody\n"` | 17 | `title 0..11; sep 11..12; body 12..17` |
| E9: nonterminal Tags Bookmark `b"# Bookmark\n\n<URL>\n\n## Description\n\nDesc\n\n## Tags\n\n#alpha\n\n## Content\n\nContent body\n"` | 83 | `title 0..11; sep 11..12; URL 12..18; sep 18..19; body 19..40; sep 40..41; tags 41..57; sep 57..58; body 58..83` |
| E10.1: H2 in Content `b"# Bookmark\n\n<URL>\n\n## Tags\n\n#official\n\n## Content\n\n## Tags in body\n"` | 67 | `title 0..11; sep 11..12; URL 12..18; sep 18..19; tags 19..38; sep 38..39; body 39..67` |
| E10.2: H2 in Source fence `b"# Bookmark\n\n<URL>\n\n## Tags\n\n#official\n\n## Source\n\n```html\n## Tags\n<p>raw</p>\n```\n"` | 81 | `title 0..11; sep 11..12; URL 12..18; sep 18..19; tags 19..38; sep 38..39; body 39..81` |
| E11: scope clarification (three evidence levels) | n/a | `n/a` (document three evidence levels: writer-produced canonical corpus + permissive `show`/`type` + operation-specific validators like Todo `do`/`undo`; no byte-level partition) |

#### Scenario: Resolution table covers W1 and E1-E11

WHEN the resolution table is reviewed

THEN every case (W1.1-W1.5, E1.1-E1.5, E2, E3.1-E3.3, E4.1-E4.3,
E5.1-E5.2, E6, E7.1-E7.2, E8.1-E8.3, E9, E10.1-E10.2, E11) SHALL
have a row in the table with exact byte offsets.

### Requirement: Lossless round-trip SHALL hold over include_bytes! fixtures

The integration test suite SHALL include `include_bytes!`
fixtures generated by pinned `nb 7.24.0`:

- `fixtures/note.md` — ordinary Note (e.g., titled tagged).
- `fixtures/todo.todo.md` — Todo (e.g., with checkbox + Tags).
- `fixtures/bookmark.bookmark.md` — Bookmark (e.g., with URL +
  Tags + Content).

For each fixture, `parse(fixture, FromPath(path)).emit() ==
fixture` SHALL hold.

#### Scenario: Round-trip for ordinary Note fixture

GIVEN `include_bytes!("fixtures/note.md")`

THEN `parse(bytes, FromPath("note.md")).emit() == bytes`.

#### Scenario: Round-trip for Todo fixture

GIVEN `include_bytes!("fixtures/todo.todo.md")`

THEN `parse(bytes, FromPath("todo.todo.md")).emit() == bytes`.

#### Scenario: Round-trip for Bookmark fixture

GIVEN `include_bytes!("fixtures/bookmark.bookmark.md")`

THEN `parse(bytes, FromPath("bookmark.bookmark.md")).emit() ==
bytes`.

### Requirement: Property-based round-trip tests SHALL pass over grammar shapes

The unit test suite SHALL include property-based round-trip
tests (proptest or quickcheck) over generated grammar shapes.

#### Scenario: Property-based round-trip over varied shapes

WHEN proptest runs the round-trip property

THEN all generated cases SHALL pass.

### Requirement: Property tests SHALL cover indexed range invariants

Property-based tests SHALL include an invariant checker that
verifies for every generated `NoteDocument`:

- The union of all ownership ranges equals `0..source.len()`.
- All ownership ranges are pairwise disjoint.
- Each range is half-open.
- The body fingerprint equals `BLAKE3(A || B || ...)` over
  body_ranges bytes in source order.

#### Scenario: Property tests verify range invariants

WHEN proptest runs the invariant checker

THEN all generated `NoteDocument` values SHALL satisfy the
range invariants

AND the fingerprint SHALL match the expected hash.

### Requirement: Migration table for NbError

| Old variant | New variant | Migration |
|---|---|---|
| `CommandFailed(String)` | `CommandFailed { command, stderr, exit_code }` | capture fields at call sites; map validation/policy failures to `ValidationError` |
| `NotFound` (unit) | `ExecutableNotFound { path }` | the unit means `nb` binary missing; track path |
| `Io(std::io::Error)` | `Io { path, source: IoError }` | track path in callers; convert via `From`; reverse is lossy |
| `UnsupportedShowTarget { selector, actual_type }` | Same | Unchanged |
| `DuplicateTitleHeading { title, heading }` | Same | Unchanged |

New variants: `ParseError`, `InvalidFingerprint`,
`JsonParseError`, `ValidationError`, `ExecutableNotFound`.

#### Scenario: Migration table documents old-to-new mapping

GIVEN a consumer in `nb-api 0.2.x` matching on
`NbError::CommandFailed(s)`

WHEN the consumer upgrades to `nb-api 0.3.0`

THEN the consumer SHALL update the match arm to
`NbError::CommandFailed { command, stderr, exit_code }`.

#### Scenario: Migration table covers all old variants

WHEN the migration table is reviewed

THEN every old `nb-api 0.2.x` `NbError` variant SHALL have a
corresponding new entry.



## ADDED Requirements (R3 revision)

This section adds the format-dispatch refusal contract to the
P1 specification. The revision is triggered by the R3 format-
dispatch review (`nb-api:reviews/1`, "R3 format-dispatch
specification re-review" section, 2026-07-30) and the operator
direction recorded in that review to reject recognized but
currently unsupported `nb` formats explicitly rather than
silently parsing them as Markdown Notes.

**Supersession.** The R3 requirements supersede the named
earlier normative clauses that they contradict:

- The earlier "Parse shall fail only on the scoped no-nonblank-
  line refusal" requirement (which excludes format-dispatch
  refusal) is superseded by the R3 format-dispatch refusal
  clause below.
- The earlier "ParseContext SHALL determine DocumentKind by
  file extension" requirement's "`.md` (or other)" clause
  (which lumps unknown extensions with Markdown) is superseded
  by the R3 known-supported / known-unsupported / unknown-
  fallback dispatch rules.
- The earlier `NbError` enum and `Public API shall expose
  complete types` exhaustive declarations are superseded by the
  R3 `UnsupportedDocumentFormat` variant addition.
- The earlier "Migration table for NbError" new-variant
  inventory (which omits `UnsupportedDocumentFormat`) is
  superseded by the R3 migration note.

Where the R3 revision does not address a clause, the earlier
specification remains the contract for Markdown parsing. The
R3 revision adds the format-dispatch layer that gates which
Markdown parser receives a given input.

**Format dispatch precedes byte parsing.** Format dispatch
returns `NbError::UnsupportedDocumentFormat` for recognized-
but-unsupported extensions **before byte parsing is attempted**.
A `.org` file with empty bytes returns
`UnsupportedDocumentFormat`, not `MissingTitle`. A supported
extension (e.g., `.todo.md`) with empty bytes proceeds to byte
parsing and returns `NbError::ParseError { kind: MissingTitle,
location: 0..0 }` per the existing Todo contract.

**`Explicit(DocumentKind)` bypasses path dispatch.**
`ParseContext::Explicit(DocumentKind)` is treated as Markdown
without going through format-dispatch. `Explicit` can never
produce `NbError::UnsupportedDocumentFormat`, but remains
subject to the selected Markdown kind's ordinary parse failures
(e.g., `Explicit(DocumentKind::Todo)` with empty bytes returns
`MissingTitle`).

### Requirement: Parse shall reject recognized but unsupported nb formats

P1 supports Markdown files only. Files whose **final filename**
matches one of the recognized-but-unsupported dotted suffixes
SHALL be rejected with
`NbError::UnsupportedDocumentFormat { extension, supported }`
where:

- `extension: String` is the **lowercase** dotted suffix that
  matched the rejection list (without the leading dot),
  e.g., `"org"`, `"latex"`, `"tex"`, `"adoc"`, `"asciidoc"`.
- `supported: Vec<String>` is the **owned** canonical list of
  supported extensions, populated at construction time from
  `SUPPORTED_DOCUMENT_EXTENSIONS`. The owned `Vec<String>`
  shape is required for the derived `Serialize` / `Deserialize`
  round-trip contract on `NbError`.

The recognized-but-unsupported suffixes are (using literal
dotted boundaries): `.org`, `.latex`, `.tex`, `.adoc`,
`.asciidoc`. These coincide with `nb` CLI's format recognition
(regexes `(\.latex$|\.tex$)`, `\.org$`, `(\.adoc$|\.asciidoc$)`)
and have format-specific title grammars that Markdown parsing
cannot recognize.

Files whose final filename does NOT match any recognized
suffix (supported or unsupported) SHALL be treated as
**Markdown Note** (permissive). File extensions are a hint,
not a contract.

#### Scenario: org file rejected

GIVEN a path P whose final filename ends in `.org`

WHEN `parse(bytes, FromPath(P))` is called

THEN the result SHALL be
`Err(NbError::UnsupportedDocumentFormat { extension: "org".to_string(), supported: nb_api::SUPPORTED_DOCUMENT_EXTENSIONS.iter().map(|s| s.to_string()).collect() })`.

#### Scenario: latex file rejected

GIVEN a path P whose final filename ends in `.latex`

WHEN `parse(bytes, FromPath(P))` is called

THEN the result SHALL be
`Err(NbError::UnsupportedDocumentFormat { extension: "latex".to_string(), supported: ... })`.

#### Scenario: tex file rejected

GIVEN a path P whose final filename ends in `.tex`

WHEN `parse(bytes, FromPath(P))` is called

THEN the result SHALL be
`Err(NbError::UnsupportedDocumentFormat { extension: "tex".to_string(), supported: ... })`.

#### Scenario: adoc file rejected

GIVEN a path P whose final filename ends in `.adoc`

WHEN `parse(bytes, FromPath(P))` is called

THEN the result SHALL be
`Err(NbError::UnsupportedDocumentFormat { extension: "adoc".to_string(), supported: ... })`.

#### Scenario: asciidoc file rejected

GIVEN a path P whose final filename ends in `.asciidoc`

WHEN `parse(bytes, FromPath(P))` is called

THEN the result SHALL be
`Err(NbError::UnsupportedDocumentFormat { extension: "asciidoc".to_string(), supported: ... })`.

#### Scenario: unrecognized extension normalized to Markdown

GIVEN a path P whose final filename ends in `.txt`

WHEN `parse(bytes, FromPath(P))` is called

THEN `parse` SHALL return `Ok(NoteDocument)` with `kind ==
DocumentKind::Note` (Markdown parsing is permissive).

### Requirement: Extension matching uses literal dotted suffix boundaries with longest-first precedence

Matching is performed against the **final filename** (the
component after the last `/` or `\` separator). The matching
algorithm walks the supported and rejected lists in
**longest-first precedence** and accepts the FIRST match. Each
entry uses a literal dotted suffix boundary (the matched
suffix begins at a literal dot boundary and ends at the end of
the final filename), so `notbookmark.md` does NOT match
`.bookmark.md`. Longest-first precedence then selects
`.bookmark.md`/`.todo.md` before `.md`.

The evaluation order is:

1. `.bookmark.md` → `DocumentKind::Bookmark`
2. `.todo.md` → `DocumentKind::Todo`
3. `.markdown` → `DocumentKind::Note`
4. `.md` → `DocumentKind::Note`
5. `.org` → rejected
6. `.latex` → rejected
7. `.tex` → rejected
8. `.adoc` → rejected
9. `.asciidoc` → rejected
10. Anything else (including no extension) → `DocumentKind::Note`
    (permissive fallback)

The matched lowercase suffix is the value carried in the
`extension` field of `UnsupportedDocumentFormat`. Bare `.todo`
(without `.md`) is NOT mutation-authoritative Todo and resolves
to step 10 (Note); the bare `.todo` extension is not in the
match lists.

#### Scenario: longest-first precedence

GIVEN a path P whose final filename ends in `.bookmark.md`

WHEN `parse(bytes, FromPath(P))` is called

THEN `kind == DocumentKind::Bookmark`.

The `.bookmark.md` rule (step 1) is matched before falling
through to `.md` (step 4). The longest-first precedence
ensures that `x.todo.md` does not resolve to `Note` purely
on the trailing `.md`.

#### Scenario: notbookmark.md does not match .bookmark.md

GIVEN a path P whose final filename ends in `notbookmark.md`
(no leading dot before `bookmark.md`)

WHEN `parse(bytes, FromPath(P))` is called

THEN `kind == DocumentKind::Note` (the `.bookmark.md` rule
requires a literal dotted boundary between the stem and the
suffix).

#### Scenario: bare `.todo` is Note, not Todo

GIVEN a path P whose final filename ends in `.todo` (no `.md`)

WHEN `parse(bytes, FromPath(P))` is called

THEN `kind == DocumentKind::Note` (step 10 fallback; `.todo`
is not in the match lists).

#### Scenario: directory components do not participate

GIVEN a path P containing slashes (e.g., `a/b/note.md`)

WHEN `parse(bytes, FromPath(P))` is called

THEN matching considers the final filename `note.md` only;
directory components do not participate.

Only the final filename (the component after the last `/` or
`\` separator) is matched. Directory components and any
intermediate dotted components within the filename do not
participate in the match.

### Requirement: Empty filename or non-UTF-8 filename SHALL be normalized to Markdown

A `FromPath` whose final filename is absent (the path has no
final filename component) or whose final filename bytes are
not valid UTF-8 SHALL be treated as Markdown Note (permissive).
These are NOT refusal cases for P1.

The portable dispatch rule:

- If the path has no final filename component → Markdown Note.
- If the final filename bytes are not valid UTF-8 → Markdown Note.
- Otherwise, perform ASCII case-insensitive dotted-suffix
  matching against the final filename per the rule above.

#### Scenario: path with no final filename is Markdown

GIVEN a path P with no final filename component (e.g.,
root path or empty path)

WHEN `parse(bytes, FromPath(P))` is called

THEN `parse` SHALL return `Ok(NoteDocument)` with `kind ==
DocumentKind::Note`.

#### Scenario: non-UTF-8 filename is Markdown

GIVEN a path P whose final filename bytes are not valid UTF-8

WHEN `parse(bytes, FromPath(P))` is called

THEN `parse` SHALL return `Ok(NoteDocument)` (non-UTF-8
filenames are not a refusal case).

#### Scenario: ASCII case-insensitive matching

`x.MD`, `x.MD`, `x.mD`, `x.Md` all match `.md` (step 4).
`x.BOOKMARK.MD` matches `.bookmark.md` (step 1). `x.ORG`
matches `.org` (step 5). The matching is **ASCII
case-insensitive** on the literal dotted suffix.

#### Scenario: uppercase bookmark.md recognized as Bookmark

GIVEN a path P whose final filename ends in `.BOOKMARK.MD`
(or any other case variant)

WHEN `parse(bytes, FromPath(P))` is called

THEN `kind == DocumentKind::Bookmark` (case-insensitive
longest-first match).

(Diverges from `nb`'s `_file_is_bookmark` which is
case-sensitive. This is the deliberate P1-vs-`nb` divergence
documented in Q-D / Option Z.)

#### Scenario: uppercase org is rejected

GIVEN a path P whose final filename ends in `.ORG` (or any
other case variant)

WHEN `parse(bytes, FromPath(P))` is called

THEN the result SHALL be
`Err(NbError::UnsupportedDocumentFormat { extension: "org".to_string(), supported: ... })`.

The carried `extension` value is the lowercase `"org"`
regardless of input case.

### Requirement: ParseContext::Explicit is Markdown for P1

`ParseContext::Explicit(DocumentKind)` sets the **Markdown
document kind** explicitly. It bypasses the path-based format
dispatch and is treated as Markdown without going through
format-dispatch. The format dimension is implicit (Markdown)
for every P1 route.

`Explicit(DocumentKind)` can NEVER produce
`NbError::UnsupportedDocumentFormat` — the format is implicit
and not subject to rejection. However, `Explicit` remains
subject to the selected Markdown kind's ordinary parse failures
(e.g., `Explicit(DocumentKind::Todo)` with empty bytes returns
`NbError::ParseError { kind: MissingTitle, location: 0..0 }`).

If a future proposal (P3+) introduces a non-Markdown
document kind, the public `ParseContext` type would need
revision to carry format. For P1, this is out of scope.

#### Scenario: Explicit(DocumentKind::Bookmark) is Markdown without format rejection

GIVEN `ParseContext::Explicit(DocumentKind::Bookmark)`

WHEN `parse(bytes, ctx)` is called

THEN the result SHALL be a Markdown Bookmark parser
delegation. The format is implicit (Markdown) and not
rejected.

### Requirement: NbError::UnsupportedDocumentFormat is a top-level variant

`NbError::UnsupportedDocumentFormat` is a top-level error
variant, not nested inside `ParseError`. The contextual
"this file's format is not supported by P1" rejection is
not a parse failure (no bytes were consumed and no
parse-error location is meaningful); it is a contextual
configuration error.

```rust
#[error("unsupported document format: {extension:?} (supported: {supported:?})")]
UnsupportedDocumentFormat {
    extension: String,
    supported: Vec<String>,
},
```

`extension` is the lowercase dotted suffix that matched the
rejection list (without the leading dot).
`supported: Vec<String>` is the owned canonical supported
extension list, populated at construction time from
`nb_api::SUPPORTED_DOCUMENT_EXTENSIONS`.

The `nb_api::SUPPORTED_DOCUMENT_EXTENSIONS` constant is the
source of truth for the supported list. It is exposed at the
**crate root** (one stable public path), even though the
implementation may define it in `parser` and `pub use` it:

```rust
// Defined in `parser` module:
pub const SUPPORTED_DOCUMENT_EXTENSIONS: &[&str] =
    &["md", "markdown", "todo.md", "bookmark.md"];

// Re-exported at the crate root for stable public access:
pub use parser::SUPPORTED_DOCUMENT_EXTENSIONS;
```

Consumers SHALL refer to the constant as
`nb_api::SUPPORTED_DOCUMENT_EXTENSIONS` (crate root) and not
via the `parser` module internal path. The crate-root re-export
is the canonical stable import path.

The owned `Vec<String>` shape in the error variant is required
for the derived `Serialize` / `Deserialize` round-trip contract
on `NbError`. A borrowed `&'static [&'static str]` cannot
satisfy the general JSON equality round trip.

#### Scenario: UnsupportedDocumentFormat is a top-level NbError variant

GIVEN a `.org` file

WHEN `parse` is called

THEN the result SHALL be
`Err(NbError::UnsupportedDocumentFormat { extension: "org".to_string(), supported: nb_api::SUPPORTED_DOCUMENT_EXTENSIONS.iter().map(|s| s.to_string()).collect() })`.

The error is NOT a `NbError::ParseError`; it is a top-level
variant.

#### Scenario: UnsupportedDocumentFormat variant derives Serialize/Deserialize round-trip

GIVEN `err = NbError::UnsupportedDocumentFormat { extension: "org".to_string(), supported: vec!["md".to_string(), "markdown".to_string(), "todo.md".to_string(), "bookmark.md".to_string()] }`

WHEN serialized to JSON and deserialized back

THEN the result SHALL equal `err` (the `Vec<String>` shape
ensures the general NbError serde round-trip holds).

### Requirement: Parse shall fail on scoped no-nonblank-line OR unsupported format

The `parse` function SHALL return `Err` for either of:

1. The no-nonblank-line case for Todo and Bookmark (existing
   contract): `NbError::ParseError { kind: MissingTitle, location: 0..0 }`.
2. The format-dispatch refusal case (R3 revision): `NbError::UnsupportedDocumentFormat`.

These failures are mutually exclusive: format dispatch
precedes byte parsing (R3-S3 ordering), so a `.org` file with
empty bytes returns `UnsupportedDocumentFormat`, not
`MissingTitle`.

No other input is rejected at the parse level.

#### Scenario: org file produces UnsupportedDocumentFormat even with empty bytes

GIVEN bytes B and path P whose final filename ends in `.org`

WHEN `parse(B, FromPath(P))` is called

THEN the result SHALL be
`Err(NbError::UnsupportedDocumentFormat { extension: "org".to_string(), supported: ... })`.

The error is NOT `NbError::ParseError`; it is a top-level
variant. Format dispatch precedes byte parsing.

#### Scenario: Markdown file with no title is permissive

GIVEN bytes `b"Just content.\n"` (no title) and path P with
final filename `.md`

WHEN `parse(bytes, FromPath(P))` is called

THEN the result SHALL be `Ok(NoteDocument)` with kind
`DocumentKind::Note` (Markdown parsing is permissive even
without a title).

### Requirement: Resolution table additions for format-dispatch

The R3 revision adds the following cases to the Resolution
Table:

| Case | Result | Notes |
|------|--------|-------|
| F1: `.org` file parsed | `Err(NbError::UnsupportedDocumentFormat { extension: "org".to_string(), ... })` | recognized-but-unsupported |
| F2: `.latex` file parsed | `Err(NbError::UnsupportedDocumentFormat { extension: "latex".to_string(), ... })` | recognized-but-unsupported |
| F3: `.tex` file parsed | `Err(NbError::UnsupportedDocumentFormat { extension: "tex".to_string(), ... })` | recognized-but-unsupported |
| F4: `.adoc` file parsed | `Err(NbError::UnsupportedDocumentFormat { extension: "adoc".to_string(), ... })` | recognized-but-unsupported |
| F5: `.asciidoc` file parsed | `Err(NbError::UnsupportedDocumentFormat { extension: "asciidoc".to_string(), ... })` | recognized-but-unsupported |
| F6: `.txt` file parsed | `Ok(NoteDocument)` with `kind == Note` | unknown -> Markdown |
| F7: `.MD` file parsed | `Ok(NoteDocument)` with `kind == Note` | case-insensitive |
| F8: `.ORG` file parsed | `Err(NbError::UnsupportedDocumentFormat { extension: "org".to_string(), ... })` | case-insensitive rejected |
| F9: `.BOOKMARK.MD` file parsed | `Ok(NoteDocument)` with `kind == Bookmark` | case-insensitive compound |
| F10: path with no final filename | `Ok(NoteDocument)` with `kind == Note` | permissive |
| F11: path with non-UTF-8 final filename | `Ok(NoteDocument)` with `kind == Note` | permissive |
| F12: `notbookmark.md` (no leading dot) | `Ok(NoteDocument)` with `kind == Note` | does not match `.bookmark.md` (literal dotted boundary) |
| F13: `.TODO.MD` file parsed | `Ok(NoteDocument)` with `kind == Todo` | case-insensitive compound |
| F14: `.markdown` file parsed | `Ok(NoteDocument)` with `kind == Note` | supported via `.markdown` |

### Requirement: Public API addition for UnsupportedDocumentFormat

The public NbApi surface SHALL add:

- `NbError::UnsupportedDocumentFormat { extension: String, supported: Vec<String> }`
  (top-level variant of `NbError`).
- `pub const SUPPORTED_DOCUMENT_EXTENSIONS: &[&str] = &["md", "markdown", "todo.md", "bookmark.md"]`
  exposed at the **crate root** as `nb_api::SUPPORTED_DOCUMENT_EXTENSIONS`
  (source of truth for the supported list).

The existing public API for `NbError` is changed by the
addition (it is a new top-level variant). This is a
**public-API breaking change** for downstream consumers
that match `NbError` exhaustively. The constant is exposed
at the crate root (not on `DocumentKind` and not on `parser`
as a module-only path) so the public import path is stable.

#### Scenario: UnsupportedDocumentFormat is a public variant

WHEN a downstream consumer matches on `NbError::UnsupportedDocumentFormat { extension, supported }`

THEN the variant SHALL be in scope and the fields SHALL be
accessible per the public API.

#### Scenario: Migration note for UnsupportedDocumentFormat (compile error, not warning)

This is a NEW variant. There is no `nb-api 0.2.x` predecessor.
Consumers upgrading to `nb-api 0.3.0` SHALL add the variant
to their match arms. **Adding a variant to an exhaustive
enum match produces a compile error, not a warning;
wildcard matches continue compiling.** The migration table
below is updated to include `UnsupportedDocumentFormat`
alongside the other new variants.
