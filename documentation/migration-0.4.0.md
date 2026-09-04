# Migration: nb-api 0.3.x → 0.4.0

0.4.0 is a breaking pre-1.0 release. `ByteString` is removed entirely (no
deprecated shim); all structured text is `String`, line endings are declared
once per document, and creates use nb-faithful filenames with stable numeric
ids.

## `ByteString` → `String`

Every field that was `{ "base64": "..." }` is now a JSON string:

| 0.3.x field | 0.4.0 field |
|---|---|
| `ShowNote.title: Option<ByteString>` | `Option<String>` (raw title line incl. trailing newline) |
| `ShowNote.body / source: ByteString` | `String` (`body` = concatenated body fragments, `source` = full file) |
| `BodyFragment.bytes: ByteString` | `String`, plus `start_byte / end_byte` offsets into `source` |
| `NoteLine.text: ByteString` | `String` (line without terminator) |
| `NoteLineHit.text: Option<ByteString>` | `Option<String>` |
| `LineEdit` Insert/Replace `content: ByteString` | `String` (bare text, no terminator; embedded `\r` preserved) |
| Substring `pattern / replacement` | `String` |
| `replace_note_body(new_body)`, `retitle_note(title)`, `search_note_lines(pattern)` | `&str` |

Deserializing the old `{ "base64": ... }` object form now fails. The crate
has no base64 dependency; JSON-transport base64 (if needed) belongs in the
wire layer (`nb-mcp-server`), not in `nb-api`.

`body_contiguous` is still present but derivable (`body_fragments.len() <= 1`);
there is no `body_includes_title` flag — `body = concat(fragment.bytes)` and
each `source[start_byte..end_byte]` is the fragment text.

## Text-only surface + `NonUtf8`

Structured reads/writes validate UTF-8 and fail with typed
`NonUtf8 { selector, path, kind, mime_hint }` on non-UTF-8-but-text files.
Non-textual targets keep returning `UnsupportedShowTarget`. The single
raw-bytes escape hatch is
`NbClient::read_note_source_bytes(NoteTarget) -> Vec<u8>`.

## Per-line terminator → document `eol`

`LineTerminator` (`Lf | Crlf | Cr | None`) and `NoteLine.terminator` are gone.
`ShowNoteLines` carries `eol: Option<LineEol>` (`Lf | CrLf`, JSON
`"lf" / "crlf"`) plus `has_final_eol: bool`:

- `eol` is the first supported EOL occurrence scanning left-to-right; bare
  `\r` is stray content, preserved verbatim, never a terminator.
- `eol: None` means empty, single-line-no-EOL, or CR-only.
- `LineAnchor` hashes the exact byte span (`text` + eol bytes, or `text` +
  `0x00` for a final line without EOL), so flipping `eol` changes anchors.
- `LineEdit.content` is bare text; the library appends document `eol`.
  Edits that materialize a boundary on an `eol: None` document adopt `Lf`
  (e.g. insert after a CR-only body yields `a\rb\rc\nx\n`).
- `Insert` at `Dollar` (or `After` the final line) when the final line lacks
  a terminator first terminates it, then appends `content + eol`.
- `Insert` at `Dollar` on an empty body is rejected (`Caret` only).

Unknown JSON fields on `NoteLine` (including the old `terminator`) are
ignored on deserialization.

## Identity: mangled names, `.index`, numeric ids

- One-shot creates mangle titles like `nb` 7.24.0 (ASCII lowercased,
  non-ASCII preserved, runs of other ASCII collapsed to `_`):
  `Hello World Title` → `hello_world_title.md`. Collisions gain `-1`, `-2`,
  … via retry. Titleless creates use local-time `%Y%m%d%H%M%S.md`. The opaque
  `{epoch}-{seq}.md` scheme is removed.
- `Transaction` maintains folder `.index` files: creates append, deletes
  replace the line with blank (never remove), within-folder moves update in
  place, cross-folder moves blank the source and append the destination. Ids
  are positional line numbers, never reused. Append-vs-append races lose
  nothing on any platform (`O_APPEND` + post-write positions). Blank and
  same-folder rename-to-shorter-or-equal remove the stale range with an
  atomic single-syscall primitive where the platform offers one (Linux
  `FALLOC_FL_COLLAPSE_RANGE`, verified per file at runtime): no concurrently
  appended line is lost there. Where the platform lacks atomic range
  removal, the race is narrowed to the single truncate boundary with retry
  and `IndexLockTimeout` refusal under sustained contention — a documented
  residual micro-window, not covered by the no-loss guarantee. Concurrent
  external rewrites are out of scope (needs `nb` cooperation, same as
  nb-vs-nb). Materialize touches only plan-dirtied paths, so external files
  are never deleted, and a same-path external create fails loudly instead
  of being overwritten.
- Same-folder renames to a LONGER name are refused at plan validation (the
  in-place update cannot splice losslessly): delete and recreate the note
  (new id) instead.
- `OpOutcome` echoes `path` plus numeric `selector` (`<notebook>:<id>` or
  `<notebook>:<folder>/<id>`) and pinned `numeric_id: Option<u32>`.
  `ShowNote` / `ShowNoteLines` expose the same numeric selector + id when the
  path resolves via `.index`.
- `NoteTarget::Selector` accepts `<folder>/<id>` / `<id>` (blank lines are
  `NotFound`). Explicit-path `Transaction` plans still get hard
  `PathCollision` errors; only auto-named one-shots retry with `-N`.
- New error: `IndexLockTimeout { path, timeout_ms }` when the `.index`
  read-append lock cannot be acquired.
- Old opaque filenames remain readable (path and numeric id) when present.

## Consumers

- **nb-mcp-server**: move base64 to the wire layer; switch line edits to bare
  `String` content with document `eol`; surface numeric selectors/ids.
- **nbspec**: no `body_includes_title`; fragment offsets replace it.
