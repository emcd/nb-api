<!-- nbspec: change=update-api-0-4-0-omnibus notebook=nb-api note=proposals/update-api-0-4-0-omnibus/specifications/text-first_types_—_remove_bytestring.md hash=sha256:1bc5280a3b3befaafdd6b6af827891ede3b156ebf9a15a7e2772d6da54bac7fa -->
# Text-first types — remove ByteString, String fields, NonUtf8, escape hatch

#nbspec #specification

## Purpose

Replace the base64-everywhere `ByteString` surface with text-first `String` fields, make the library base64-free, and add a typed UTF-8 boundary plus a raw-bytes escape hatch. Make body vs source computable via fragment offsets.

## ADDED Requirements

### Requirement: ByteString SHALL be removed

The crate SHALL NOT expose `pub struct ByteString` or its `base64` JSON form. All code, docs, and `schemars` schemas that previously referenced `ByteString` SHALL be removed or rewritten to `String`/`Vec<u8>`.

#### Scenario: ByteString not in public API

- **WHEN** a consumer depends on `nb-api` 0.4.0
- **THEN** `nb_api::ByteString` SHALL be unresolved
- **AND** `cargo doc` for 0.4.0 SHALL contain no `ByteString` type

### Requirement: Structured textual fields SHALL be String

The following public fields SHALL be `String` (JSON string), not a wrapper:

- `ShowNote.title` (Option<String> — full raw title line including trailing newline, as UTF-8)
- `ShowNote.body` (String — concatenated body-fragment bytes, UTF-8)
- `ShowNote.source` (String — full file bytes, UTF-8)
- `BodyFragment.bytes` (String)
- `NoteLine.text` (String — line without terminator)
- `NoteLineHit.text` (Option<String>)
- `LineEdit.content` for Insert/Replace (String — bare text, no terminator included)
- Substring `pattern` / `replacement` (String)

All serialized JSON for these fields SHALL be JSON strings.

#### Scenario: ShowNote String round-trip

- **WHEN** `ShowNote { body: "hello\n" }` is serialized to JSON and deserialized
- **THEN** `body` SHALL equal `"hello\n"`

### Requirement: Text-only surface SHALL validate UTF-8 and return typed NonUtf8

Any structured read or write that operates on a note file classified as text (via `nb show --type text` probe or direct file read) SHALL validate that the relevant bytes are UTF-8. On failure it SHALL return `NbError::NonUtf8 { selector, path, kind, mime_hint }` where `kind` is the detected `DocumentKind` string and `mime_hint` is the probe's MIME when available. For notes whose type is non-textual (folders, media, etc.), the existing `NbError::UnsupportedShowTarget` remains the boundary.

#### Scenario: Non-UTF-8 text file returns NonUtf8

- **WHEN** a file classified as `text` contains bytes `0xff 0xfe` in its body
- **AND** `show_note` or `show_note_lines` is called
- **THEN** the call SHALL fail with `NbError::NonUtf8`
- **AND** `selector` and `path` SHALL identify the target

### Requirement: Raw-bytes escape hatch SHALL exist

The crate SHALL expose `NbClient::read_note_source_bytes(NoteTarget) -> Result<Vec<u8>, NbError>` (and a `Transaction`-level byte accessor where commit-time validation needs it) returning native `Vec<u8>` without UTF-8 validation or base64. This is the only public API that returns raw bytes for note content.

#### Scenario: Escape hatch returns exact bytes for non-UTF-8 file

- **WHEN** a text-classified file contains non-UTF-8 bytes
- **AND** `read_note_source_bytes` is called
- **THEN** it SHALL return `Vec<u8>` equal to the file's bytes

### Requirement: ShowNote.body vs source SHALL be computable via fragment offsets

`ShowNote` SHALL expose:

- `body: String` — concatenated `body_ranges` bytes in source order (excludes title H1 + prefix tags/URL/separator/tag-section; fingerprint domain)
- `source: String` — full file bytes verbatim
- `body_fragments: Vec<BodyFragment>` where `BodyFragment { index: u32, bytes: String, start_byte: u32, end_byte: u32 }` with `start_byte`/`end_byte` offsets into `source` (same vocabulary as `NoteLineHit`). Then `body = concat(fragment.bytes)` and `body == source[start_byte..end_byte]` joined, and `body_contiguous` is derivable (`fragments.len() <= 1`). No `body_includes_title` boolean is needed.

`fingerprint` SHALL remain `b3:` over `body` bytes (see `note-document-model` spec).

#### Scenario: Body excludes title H1 and is computable

- **WHEN** a note file begins with `# Title\n` and body is `hello\n`
- **AND** `show_note` returns `ShowNote`
- **THEN** `body` SHALL equal `hello\n` and not contain `# Title\n`
- **AND** `source` SHALL contain `# Title\n`
- **AND** `body_fragments[0].start_byte` SHALL equal offset of `hello` in `source`

### Requirement: Contiguous-body ops SHALL return FragmentedBody

Ops that require a contiguous body (`show_note_lines`, `search_note_lines`, `edit_note_lines`, `edit_note_substring`, `replace_note_body`) SHALL fail with typed `NbError::FragmentedBody { fragment_count, guidance }` when `body_fragments.len() >= 2` (bookmark with split tags). This is the existing `body-aware-editing` contract carried into 0.4.0 (not a new shape).

#### Scenario: Line read on fragmented bookmark returns FragmentedBody

- **WHEN** `show_note_lines` targets a bookmark with two body fragments
- **THEN** it SHALL fail `FragmentedBody` with `fragment_count == 2`

### Requirement: Migration note SHALL document the break

`documentation/migration-0.4.0.md` SHALL list every field/type changed from `ByteString` to `String`, the new `NonUtf8` error, and the escape hatch. `documentation/migration-0.3.0.md` SHALL be removed per `todos/api/7` in the same change.

#### Scenario: Migration covers ByteString removal

- **WHEN** `documentation/migration-0.4.0.md` is reviewed
- **THEN** it SHALL contain a table mapping each 0.3.x `ByteString` field to its 0.4.0 `String` field and the `NonUtf8` guidance
