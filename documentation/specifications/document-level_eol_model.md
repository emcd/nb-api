<!-- nbspec: change=update-api-0-4-0-omnibus notebook=nb-api note=proposals/update-api-0-4-0-omnibus/specifications/document-level_eol_model.md hash=sha256:323df7b639ee7b19c15c5cd6b365e6510a8b0036b97463da18fc91856c295db5 -->
# Document-level EOL model — replace per-line terminator

#nbspec #specification

## Purpose

Replace per-line `NoteLine.terminator` waste with a single document-level EOL declaration, fixing mixed-EOL semantics and anchor stability for LLM-facing reads and edits.

## ADDED Requirements

### Requirement: ShowNoteLines SHALL expose document-level eol

`ShowNoteLines` SHALL have fields:

- `eol: Option<LineEol>` where `LineEol` is enum `Lf | CrLf` (serde `rename_all = "lowercase"`; JSON `"lf"` / `"crlf"`)
- `has_final_eol: bool`

`eol` is `Some` when body contains at least one supported EOL (`\n` or `\r\n`); `None` for empty, single-line-no-EOL, or CR-only bodies (bare `\r` is not a supported EOL). `has_final_eol` indicates body ends with `eol` bytes.

#### Scenario: Lf body reports document eol

- **WHEN** body is `"a\nb\n"`
- **THEN** `ShowNoteLines.eol` SHALL be `Some(Lf)`
- **AND** `has_final_eol` SHALL be true

### Requirement: Bare CR SHALL NOT be treated as EOL and SHALL be preserved verbatim

Bare `\r` (not part of `\r\n`) SHALL NOT be considered a line terminator and SHALL NOT be normalized to `\n`. It SHALL remain part of `NoteLine.text` and be preserved verbatim on round-trip and on edit-write. The library SHALL only split on supported EOLs (`\r\n` and `\n`); CR-only files are therefore a single line with embedded `\r` bytes.

#### Scenario: CR-only body is single line with preserved CR

- **WHEN** stored body bytes are `"a\rb\rc"` (CR-only, no `\n`)
- **THEN** `ShowNoteLines.eol` SHALL be `None`
- **AND** `lines` SHALL be `[{number:1,text:"a\rb\rc"}]`
- **AND** `has_final_eol` SHALL be false
- **AND** line 1 anchor SHALL equal `LineAnchor::from_line_bytes(b"a\rb\rc\x00")`
- **AND** after a read round-trip with no edits the file bytes SHALL remain `"a\rb\rc"`

#### Scenario: CR-only with trailing CR preserved

- **WHEN** stored body bytes are `"a\rb\r"` (CR-only)
- **THEN** `lines` SHALL be `[{number:1,text:"a\rb\r"}]` with `eol: None`, `has_final_eol: false`
- **AND** file bytes SHALL remain `"a\rb\r"` after round-trip

### Requirement: EOL detection SHALL use first supported occurrence

The body EOL SHALL be determined by the first **supported** EOL occurrence (`\r\n` or `\n`) scanning left-to-right. Bare `\r` is not a supported EOL and is stray content. The body SHALL be split only on the declared `eol` (`\r\n` or `\n`); bare `\r` remains in line text.

#### Scenario: Stray CR before first supported EOL preserved

- **WHEN** stored body bytes are `"a\rb\nc\n"` (stray CR before first `\n`)
- **THEN** `eol` SHALL be `Some(Lf)`
- **AND** `lines` SHALL be `[{number:1, text:"a\rb"}, {number:2, text:"c"}]`
- **AND** `has_final_eol` SHALL be true

#### Scenario: Mixed body uses first supported EOL

- **WHEN** body is `"a\nb\r\nc\n"` (first supported is Lf)
- **THEN** `eol` SHALL be `Some(Lf)`
- **AND** line 2 text SHALL contain `"\r"` (stray CR preserved as part of `b\r`)

### Requirement: NoteLine SHALL be terminator-free

`NoteLine` SHALL have fields `number: u32` (1-based), `anchor: LineAnchor`, `text: String` (line without terminator). It SHALL NOT have `terminator`. Sequences that previously used `terminator` now use document `eol` + `has_final_eol`.

#### Scenario: NoteLine has no terminator field

- **WHEN** `NoteLine` is serialized to JSON
- **THEN** the JSON SHALL NOT contain `terminator`
- **AND** it SHALL contain `number`, `anchor`, `text`

### Requirement: LineAnchor SHALL hash the exact byte span

`LineAnchor` for each body line SHALL be `BLAKE3` over the line's exact byte span: `text` + `eol` bytes (`\n` or `\r\n`) for all lines except possibly the last; last line when `has_final_eol == false` uses `text` + single `0x00` byte (input-only marker, not written). Thus flipping document `eol` changes all anchors as corollary, and Dollar insertion when `has_final_eol == false` changes the previous final line's anchor (its span gained a terminator).

#### Scenario: Flipping document EOL changes all anchors

- **WHEN** body `"a\nb\n"` (Lf) changes to `"a\r\nb\r\n"` (CrLf) with same text lines
- **THEN** every `NoteLine.anchor` SHALL change

#### Scenario: Final line without EOL uses 0x00 marker

- **WHEN** body is `"a\nb"` (`eol: Some(Lf)`, `has_final_eol: false`)
- **THEN** line 2 anchor SHALL equal `LineAnchor::from_line_bytes(b"b\x00")`

#### Scenario: Dollar insertion changes previous final line anchor

- **WHEN** body is `"a\nb"` (`has_final_eol: false`, anchors `hash(a\n)`, `hash(b\x00)`) and `Insert` at `Dollar` with `"c"` produces `"a\nb\nc\n"`
- **THEN** re-read line 2 anchor SHALL equal `hash(b\n)` not `hash(b\x00)`

### Requirement: LineEdit content SHALL be bare text

`LineEdit::Insert` and `LineEdit::Replace` SHALL take `content: String` as bare text (no terminator included, but may contain embedded `\r` which is preserved). The library SHALL append document `eol` bytes when materializing edits. Empty `content` is allowed for Insert (inserts a blank line).

#### Scenario: Insert appends document EOL

- **WHEN** `ShowNoteLines.eol` is `Some(CrLf)` and `LineEdit::Insert { at: After(line1), content: "x" }` is committed
- **THEN** the resulting file SHALL contain `"x\r\n"` inserted after line 1

### Requirement: Edits on eol None documents SHALL adopt Lf

When `eol` is `None` (empty or CR-only) and an edit materializes a new line boundary, the document SHALL become `eol: Some(Lf)` and `has_final_eol: true`. Read-only round-trips stay faithful (CR preserved), but any edit introduces Lf. This includes `Insert After(line 1)` on a CR-only body.

#### Scenario: Insert After on CR-only adopts Lf

- **WHEN** body is `"a\rb\rc"` (`eol: None`) and `Insert After(line 1)` with `"x"` is committed
- **THEN** resulting file bytes SHALL be exactly `"a\rb\rc\nx\n"`
- **AND** re-read `eol` SHALL be `Some(Lf)` and `has_final_eol` true

### Requirement: Dollar insertion with no final EOL SHALL normalize to trailing EOL

`LinePosition::Boundary { at: Caret }` SHALL mean start of body; `Boundary { at: Dollar }` SHALL mean end of body. For `Dollar` when `has_final_eol == false`, an `Insert` SHALL produce a file with `has_final_eol == true`: the previous final line is terminated with `eol` bytes (or `\n` if `eol` was `None`), then `content + eol` is appended. For empty body (`total_lines == 0`, `eol: None`), `Caret` is the only valid boundary; `Insert` at `Caret` with `content` SHALL produce `content + "\n"` and `eol: Some(Lf)`, `has_final_eol: true`. `Replace` of the final line when `has_final_eol == false` with new content SHALL end with `eol` (has_final_eol true).

#### Scenario: Insert at Dollar when has_final_eol is false produces trailing EOL

- **WHEN** body is `"a\nb"` (`eol: Some(Lf)`, `has_final_eol: false`) and `Insert` at `Dollar` with `"c"` is committed
- **THEN** resulting file bytes SHALL be exactly `"a\nb\nc\n"`
- **AND** re-read `ShowNoteLines.has_final_eol` SHALL be true
- **AND** re-read line 3 anchor SHALL equal `LineAnchor::from_line_bytes(b"c\n")`

#### Scenario: Insert at Dollar into empty body

- **WHEN** body is `""` (`eol: None`, `total_lines: 0`) and `Insert` at `Caret` with `"x"` is committed
- **THEN** resulting file bytes SHALL be exactly `"x\n"`
- **AND** re-read `eol` SHALL be `Some(Lf)` and `has_final_eol` true

#### Scenario: Replace final line without EOL normalizes to trailing EOL

- **WHEN** body is `"a\nb"` (`has_final_eol: false`) and `Replace` start line 2 end line 2 with `"c"` is committed
- **THEN** resulting file bytes SHALL be exactly `"a\nc\n"`

### Requirement: Internal splitter SHALL match first-supported occurrence and preserve bare CR

The internal body splitter (`src/lines.rs`) SHALL use first-supported-occurrence `eol` detection and SHALL preserve bare `\r` verbatim (no split, no normalization), so read and write never disagree and CR round-trips faithfully.

#### Scenario: Splitter preserves CR-only as single line

- **WHEN** body bytes are `"a\rb\r"` (CR-only)
- **THEN** splitter and `ShowNoteLines.lines` SHALL both yield 1 line `["a\rb\r"]` with `eol: None`, `has_final_eol: false`
