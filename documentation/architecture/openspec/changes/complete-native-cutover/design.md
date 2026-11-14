## Context

All typed reads and all `Transaction` writes are already native (see reviews/41). What remains behind the `nb` subprocess boundary (`NbClient::exec`, `src/client/core.rs:404`):

- Notebook lifecycle: `ensure_notebook` / `show_notebook_path_unguarded` shell out to `nb notebooks show --path`, and creation shells out to `nb notebooks add`. Only the "directory already exists under `NB_DIR`" fast path avoids a spawn.
- `import_note`: `nb <scope>:import`, with a documented containment gap (`write.rs:647` — no `--wait`, cannot join the backgrounded checkpoint).
- Verbatim String surfaces: `list_notebooks`, `list_notes`, `list_folders`, `list_tasks`, `search_notes` String forms exist to mirror `nb` CLI text byte-for-byte.
- Test oracles that spawn real `nb` for differential assertions, plus the fixture (`testing/env.rs`) which runs `nb notebooks add`.

Operator direction is pre-1.0 no-compat: no shims, no phased flags. `git` subprocesses stay; only the `nb` dependency goes.

## Goals / Non-Goals

**Goals:**

- Zero `nb` process spawns in production code paths and the default test suite.
- Notebook create/ensure/path with the same failure semantics callers rely on (`NotFound` stays terminal on read paths; creation is explicit).
- Import with single-checkpoint semantics and race containment equal to `Transaction::commit`.
- Retire the String surfaces and the spawn machinery in the same change (no half-migrated tree).

**Non-Goals:**

- Removing the `git` dependency (out of scope; `git` remains a subprocess with env scrubbing).
- Changing typed API behavior or output shapes.
- Migrating `nb-mcp-server` (downstream; audited pre-release, not in this change).
- Reproducing `nb` CLI text output natively.

## Decisions

**D1 — Lifecycle against `NB_DIR` layout, not through `nb`.**
`$NB_DIR/<name>` is a git worktree with `.git` (the existing validity probe) and a resolvable `HEAD` (initial commit at create; required because `Transaction::commit` reads `HEAD`). `.index` files stay lazy as today. Concurrent creation is atomic single-winner with owned-only cleanup; pre-existing unrelated contents are never initialized or removed. Verified by native structural tests by default plus two-way interop in the extended profile (native-created opens in true `nb` and vice versa), which is the only detector for upstream layout drift.
*Alternative (keep ensure via `nb`): rejected — preserves the auto-checkpoint race the cutover exists to kill.*

**D2 — Import reuses the transaction pipeline, file-only v1.**
File bytes are read natively; valid UTF-8 text is stored byte-identical through the existing parser/mangling/index placement as a planned create op, and the change commits through `notebook_commit_all` under the process-shared gate. The public signature is path-typed with optional folder/filename and a `CommitOutcome` result. Non-UTF-8/binary sources, non-regular files, and `://` URL shapes are refused typed with zero network activity; the `convert` parameter is removed outright (never silently ignored). Validation order is notebook-independent checks first; creation is an explicit persistent effect that survives a refused import; another writer's commits are never disturbed. URL sources are explicitly out: tracked as `nb-api:todos/4` (fetch policy, HTTP client dependency, `ImportSource::Url`, failure cases). The import entry point stays source-agnostic so URL support lands additively.
*Alternative (shell `curl` + import bytes): rejected — trades one subprocess for another.*

**D3 — String surfaces are removed, not re-rendered.**
Reproducing `nb`'s exact human text natively couples the library to another program's display format with no consumer value over the typed APIs. Pre-1.0 removal is the clean cut; the migration notes (proposal capability `string-surface-retirement`) map each retired method to its typed replacement.
*Alternative (byte-identical native rendering): rejected — high-fidelity mimicry with zero contract value.*

**D4 — Demolition order: callers first, machinery last; harness stays.**
Lifecycle → import → String surfaces → test conversion → `exec`/`exec_vec`/`nb_program` removal from production paths. `NbError::ExecutableNotFound` is **retained**: the `nb` integration harness remains for checking native behavior against a true-`nb` oracle, and the harness needs the typed spawn-failure error. Test-only oracle tests move behind a dedicated Nextest profile (complete/extended suite); the default suite is purely native.
*Alternative (retire the variant with the machinery): rejected — the retained harness still produces exactly the failures the variant types.*

**D5 — Test selection is explicit per entry point.**
A Nextest profile name alone changes only nextest selection, not `cargo test` or hooks. True independent `nb` comparisons are classified into an explicit oracle set; pure-native safety tests sharing their modules stay unignored in default. The oracle set is `#[ignore]`-gated; the extended command invokes the dedicated `--profile oracle` (defined in `.config/nextest.toml`) with `--run-ignored` and a filter selecting exactly that set (never blanket historical ignored probes), fails on empty oracle selection, and fails visibly when `nb` is missing. Complete verification is the documented union of the default native command and the extended oracle command. The default suite runs against a logging dead-`nb` tripwire stub, proving zero spawn attempts while git stays alive.

## Risks / Trade-offs

- [Risk] `nb`'s on-disk layout drifts out from under the replicated create → Mitigation: structural round-trip tests (layout assertions, not subprocesses); document the exact layout version supported.
- [Risk] `nb-mcp-server` uses a retired String/import API → Mitigation: audit its call sites before release; pre-1.0 breakage is accepted but must be known, not discovered.
- [Risk] URL import demand arrives immediately after file-only v1 → Mitigation: keep the import entry point source-agnostic (`ImportSource::File` today) so URL support is additive.
- [Risk] Windows CI still red after cutover (non-`nb` causes) → Mitigation: the cutover removes the known flaky family; whatever remains gets a smaller, hermetic failure surface to diagnose.

## Migration Plan

Single change on `master`, no feature flags (operator no-compat direction). Steps: implement lifecycle + import natively, retire String surfaces, convert oracle/fixture tests, remove spawn machinery, run the full suite (both feature configs), clippy, fmt, specs validation, then review as a normal delegated change (Tier-1; new capability, not a fixup). Rollback is `git revert` of the single change; nothing is released until CI is green on both platforms.

## Open Questions

All three review questions resolved with the operator (2026-11-14):

1. URL imports: file-only v1; tracked as `nb-api:todos/4`.
2. `ExecutableNotFound`: retained — the `nb` integration harness stays for true-oracle checks and needs the typed spawn failure.
3. Layout tests: purely-native assertions in the default suite, plus a dedicated Nextest profile running the true-`nb` oracle as the complete/extended suite.
