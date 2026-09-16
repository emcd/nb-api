<!-- nbspec: change=update-api-0-4-0-omnibus notebook=nb-api note=proposals/update-api-0-4-0-omnibus/designs/design__0.4.0_omnibus.md hash=sha256:41692efaf858fc88165e7e18ab005bac2d9d6e76847c828bdfd6c4c7712b6ff8 -->
# Design: 0.4.0 omnibus — text-first, document EOL, and nb-faithful identity

#nbspec #design

## Goals / Non-Goals

### Goals

- Make `nb-api` text-first: `String` fields, no base64 in the library, typed `NonUtf8` boundary, single `Vec<u8>` escape hatch.
- Make body vs source computable via `BodyFragment {start_byte,end_byte}` offsets (no `body_includes_title` bool).
- Replace per-line `LineTerminator` with document-level `eol: Option<LineEol>` + `has_final_eol`, first-supported-occurrence detection, preserve bare `\r` faithfully, stable `LineAnchor` (`hash(text+eol)` / `hash(text+0x00)`), deterministic `Dollar`/`After` on `eol:None` → `Lf`.
- Make creates faithful to `nb` 7.24.0: ASCII-locale mangling (Unicode preserved), local-time titleless, collision `-N`, `.index` maintenance with blank-not-remove + `O_APPEND` + `.nb-api-index.lock` read-append + post-write selector, single checkpoint.
- Keep path canonical; expose numeric `<folder>/<id>` convenience; preserve 0.3.x opaque files readable.
- Ship `migration-0.4.0.md` and remove `migration-0.3.0.md` (`todos/api/7`).

### Non-Goals

- Full native read cutover (`todos/2` + `ideas/1` → 0.5.0).
- `README` subprocess-wrapper wording (`todos/api/8` → 0.5.0).
- `list_todos` (`todos/api/5` → 0.5.0).
- New format parsers (`todos/format/{1,2,3}` → P3+).

## Architecture

```
Types (src/types.rs)
  ByteString removed → String fields
  ShowNote { title/title_text/body/source, body_fragments:Vec<BodyFragment{index,bytes,start_byte,end_byte}>, fingerprint, body_contiguous=derivable }
  ShowNoteLines { eol: Option<LineEol>, has_final_eol, lines: Vec<NoteLine{text,String}> }
  LineEol { Lf, CrLf }  NoteLine { number, anchor, text }  (no terminator, ignore unknown fields)
  LineAnchor = b3l1:<32 hex> over exact byte span (text + eol bytes | 0x00 final-none)
  LineEdit { Insert{at,content:String}, Delete, Replace{content:String} }

Errors (src/error.rs)
  NonUtf8 { selector, path, kind, mime_hint }  FragmentedBody {fragment_count,guidance}  IndexLockTimeout

Reads (src/client.rs)
  show_note / show_note_lines → probe text vs binary → UTF-8 validate → NonUtf8 or String + fragment offsets
  read_note_source_bytes(NoteTarget) -> Vec<u8>  (bypass validation)

Writes (src/transaction.rs)
  add_note(title) → ASCII-locale mangle (Unicode preserved) → collision → plan
  commit → O_APPEND + .nb-api-index.lock read-append → post-write re-read for selector → stage .index+note → commit
  delete: blank line, never remove; move: blank source + append dest; ids never reused
  .nb-api-index.lock wait/retry with timeout (todos/api/6) → IndexLockTimeout; lock scope nb-api only, cross-process guarantee via O_APPEND + post-write

Splitter (src/lines.rs)
  first-supported-EOL detection → split on that EOL only; bare \r preserved verbatim
  Edits on eol:None adopt Lf; Dollar/empty/Replace normalize to trailing EOL
```

## Decisions

### D1 — Remove, don't deprecate, ByteString (pre-1.0 hard break)

Operator 2026-08-20: 0.4.0 removes `ByteString` entirely (no `#[deprecated]` shim). Base64 is wire-layer only (`nb-mcp-server`). `ByteString` only guarded non-UTF-8-but-text; non-textual already gated by `UnsupportedShowTarget`.

### D2 — Text-only + NonUtf8 + escape hatch

Structured surface is text-only. Non-UTF-8 → `NonUtf8`. One `read_note_source_bytes` covers narrow case. Keeps library base64-free.

### D3 — Body vs source via offsets, not bool [rev advisor 2026-09-02]

Dropped `body_includes_title: bool` (always false, docs-as-data). Instead `BodyFragment { start_byte,end_byte }` offsets into `source` (vocab already in `NoteLineHit`). Then `body = concat(fragments.bytes)`, `body_contiguous = fragments.len()<=1` derivable, and `source[start..end]` relation is computable. Fixes `display --full` trap without constant field.

### D4 — Document-level EOL, first-supported-occurrence, preserve bare CR [rev 2026-09-01]

`eol: Option<LineEol>` + `has_final_eol` replaces per-line terminator. First **supported** occurrence (`\r\n`/`\n`) wins; bare `\r` is not a terminator, preserved verbatim (`probe_empirical.rs:273` CR-only remains `"\r"`). `eol: None` for CR-only/empty. No normalization CR→LF on read or write. Edits on `eol:None` adopt `Lf` (e.g. `After` on CR-only `a\rb\rc` → `a\rb\rc\nx\n`).

### D5 — Anchor exact span, 0x00 final-none, Dollar changes previous anchor [rev advisor 2026-09-02]

Anchor = `hash(text + eol_bytes)` or `hash(text + 0x00)` for final without EOL. Phrase as "exact byte span" — EOL flip corollary. `Dollar` when `has_final_eol==false` changes previous final line's anchor (`b\x00` → `b\n`), correct conflict if editor holds old anchor.

### D6 — Bare-text LineEdit, deterministic trailing EOL [rev 2026-09-01 P1 + advisor 4]

`LineEdit.content: String` bare text (may contain `\r` preserved); library appends `eol`. Deterministic: `Insert Dollar` on `has_final_eol==false` → trailing `eol` (or `\n` if `None`); empty `Caret "x"` → `x\n`; `After` on CR-only adopts `Lf`.

### D7 — Title-mangling ASCII-locale, local time, stable ids [rev advisor probe 2026-09-02]

Mangle: ASCII `A-Z` lowercased, non-ASCII preserved (`Café Über` → `café_Über.md` per `probe_advisor`), `[^A-Za-z0-9]` → `_`, collapse, trim. Titleless `%Y%m%d%H%M%S` is **local time** (`date` not `date -u`, probe `20260901215114` local). Collision `-1`,`-2`. Delete = blank line never remove (probe `note_a,"",note_c`; `C` keeps id 3, new `D` → 4). Move within = in-place update; across = blank source + append dest (both `.index` committed atomically). Ids never reused.

### D8 — .nb-api-index.lock + O_APPEND + post-write selector [rev reviewer P1 2026-09-02 + advisor tightening; platform scope amended round 4]

Holding `\.git/index.lock` across `git add`/`commit` is impossible (those commands acquire it and would fail) and does not serialize external `nb` pre-commit writes. Instead: `O_APPEND` so no appended line is lost; `.nb-api-index.lock` held **only** for `.index` read-append (not across git staging/commit) — serializes **nb-api writers only**, not external `nb`; cross-process guarantee comes from `O_APPEND` + post-write re-read (search from end for basename, unique per Collision rule) to derive `OpOutcome.selector`/`numeric_id`, never pre-computed `lines+1`. Staged `.index` may include external not-yet-committed line (harmless). `IndexLockTimeout` on timeout. Lock file carries a nonce (conditional removal) plus a holder heartbeat, so live holders are never reaped.

Platform scope (round 4): blank and same-folder rename-to-shorter-or-equal remove the stale range with an atomic single-syscall primitive where available (Linux `FALLOC_FL_COLLAPSE_RANGE`, verified per file; portable truncate fallback elsewhere with a documented residual micro-window and `IndexLockTimeout` refusal under sustained contention). Same-folder growing renames are refused at plan time. Concurrent external rewrites remain out of scope (needs `nb` cooperation).

### D9 — Path canonical, id convenience, forward-compat

`NoteTarget::Path` / `OpOutcome.path` canonical; `selector`/`numeric_id` convenience via `.index` scan. Unknown JSON fields (e.g. old `terminator`) ignored (no `deny_unknown_fields`); removal tested via absence in serialization. `FragmentedBody` carried from `body-aware-editing` (not new shape).

## Implementation sequence

1. Types + errors + schemars (`NonUtf8`, `FragmentedBody`, `IndexLockTimeout`), remove `ByteString`, add `BodyFragment` offsets.
2. `lines.rs` first-supported-occurrence + preserve `\r` + `eol:None`→`Lf` on edit + anchor exact span.
3. `client.rs` reads → `String` + offsets + `read_note_source_bytes`; `ShowNoteLines` wire; `LineEdit` + `Dollar`/`After` tests.
4. `transaction.rs` ASCII-locale mangle + local-time titleless + collision + `.index` blank-not-remove + `O_APPEND` + `.nb-api-index.lock` read-append + post-write re-read + both-indexes for move; `OpOutcome` path+selector.
5. `migration-0.4.0.md`, remove `migration-0.3.0.md`, bump `Cargo.toml` to `0.4.0`, `cargo test` + `nbspec validate` + `nbspec render` + concurrent `Transaction` test.

## Alternatives considered

- `body_includes_title: bool` → rejected advisor: constant field; offsets computable.
- Normalize bare `\r` → `\n` → rejected operator 2026-09-01: preserve CR faithful.
- Reuse blank id on next create → rejected probe: `nb` appends monotonic, never reuses.
- Remove `.index` line on delete → rejected probe: would renumber subsequent ids.
- Hold `\.git/index.lock` through `git add`/`commit` → rejected reviewer P1: impossible and still races external `nb`; replaced with `O_APPEND` + `.nb-api-index.lock` read-append + post-write.

## References

- `coordination/general/5`, `src/types.rs:12-33`, `src/lines.rs:26-78`, `src/client.rs:488,709-745`, `src/transaction.rs:1750`, `tests/integration/probe_advisor.rs`, `tests/integration/probe_move.rs`, `tests/integration/probe_empirical.rs:273`, `nb-api:artifacts/2`, `issues/api/3`
- Reviews `nb-api:proposals/update-api-0-4-0-omnibus/verdicts/20260902041122-c2561-0877af-0.md` (APPROVE) + amendments 2026-09-01/02
