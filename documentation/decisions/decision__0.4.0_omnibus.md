<!-- nbspec: change=update-api-0-4-0-omnibus notebook=nb-api note=proposals/update-api-0-4-0-omnibus/decisions/decision__0.4.0_omnibus.md hash=sha256:f75649100fa04eab3b32ae16acf7f997dba925db3ab43a60fd11a1d6065c68d3 -->
# Decision: 0.4.0 omnibus — break vs shim, EOL, and identity

#nbspec #decision

## Status

Proposed — captures operator + NbMcp + Nbspec + Advisor threads from `coordination/general/5` and probes `probe_advisor.rs`/`probe_move.rs` (nb 7.24.0) before implementation. Rev 2026-09-02 incorporates advisor tightening and reviewer P1/P2.

## Context

0.3.x `ByteString` base64 leaked wire concerns into the library; per-line `terminator` wasted LLM tokens and mishandled bare `\r`; `Transaction` filenames and `.index` drifted from `nb` 7.24.0. Full native cutover is large and benefits from landing on the final text-first surface.

## Decision

### 1. ByteString: hard remove, not deprecate

Operator 2026-08-20: 0.4.0 is a breaking pre-1.0 release, `ByteString` removed entirely, no `#[deprecated]` shim. `NbMcp Owner` agreed; base64 moves to wire layer (`nb-mcp-server`). `FragmentedBody` is existing shape, not new.

### 2. Text-first + NonUtf8 + raw-bytes hatch

Structured surface becomes `String`; non-UTF-8-but-text returns `NonUtf8 { selector, path, kind, mime_hint }`; non-textual stays `UnsupportedShowTarget`. One `read_note_source_bytes -> Vec<u8>` escape hatch.

### 3. Body vs source via offsets

`ShowNote.body` = concatenated body fragments (fingerprint domain), `source` = full file. Dropped `body_includes_title: bool` (always false, docs-as-data); instead `BodyFragment { start_byte,end_byte }` offsets into `source` (vocab `NoteLineHit`), making `body` computable and `body_contiguous` derivable. Fixes `display --full` trap.

### 4. Document-level EOL

`ShowNoteLines { eol: Option<LineEol(Lf|CrLf)>, has_final_eol }` replaces per-line terminator. First **supported** occurrence (`\r\n`/`\n`) wins; bare `\r` is not terminator — preserved verbatim (faithful `probe_empirical.rs:273` CR-only round-trip). `eol: None` for empty/CR-only. Anchor = exact byte span `hash(text+eol)` / `hash(text+0x00)` final-none; `Dollar` when `has_final_eol==false` changes previous anchor. Edits on `eol:None` adopt `Lf` (e.g. `After` on `a\rb\rc` → `a\rb\rc\nx\n`); `LineEdit.content: String` bare text; unknown JSON fields ignored (no `deny_unknown_fields`).

### 5. Identity: mangling + .index with stable ids and explicit lock

Title-mangle: ASCII `A-Z` lowercased, non-ASCII preserved (`Café Über` → `café_Über.md` per `probe_advisor.rs`), `[^A-Za-z0-9]`→`_`, collapse, trim; collision `-1`; titleless `%Y%m%d%H%M%S.md` in **local time** (process `TZ`, not UTC) per probe. `Transaction` owns `.index` with blank-not-remove (probe `A:1,B:2,C:3`→delete 2 blanks line, `C` keeps 3, `D`→4; moves `probe_move.rs` in-place vs blank+append across folders). Ids never reused. `.index` appends use `O_APPEND` + notebook-scoped file lock `.nb-api-index.lock` held only for read-append (not across `git add`/`commit` — holding `.git/index.lock` would fail), wait/retry → `IndexLockTimeout`; `OpOutcome` derived **post-write** (re-read after append) so external `nb add` interleaving yields correct selector; staged `.index` may include external line (harmless). `numeric_id: Option<u32>` pinned type.

### 6. Defer native cutover to 0.5.0

`todos/2` + `ideas/1` native read rewrite deferred. 0.4.0 fixes the wire types it will implement against. `todos/api/8` and `todos/api/5` defer with it; `todos/api/6` closes in 0.4.0 with concurrent test (spawn two `Transaction` commits); `todos/api/7` (remove `migration-0.3.0.md`) ships here.

## Consequences

- Breaking 0.4.0: recompilation + wire JSON break (ByteString→String, BodyFragment offsets, LineTerminator removal, `NonUtf8`/`IndexLockTimeout` ( + existing `FragmentedBody`)); token savings; faithful filenames (ASCII-locale, local time) and stable ids with correct post-write selectors; unblocks 0.5.0 native rewrite.
- `nbspec validate` passes before land; `migration-0.4.0.md` required.

## References

- `coordination/general/5` Threads A/B, `src/types.rs:12-33`, `src/lines.rs:26-78`, `tests/integration/probe_identity.rs`, `probe_advisor.rs`, `probe_move.rs`, `probe_empirical.rs:273`, `nb-api:artifacts/2`, `issues/api/3`
- Agentmux `nb-mcp@notebook` 2026-08-20, `nbspec@notebook` 2026-08-18, `advisor@infrastructure` 2026-09-02
