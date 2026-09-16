<!-- nbspec: change=add-body-aware-note-editing notebook=nb-api note=proposals/add-body-aware-note-editing/specifications/body-aware-editing.md hash=sha256:4bb510b22cad7980c1511e873ee64f2cac5faa0d63175d5a7273600421f25903 -->
# Body-aware editing surface specification

#nbspec #specification

## Purpose

Structured reads and body/metadata mutation plan ops with
**exact** public type names and JSON wire forms for MCP lockstep.
Mutators are methods on `Transaction` (see `notebook-transaction`
inventory); `NbClient` exposes one-shot wrappers. Legacy
`edit_note`/`EditMode` are removed.

## ADDED Requirements

### Requirement: Exact public type names

The crate SHALL expose these public types under these exact names
(no deferred aliasing):

- `ShowNote`
- `ShowNoteLines`
- `NoteLine`
- `LineAnchor`
- `SearchNoteLines`
- `NoteLineHit`
- `NoteTarget`
- `ByteString`
- `LineRef`
- `LinePosition`
- `LineEdit`
- `Occurrence`
- `BodyFragment`
- `CommitOutcome` (also required by `notebook-transaction`)
- `OpOutcome`

All of the above SHALL implement `Serialize` and `Deserialize`.
`LineAnchor`, `Fingerprint`, and enum types used on the wire SHALL
have the JSON forms defined below.

#### Scenario: Public names are stable

- **WHEN** a consumer imports body-edit types from `nb_api` 0.3.0
- **THEN** the type names listed above SHALL resolve
- **AND** each SHALL round-trip through serde JSON per this spec

### Requirement: ByteString JSON representation

`ByteString` is the public wrapper for arbitrary file bytes on the
wire:

```json
{ "base64": "<standard base64 of raw bytes>" }
```

Rust fields of type `ByteString` serialize as that object. There is
no parallel lossy-only title field without bytes: where text is
offered, it is in addition to bytes.

#### Scenario: ByteString round-trip preserves bytes

- **WHEN** a `ByteString` holds bytes `[0xff, 0x0a]`
- **AND** it is serialized to JSON and deserialized
- **THEN** the raw bytes SHALL equal `[0xff, 0x0a]`

### Requirement: NoteTarget addressing

Operations that name an existing note SHALL take exactly one
`NoteTarget` value:

```json
{ "type": "selector", "value": "home:123" }
```
or
```json
{ "type": "path", "value": "folder/note.md" }
```

Serde: internally tagged with `"type"`. `value` is a string.
`CommitOutcome` / `OpOutcome` SHALL echo both `selector` and `path`
when both are known after apply; errors that name a target SHALL
use the same `NoteTarget` form the caller supplied when available.

#### Scenario: Selector and path targets are distinct wire forms

- **WHEN** a client sends `{ "type": "path", "value": "a.md" }`
- **THEN** deserialization SHALL yield `NoteTarget::Path("a.md")`
- **AND** not a selector variant

### Requirement: show_note returns ShowNote

`NbClient::show_note` SHALL return `ShowNote` with exact fields:

| Field | Type | JSON |
|---|---|---|
| `selector` | `String` | string |
| `path` | `String` | notebook-relative path string |
| `kind` | `DocumentKind` | `"note"` \| `"todo"` \| `"bookmark"` (serde rename_all = "lowercase" or existing crate convention documented as these three strings) |
| `todo_state` | `Option<TodoState>` | `"open"` \| `"done"` \| null |
| `title` | `Option<ByteString>` | null or ByteString of raw title line bytes including trailing newline when present |
| `title_text` | `Option<String>` | UTF-8 lossy convenience; null if no title; MUST NOT be the sole title authority |
| `tags` | `Vec<String>` | JSON array of tag strings without requiring `#` prefix normalization beyond what `tags()` yields decoded lossily; raw tokens also available via document parse |
| `body_fragments` | `Vec<BodyFragment>` | ordered P1 body fragments |
| `body_contiguous` | `bool` | true iff `body_fragments.len() <= 1` |
| `body` | `ByteString` | concatenation of fragment bytes in source order (fingerprint domain) |
| `fingerprint` | `Fingerprint` | string `"b3:"` + 64 lowercase hex |
| `source` | `ByteString` | full file bytes |

`BodyFragment` fields:

| Field | Type |
|---|---|
| `index` | `u32` zero-based |
| `bytes` | `ByteString` |

Title or tag metadata changes MUST NOT change `fingerprint` when
concatenated body fragment bytes are unchanged.

#### Scenario: Fingerprint matches P1 body hash

- **WHEN** `show_note` returns a result for a parseable note
- **THEN** `result.fingerprint` SHALL equal
  `fingerprint(parse(source_bytes, FromPath(path)))`

#### Scenario: Fragmented bookmark exposes multiple fragments

- **WHEN** a Bookmark has Tags between Content and Source body
  regions (P1 multi-fragment case)
- **THEN** `body_contiguous` SHALL be false
- **AND** `body_fragments.len()` SHALL be greater than 1
- **AND** `body` SHALL equal the concatenation of fragment bytes
  in source order

### Requirement: v1 body line/search/substring/replace require contiguous body

`show_note_lines`, `search_note_lines`, `edit_note_lines`,
`edit_note_substring`, and `replace_note_body` SHALL operate only
when the parsed document has a **contiguous** body domain:
`body_fragments.len() <= 1` (zero or one fragment).

If `body_fragments.len() >= 2`, these operations SHALL fail with
typed error kind `FragmentedBody` (stable name) without writing.
Guidance: use metadata ops (`retitle_note`, `edit_note_tags`) or a
future multi-fragment API; do not concatenate for editing.

Rationale: concatenated fragment bytes are not one physical source
span; line anchors and byte edits cannot map through excluded
Tags/separator bytes without a fragment-aware model. v1 refuses
rather than guess.

`retitle_note` and `edit_note_tags` remain allowed on fragmented
documents because they mutate metadata partitions only.

#### Scenario: Line read refuses fragmented Bookmark

- **WHEN** `show_note_lines` is called on a Bookmark with two or
  more body fragments
- **THEN** the call SHALL fail with `FragmentedBody`
- **AND** no partial line list SHALL be returned as success

#### Scenario: replace_note_body refuses fragmented Bookmark

- **WHEN** `replace_note_body` is committed against a fragmented
  body
- **THEN** `commit` SHALL fail with `FragmentedBody`
- **AND** source bytes SHALL be unchanged

#### Scenario: Contiguous Note allows line edits

- **WHEN** a Note has a single body fragment
- **THEN** `show_note_lines` and `edit_note_lines` SHALL be
  permitted subject to other validation

#### Scenario: Content Tags Source Bookmark regression

- **WHEN** a Bookmark contains Content, Tags, and Source sections
  yielding multiple body fragments
- **THEN** body mutation APIs SHALL refuse with `FragmentedBody`
- **AND** `edit_note_tags` SHALL still be able to modify Tags
  metadata without synthesizing Content/Source bytes into body
  lines

### Requirement: Line anchors are versioned and deterministic

`LineAnchor` serializes as a JSON string:

```text
b3l1:<32 lowercase hex>
```

where `<32 hex>` is the first 32 hex characters (128 bits) of
BLAKE3-256 over the **exact body-line byte sequence including its
line terminator as stored**.

Terminator encoding in the hash input:

- CRLF line: content + `\r\n`
- LF line: content + `\n`
- CR line: content + `\r`
- Final line with no terminator: content + single `0x00` EOF marker
  (hash input only; not written to the file)

Unknown prefixes SHALL be rejected at verification.

#### Scenario: Identical line bytes produce identical anchors

- **WHEN** two contiguous bodies have the same line bytes and
  terminator
- **THEN** anchors SHALL be equal

#### Scenario: Terminator change changes anchor

- **WHEN** only a line's terminator changes from LF to CRLF
- **THEN** the line's anchor SHALL change

### Requirement: show_note_lines enumerates body-relative lines

`show_note_lines(target, offset, limit)` SHALL return `ShowNoteLines`
only for contiguous bodies.

`NoteLine` JSON object fields:

| Field | Type | JSON |
|---|---|---|
| `number` | `u32` | 1-based body line number |
| `anchor` | `LineAnchor` | string |
| `text` | `ByteString` | line without terminator |
| `terminator` | enum | `"lf"` \| `"crlf"` \| `"cr"` \| `"none"` |

`ShowNoteLines` fields:

| Field | Type |
|---|---|
| `selector` | `String` |
| `path` | `String` |
| `kind` | `DocumentKind` |
| `total_lines` | `u32` |
| `offset` | `u32` |
| `limit` | `u32` |
| `next_offset` | `Option<u32>` |
| `lines` | `Vec<NoteLine>` |
| `title` | `Option<ByteString>` |
| `tags` | `Vec<String>` |
| `body_fingerprint` | `Fingerprint` |

Window: `offset` default 1; `limit` required from MCP callers
(library default MAY be 100 when invoked from Rust without limit).
`offset > total_lines + 1` is `InvalidLineWindow` except empty body
with `offset == 1`.

Title and tags are metadata only; not body lines.

#### Scenario: Body line 1 is not the title H1

- **WHEN** a titled contiguous Note is read with `show_note_lines`
- **THEN** line 1 text SHALL be the first body line

### Requirement: Virtual boundary tokens

`LinePosition` JSON (internally tagged on `type`):

```json
{ "type": "before", "line": { "number": 1, "anchor": "b3l1:..." } }
{ "type": "after", "line": { "number": 3, "anchor": "b3l1:..." } }
{ "type": "boundary", "at": "caret" }
{ "type": "boundary", "at": "dollar" }
```

`caret` = start of body; `dollar` = end of body.

#### Scenario: Insert into empty body uses boundary caret

- **WHEN** body has zero lines and INSERT uses
  `{ "type": "boundary", "at": "caret" }`
- **THEN** commit SHALL accept when otherwise valid

### Requirement: search_note_lines returns SearchNoteLines

Request pattern field: `ByteString` non-empty (or UTF-8 string
field `pattern_text` that is encoded as UTF-8 bytes for search —
exactly one of `pattern` ByteString or `pattern_text` string
required). Matching is byte-level on the contiguous body.

`NoteLineHit` fields: `number`, `anchor`, `start_byte`, `end_byte`
(half-open within line `text` bytes), plus optional `text`.

#### Scenario: Search hit anchor is commit-usable

- **WHEN** a hit is returned and body is unchanged
- **THEN** `edit_note_lines` citing that number+anchor SHALL verify

### Requirement: edit_note_lines batch wire form

Plan payload field `edits` is a JSON array of `LineEdit` objects
internally tagged on `type`:

```json
{
  "type": "insert",
  "at": { "type": "before", "line": { "number": 1, "anchor": "b3l1:..." } },
  "content": { "base64": "..." }
}
{
  "type": "delete",
  "start": { "number": 2, "anchor": "b3l1:..." },
  "end": { "number": 3, "anchor": "b3l1:..." }
}
{
  "type": "replace",
  "start": { "number": 2, "anchor": "b3l1:..." },
  "end": { "number": 2, "anchor": "b3l1:..." },
  "content": { "base64": "..." }
}
```

`LineRef` is always `{ "number": <u32>, "anchor": "<LineAnchor string>" }`.

Ranges for delete/replace are **inclusive** on both ends.
Overlaps and multiple inserts at the same boundary fail the whole
transaction. Apply via descending original-snapshot byte spans.

#### Scenario: Inclusive replace of lines 2-3

- **WHEN** replace start=2 end=3 with content bytes for `X\n`
- **THEN** those lines SHALL be replaced accordingly

#### Scenario: Overlapping deletes fail the transaction

- **WHEN** two deletes both cover line 2
- **THEN** commit fails with `OverlappingEdits` and applies nothing

#### Scenario: Line read to line edit JSON round-trip

- **WHEN** a client deserializes `ShowNoteLines`, picks line 1's
  `number` and `anchor`, and serializes a delete `LineEdit` for that
  `LineRef`
- **THEN** serde round-trip of that edit SHALL preserve number and
  anchor strings exactly

### Requirement: edit_note_substring wire form

```json
{
  "target": { "type": "selector", "value": "..." },
  "pattern": { "base64": "..." },
  "replacement": { "base64": "..." },
  "occurrence": { "type": "all" },
  "expected_count": 2,
  "fingerprint": "b3:..." 
}
```

`occurrence` internally tagged:

```json
{ "type": "first" }
{ "type": "all" }
{ "type": "nth", "n": 2 }
```

`n` is 1-based. `pattern` MUST be non-empty. `expected_count` is
required and MUST equal the total non-overlapping left-to-right
match count in the contiguous body. `fingerprint` may be null or
omitted for non-strict mode; when present MUST match.

Optional alternate: `pattern_text` / `replacement_text` UTF-8
strings instead of base64 fields (mutually exclusive with ByteString
fields pair).

#### Scenario: expected_count mismatch refuses

- **WHEN** body has 2 matches and `expected_count` is 1
- **THEN** commit fails `OccurrenceMismatch` with expected=1 actual=2

#### Scenario: Substring request JSON round-trip

- **WHEN** a substring request object is serialized and deserialized
- **THEN** occurrence discriminant, expected_count, and pattern
  bytes SHALL be preserved

### Requirement: replace_note_body wire form

```json
{
  "target": { "type": "path", "value": "n.md" },
  "new_body": { "base64": "..." },
  "fingerprint": "b3:..."
}
```

Fingerprint required (JSON string). Contiguous body only.

#### Scenario: Mismatched fingerprint refuses

- **WHEN** fingerprint does not match
- **THEN** commit fails `FingerprintMismatch`; no write

### Requirement: retitle_note and edit_note_tags

`retitle_note` JSON: `target`, `title` as `ByteString` or
`title_text` string (exactly one). Path unchanged; body fingerprint
unchanged when body bytes unchanged.

`edit_note_tags` JSON: `target`, `add`: string array, `remove`:
string array. Contradictory add/remove of same tag fails at enqueue.

`move_note` remains path/basename/type only.

#### Scenario: Retitle preserves body fingerprint and path

- **WHEN** a transaction only retitles a note and body bytes are
  unchanged
- **THEN** body fingerprint and path SHALL be unchanged

### Requirement: Typed recovery errors for body ops

Stable `NbError` variant names (or nested enum discriminants in
JSON as `"type": "<name>"`) SHALL include:

| Type string | Fields |
|---|---|
| `fingerprint_mismatch` | `target`, guidance |
| `anchor_mismatch` | `target`, `number`, guidance |
| `occurrence_mismatch` | `expected`, `actual` |
| `overlapping_edits` | `indices` array |
| `invalid_line_window` | `offset`, `limit`, `total_lines` |
| `empty_substring_pattern` | |
| `fragmented_body` | `fragment_count`, guidance |
| `unsupported_structure` | `reason` |

#### Scenario: FragmentedBody is distinct from AnchorMismatch

- **WHEN** a fragmented bookmark is line-edited
- **THEN** the error type string SHALL be `fragmented_body`
