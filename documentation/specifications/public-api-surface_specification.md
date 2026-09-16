<!-- nbspec: change=add-0-2-0-foundation notebook=nb-api note=proposals/add-0-2-0-foundation/specifications/public-api-surface_specification.md hash=sha256:3c7baecb8d4936eb3e773e5e179a2448f8f9165057c5e48a36fe8f8b400246d4 -->
## ADDED Requirements

### Requirement: NbClient::add_note SHALL reject duplicate title H1 in note body

`NbClient::add_note` SHALL inspect the first nonblank line of `content`. When
`title` is present AND the first nonblank line is a CommonMark ATX H1 whose
heading text (after stripping any optional closing-hash sequence) equals the
trimmed title, the method SHALL return `NbError::DuplicateTitleHeading` before
invoking any subprocess or notebook side effect.

A CommonMark ATX H1 is:
- 0 to 3 leading spaces (4 or more is an indented code block, NOT a
heading);
- exactly one opening `#` (2 or more is H2 or lower, allowed);
- a required space, tab, or end-of-line after the opening hash
  (`#Title` is NOT an H1);
- the heading text (inline content);
- an optional closing hash sequence: a run of `#`s preceded by a
  space or tab and followed only by spaces or tabs to end of
  line. Literal trailing hashes with no preceding whitespace
  (e.g., `# C#`) are part of the heading text, NOT a closing
  sequence.

The `heading` field on the error variant carries the exact
detected source line (with leading spaces and any closing hash
sequence preserved), not a stripped version. The `title` field
carries the user-supplied value verbatim (with surrounding
whitespace preserved).

#### Scenario: Exact duplicate H1 rejected

- **WHEN** `title = "Title"` and `content` begins with `# Title` (exact
match)
- **THEN** the method SHALL return `Err(NbError::DuplicateTitleHeading {
title, heading })` where `heading` carries the exact detected source line
including the leading `#`.

#### Scenario: Leading blank lines do not affect detection

- **WHEN** `title = "Title"` and `content` begins with blank lines
followed by `# Title`
- **THEN** the method SHALL reject (blank lines are skipped; detection
operates on first nonblank line).

#### Scenario: Different H1 text is allowed

- **WHEN** `title = "Title"` and `content` begins with `# Different
Title`
- **THEN** the method SHALL proceed to subprocess invocation.

#### Scenario: Lower-level headings are allowed

- **WHEN** `title = "Title"` and `content` begins with `## Title`
(level-2 heading)
- **THEN** the method SHALL proceed (only ATX H1 triggers rejection).

#### Scenario: Content without H1 is unchanged

- **WHEN** `title = "Title"` and `content` does not begin with an H1
- **THEN** the method SHALL proceed.

#### Scenario: No title means no validation

- **WHEN** `title` is `None` or empty
- **THEN** the duplicate-title validation SHALL be skipped entirely.

#### Scenario: Leading indentation permitted (0-3 spaces)

- **WHEN** `title = "Title"` and `content` begins with 1 to 3 leading
spaces followed by `# Title` (e.g., `  # Title`, `   # Title`)
- **THEN** the method SHALL reject. Leading indentation up to 3
spaces is part of the ATX H1 grammar.

#### Scenario: Tab delimiter permitted after opening hash

- **WHEN** `title = "Title"` and `content` begins with `#\tTitle` (tab
delimiter between opening hash and text)
- **THEN** the method SHALL reject. A tab is a valid delimiter per
CommonMark.

#### Scenario: Closing hashes stripped before comparison

- **WHEN** `title = "Title"` and `content` begins with `# Title #`,
`# Title ###`, or any other valid ATX closing-hash sequence
- **THEN** the method SHALL reject. The closing hashes are stripped
from the heading text before comparison.

#### Scenario: 4-space indentation is not an H1

- **WHEN** `title = "Title"` and `content` begins with 4 or more
leading spaces followed by `# Title`
- **THEN** the method SHALL proceed. 4+ leading spaces is an
indented code block, not a heading.

#### Scenario: Literal trailing hash is heading text

- **WHEN** `title = "C#"` and `content` begins with `# C#`
- **THEN** the method SHALL reject. The trailing `#` has no preceding
whitespace, so it is part of the heading text (`C#`), not a
closing sequence. The `heading` field carries `# C#` (the exact
source line, NOT stripped).

### Requirement: EditMode SHALL use Overwrite variant with replace alias

The `EditMode` enum SHALL expose a variant named `Overwrite` (not
`Replace`) whose canonical serde serialization is the string
`"overwrite"`. The legacy string `"replace"` SHALL be accepted as a
serde alias on deserialization for backward compatibility with
payloads produced before the variant rename, but it SHALL NOT be
advertised in the derived JSON Schema.

This rename closes the vocabulary trap at the root of
`nb-api:issues/api/6`: callers reading `mode: "replace"` reasonably
expected a substring-style replacement (analogous to
[`str::replace`](std::str::replace) /
[`String::replace`](std::string::String::replace)), but `nb edit --overwrite`
is destructive — it replaces every byte of the note body. Praetor
explicitly passed `mode: "replace"` intending substring replacement;
no omission or default was involved (`nb-mcp-server:issues/mcp/6`).
Renaming the variant to `Overwrite` aligns the Rust API name with
the `nb` CLI flag name (`--overwrite`) and makes the destructive
intent unambiguous at the call site.

The `EditMode` enum exposes three variants, each mapping to a
distinct `nb edit` flag combination:

| Variant | Canonical serialization | Alias | `nb edit` flags | Effect |
|---------|------------------------|-------|-----------------|--------|
| `EditMode::Overwrite` | `"overwrite"` | `"replace"` (compat) | `--overwrite --content <content>` | Replaces every byte of the note body with `<content>`. Destructive: any existing content is lost. |
| `EditMode::Append` | `"append"` | — | `--content <content>` | Appends `<content>` after the existing body. |
| `EditMode::Prepend` | `"prepend"` | — | `--prepend --content <content>` | Prepends `<content>` before the existing body. |

`EditMode` derives `Serialize` and `Deserialize`. The legacy
`"replace"` alias is registered on the `Overwrite` variant via
`#[serde(alias = "replace")]` so legacy payloads continue to
deserialize. Schemars' default enum-schema generation does not
advertise serde aliases, so the derived JSON Schema contains only
the canonical value `"overwrite"`.

`EditMode` continues to derive `Default` with `EditMode::Overwrite`
as the default variant. Requiredness on the consumer side (e.g.,
the `mode` field on `nb-mcp-server`'s `EditArgs`) is a
**consumer-layer concern**, not enforced at the `nb-api` layer.
Downstream consumers that want to require `mode` explicitly SHALL
drop `#[serde(default)]` from their containing struct so a missing
`mode` field is a schema-level rejection.

#### Scenario: Overwrite destroys existing body

- **WHEN** a caller invokes `NbClient::edit_note(id, "fresh body",
EditMode::Overwrite, ...)` against a note whose body is "original content"
- **THEN** the resulting body SHALL be "fresh body" (or its
`nb`-normalized equivalent). The original body SHALL be lost.

#### Scenario: Append preserves existing body

- **WHEN** a caller invokes `NbClient::edit_note(id, "second",
EditMode::Append, ...)` against a note whose body is "first"
- **THEN** the resulting body SHALL contain both "first" and "second".
`nb` MAY normalize trailing newlines and add a blank-line separator.

#### Scenario: Prepend inserts before existing body

- **WHEN** a caller invokes `NbClient::edit_note(id, "first",
EditMode::Prepend, ...)` against a note whose body is "second"
- **THEN** the resulting body SHALL contain both "first" and "second",
with "first" preceding "second". `nb` MAY normalize trailing newlines.

#### Scenario: Canonical serialization is overwrite

- **WHEN** a caller serializes `EditMode::Overwrite` via
`serde_json::to_string`
- **THEN** the resulting string SHALL be `"\"overwrite\""`.

#### Scenario: Legacy replace string deserializes as alias

- **WHEN** a caller deserializes `"\"replace\""` into `EditMode`
- **THEN** the result SHALL be `Ok(EditMode::Overwrite)`. The legacy
string is accepted as a backward-compatibility alias.

#### Scenario: Derived JSON Schema advertises only canonical value

- **WHEN** the `schemars` feature is enabled and a caller generates the
JSON Schema for `EditMode` via `schemars::schema_for!`
- **THEN** the schema SHALL advertise the canonical values
`"overwrite"`, `"append"`, `"prepend"` and SHALL NOT advertise the legacy alias
`"replace"`. The alias exists for serde backward compat only.

#### Scenario: Unknown mode strings fail to deserialize

- **WHEN** a caller deserializes a string other than `"overwrite"`,
`"append"`, `"prepend"`, or `"replace"` (the legacy alias) into `EditMode`
- **THEN** deserialization SHALL fail with a serde error. Typo'd strings
like `"Overwrite"` (wrong case), `"REPLACE"`, or `"delete"` are rejected.

#### Scenario: API layer does not enforce MCP-side requiredness

- **WHEN** a downstream consumer (e.g., `nb-mcp-server`'s `EditArgs`)
declares `#[serde(default)] mode: EditMode` and serializes a payload without
the `mode` field
- **THEN** the consumer-side serializer applies the default. The
`nb-api` layer SHALL NOT reject the missing field; that is the consumer's
responsibility. Phase 5 implements the API-layer vocabulary fix; the MCP-layer
requiredness is downstream work.

### Requirement: NbClient methods SHALL use the Phase 6 public-API vocabulary

`NbClient` SHALL expose its notes/todos/organization methods under the
Phase 6 vocabulary (uniform `<action>_<resource>` shape across all
three subsystems), as listed below. The pre-Phase-6 method names
SHALL NOT exist on `NbClient`; this is a hard rename, not a
deprecation. Existing callers MUST update their call sites;
`nb-api 0.2.0` ships no backward-compat aliases (the API is not yet
widely adopted and the 0.2.0 bump is the natural migration
boundary).

#### Notes API

| Phase 6 method | Replaces | Notes |
|----------------|----------|-------|
| `add_note` | `add` | |
| `show_note` | `show` | |
| `edit_note` | `edit` | |
| `delete_note` | `delete` | |
| `move_note` | `move_note` | Unchanged (already suffixed). |
| `list_notes` | `list` | |
| `search_notes` | `search` | |

#### Todos API

| Phase 6 method | Replaces | Notes |
|----------------|----------|-------|
| `add_todo` | `todo` | |
| `mark_task_done` | `do_task` | Renamed to `mark_…` / `unmark_…` for symmetric, unambiguous state-transition names. |
| `unmark_task_done` | `undo_task` | See above. |
| `list_tasks` | `tasks` | Renamed so the method name matches the underlying `nb` CLI invocation (`nb tasks`, which lists checklist items within todos). The future `list_todos` (todo container listing, `nb todos`) is tracked separately as deferred work. |

#### Organization API

| Phase 6 method | Replaces |
|----------------|----------|
| `add_bookmark` | `bookmark` |
| `import_note` | `import` |
| `list_folders` | `folders` |
| `add_folder` | `mkdir` |
| `list_notebooks` | `notebooks` |
| `show_notebook_status` | `status` |
| `show_notebook_path` | `notebook_path` |

#### Scenario: Old method names do not exist on NbClient

- **WHEN** a caller attempts `NbClient::add(...)`, `NbClient::show(...)`, or any other pre-Phase-6 method name
- **THEN** the code SHALL fail to compile. `nb-api 0.2.0` ships no `#[deprecated]` aliases; the method names have been hard-renamed.

#### Scenario: New method names compile and behave end-to-end

- **WHEN** a caller invokes the Phase 6 names (e.g., `client.add_note(...)`, `client.list_notes(...)`, `client.mark_task_done(...)`)
- **THEN** the code SHALL compile and the methods SHALL execute the corresponding `nb` CLI subcommand with the same arguments and return semantics as the pre-Phase-6 methods.

#### Scenario: Internal call sites use new names

- **WHEN** the source tree is inspected
- **THEN** no `self.<old_name>(...)` or `<client_var>.<old_name>(...)` call SHALL remain. All internal call sites within `src/lib.rs` and `tests/integration/*.rs` SHALL use the Phase 6 names.

#### Scenario: README example uses new names

- **WHEN** the `README.md` example is inspected
- **THEN** the example code SHALL call `client.add_note(...)` and `client.search_notes(...)` (not the pre-Phase-6 names).
