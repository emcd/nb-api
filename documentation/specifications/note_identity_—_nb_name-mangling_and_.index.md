<!-- nbspec: change=update-api-0-4-0-omnibus notebook=nb-api note=proposals/update-api-0-4-0-omnibus/specifications/note_identity_—_nb_name-mangling_and_.index.md hash=sha256:7056107228cfeb52a6cf3a88af0eedacafe7bc74efcec74a9c77e30287300205 -->
# Note identity — nb name-mangling and proper .index

#nbspec #specification

## Purpose

Make `nb-api` creates faithful to `nb` 7.24.0 filenames and `.index` identity, while keeping path as canonical API identity and exposing numeric ids as resolved convenience for `<folder>/<id>` and LLM expectations.

## ADDED Requirements

### Requirement: Title-mangling SHALL match nb ASCII-locale rule

When `Transaction` creates a note with a title, the filename SHALL be the `nb` slug: ASCII `A-Z` lowercased, non-ASCII bytes preserved verbatim (no Unicode transliteration), ASCII non-alphanumeric `[^A-Za-z0-9]` → `_`, collapsed consecutive `_`, trimmed leading/trailing `_`. Probe `Café Über` → `café_Über.md` (é preserved, Ü preserved, space → `_`) validates Unicode preservation. Extension preserves kind (`.md`, `.todo.md`, `.bookmark.md`). Titleless creates use `%Y%m%d%H%M%S.md` in **local time** (process TZ, matches `nb` `date`, not UTC) per probe `20260901215114` matching `date` not `date -u`. Validated against `tests/integration/probe_identity.rs` and `probe_advisor.rs` (nb 7.24.0).

#### Scenario: Mangled title filename

- **WHEN** `add_note` with title `"Hello World Title"` in folder `""`
- **THEN** the created path SHALL be `hello_world_title.md` (or `hello_world_title-1.md` on collision)

#### Scenario: Non-ASCII preserved

- **WHEN** `add_note` with title `"Café Über"` in folder `""`
- **THEN** the created path SHALL be `café_Über.md` (Unicode preserved, space → `_`, ASCII lowercased)

#### Scenario: Titleless local time

- **WHEN** `add_note` with no title at local time `20260901215114` (process TZ)
- **THEN** the created path SHALL be `20260901215114.md` matching local `date`, not UTC

### Requirement: Collision suffix SHALL be -N

If the mangled filename (or timestamp name) already exists in the target folder at commit time, the filename SHALL gain suffix `-1`, `-2`, … before the extension until free. This guarantees basename uniqueness within a folder, which makes post-write lookup by basename unambiguous.

#### Scenario: Collision gets -1

- **WHEN** `hello_world_title.md` exists and `add_note` with title `"Hello World Title"` is committed
- **THEN** the created path SHALL be `hello_world_title-1.md`

### Requirement: Transaction SHALL maintain .index with stable positional ids

On each create, `Transaction::commit` SHALL append the created filename (basename only) as a new line to `.index` in the target notebook/folder (creating `.index` if absent). `.index` and the note file SHALL be committed in a single git checkpoint.

- **Delete:** the line for the deleted note SHALL be **replaced with an empty line** (blank), **never removed**. This keeps all subsequent ids stable (probe: create A:1,B:2,C:3 → delete 2 → `.index` = `note_a.md`,``, `note_c.md`; `show 3` still resolves to `note_c.md`, `show 2` → `NotFound`, new `add D` → id 4 not reuse 2).
- **Move within folder (rename):** update the same `.index` line in-place to the new basename (probe `probe_move.rs`: `foo/foonote.md` → `renamed.md` at foo/1). Same-folder renames to a LONGER name are refused at plan validation (the in-place update cannot splice losslessly — delete + recreate instead).
- **Move across folders:** replace source `.index` line with blank (as delete) and append to destination `.index` (as create, with its own monotonic id). Both `.index` files and the moved file are committed atomically (probe: `movea.md` 1 → `foo/` blanks root line 1, appends `movea.md` at foo/2; `foo/1` → `bar/1` blanks foo line 1, appends `renamed.md` at bar/1).
- Ids are **never reused**; blank lines remain forever (nb never compacts `.index`).

#### Scenario: Delete blanks the line and ids stay stable

- **WHEN** `.index` is `note_a.md\nnote_b.md\nnote_c.md\n` (ids 1,2,3) and `delete_note` targets id 2
- **THEN** after commit `.index` SHALL be `note_a.md\n\nnote_c.md\n`
- **AND** `show_note` id 3 SHALL still resolve to `note_c.md`
- **AND** `show_note` id 2 SHALL fail `NotFound`

#### Scenario: Create after delete does not reuse id

- **WHEN** `.index` has a blank at line 2 and `add_note` creates `note_d.md`
- **THEN** `.index` line 4 SHALL be `note_d.md` (id 4), not line 2

#### Scenario: Move across folders updates both indexes

- **WHEN** `move_note` moves `foo/note.md` (id foo/1) to `bar/`
- **THEN** source `foo/.index` line 1 SHALL become blank and `bar/.index` SHALL append `note.md` with new id

#### Scenario: Growing in-folder rename refused

- **WHEN** `move_note` renames `foo/a.md` to `foo/a_much_longer_name.md` in the same folder
- **THEN** commit SHALL fail plan validation and neither the file nor `.index` SHALL change

### Requirement: Cross-process .index append SHALL use append-mode and post-write selector with explicit .index lock

Writers (this `Transaction` + external `nb add`) both compute `lines+1` and race → misnumbered selector if pre-computed. Holding `\.git/index.lock` across `git add`/`commit` is impossible (those commands acquire it and would fail). Instead:

1. `.index` writes SHALL use OS append-mode (`O_APPEND`) so concurrent appends never clobber; no appended line is lost in append-vs-append races on any platform.
2. A notebook-scoped file lock `\.nb-api-index.lock` (distinct from git's `\.git/index.lock`) SHALL be held **only** for the `.index` read-append critical section, not across git staging/commit. The lock serializes **nb-api writers against each other only**; it does **not** serialize external `nb` processes. Wait/retry with timeout; on timeout return `NbError::IndexLockTimeout` (no mutation). A live holder refreshes the lock so it never looks stale; removal requires stable stale content plus a matching nonce.
3. The cross-process guarantee against external `nb` comes from (1) `O_APPEND` (no clobber) + (3) post-write position (correct selector), not from the lock — this shall be stated explicitly so future readers do not believe the lock covers `nb add`.
4. `OpOutcome.selector` and `numeric_id` SHALL be derived from the **post-write** position of the basename in `.index` (re-read after append and search **from the end**, finding the last occurrence of that basename; uniqueness is guaranteed by the Collision-suffix requirement above), never from the pre-computed `lines+1`. Then interleaving yields a correct selector even if an external writer appended between re-read and append.
5. The subsequent `git add .index` + `git commit` stages `.index` as it exists on disk at that moment, which may already include an external writer's not-yet-committed line — harmless; their later commit will find nothing to add for `.index` and succeed. This shall be documented as expected, not corruption.
6. Platform scope for blank/rename vs concurrent external appends (amended round 4: absolute losslessness against appends is unimplementable portably — any truncate-based rewrite retains a read-to-write window, and only a single-syscall range removal closes it):
   - Blank and same-folder rename-to-shorter-or-equal SHALL remove the stale range with an atomic single-syscall primitive where the platform offers one (Linux `FALLOC_FL_COLLAPSE_RANGE`, verified per file at runtime); concurrent appends there land wholly before (shifted left, preserved) or wholly after (at the new end, preserved), and any size movement retries with their bytes present.
   - Where the platform lacks atomic range removal, the implementation SHALL narrow the race to the single truncate boundary, retry on any observed growth, and refuse with `IndexLockTimeout` under sustained contention instead of risking silent loss. This residual micro-window (one racing append landing exactly across the truncate syscall) is documented, not covered by the no-loss guarantee.
   - Concurrent external REWRITES (another blank/rename) remain out of scope with or without atomic removal: serializing them requires `nb` cooperation, same as nb-vs-nb.

For append-vs-append races, each writer SHALL receive the actual line position of its basename; no line is lost; each `OpOutcome` selector is correct. A concurrent integration test (`tests/integration/probe_advisor.rs` delete+create, `probe_move.rs` moves) SHALL exercise the path and the new concurrent test SHALL spawn two `Transaction` commits concurrently.

#### Scenario: Concurrent appends each get correct post-write selector

- **WHEN** two processes append `a.md` and `b.md` concurrently to `.index` at 3 lines
- **THEN** `.index` SHALL contain both lines (4 and 5 in some order) and each writer's `OpOutcome.selector` SHALL equal the actual line number of its basename after commit (re-read from end), not the pre-computed 4

#### Scenario: External nb append interleaves

- **WHEN** external `nb add` appends `ext.md` between this `Transaction`'s re-read and append
- **THEN** both lines SHALL appear and this `Transaction`'s `OpOutcome` SHALL reflect its post-write position (e.g. 6), not the stale pre-compute

### Requirement: Path SHALL remain canonical identity

The public API SHALL keep `NoteTarget::Path` / `OpOutcome.path` as canonical identity. Numeric ids are resolved convenience: `ShowNote` and `ShowNoteLines` SHALL expose `selector: String` as `"<folder>/<id>"` when resolvable via `.index`, and `numeric_id: Option<u32>` (pinned type, not "or equivalent"). `OpOutcome` after commit SHALL echo both `path` and `selector` when known. Selector stability depends on the blank-not-remove rule above. Local time is process `TZ` (matches `nb` `date`).

#### Scenario: OpOutcome echoes path and selector

- **WHEN** `add_note` creates `hello_world_title.md` as `.index` line 3 in folder `work`
- **THEN** `OpOutcome { path: Some("work/hello_world_title.md"), selector: Some("work/3"), numeric_id: Some(3) }` SHALL be returned

#### Scenario: Deleted id selector is NotFound

- **WHEN** `.index` line 2 is blank after delete
- **THEN** `show_note` with `NoteTarget::Selector("2")` SHALL fail `NotFound` and not resolve to the next note

### Requirement: Direct-write adoption SHALL remain compatible

Existing 0.3.x opaque filenames (`{epoch}-{seq}.md`) SHALL remain readable if present on disk / in `.index`; `nb` self-heals `.index` on next `nb list` if an entry is missing, but 0.4.0 creates use mangled names going forward.

#### Scenario: Old opaque file still shown

- **WHEN** a note `1787096169-0.md` exists on disk and in `.index`
- **THEN** `show_note` by path or by numeric id SHALL succeed
