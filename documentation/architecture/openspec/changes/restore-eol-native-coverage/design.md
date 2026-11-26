## Context

S1 lifecycle is done and pushed (native create/ensure/path on `NB_DIR`); the `eol_model` tests exercise purely native behavior (line-EOL parsing, body edits through `Transaction`) but still pay the live-`nb` setup cost via `NbTestEnv::new()`. Quasar's audit (`nb-api:issues/13`) splits by what each test proves: native-behavior tests go hermetic with postcondition asserts, pilot one, then restore by dependency inventory. The pilot (`insert_appends_document_crlf` on `diag/quasar-pilot-eol-native`) added a local `assert_notebook_settled` (HEAD peels to a commit, `status --porcelain` clean, plain `git`, no client) plus `git_capture`, and went green on Windows twice. Master still holds all 10 tests nb-backed. This change replays the pilot shape across the whole EOL file as its own test-only batch, ahead of cutover S2 per operator ordering.

## Goals / Non-Goals

**Goals:**

- All 10 `eol_model` tests run on `NbTestEnv::hermetic()` with zero `nb` spawns in setup or assertions.
- Each test keeps its exact EOL assertions and adds settled-repo postconditions (HEAD resolvable, tree clean) at the same points the pilot used (after setup write, after mutation).
- Default Linux suite, clippy (both configs), and fmt stay green; Windows evidence is named-test execution for the file.

**Non-Goals:**

- No production code changes; no fixture API changes (reuse `hermetic()` / `build_hermetic()` as-is).
- No oracle, coexistence, or mixed-family conversions — those stay Unix-gated; later batches are separate.
- No `#[cfg(unix)]` removals outside `eol_model.rs` (the file is currently ungated; no gating change needed).

## Decisions

**D1 — Replay the pilot verbatim, not a new harness.**
Reuse `NbTestEnv::hermetic()` plus the same two helpers (`assert_notebook_settled`, `git_capture`). The pilot's Windows greens are the pattern's qualification; inventing a second shape would void that evidence.
*Alternative (new shared settled-assert API in `src/testing`): rejected — production-surface churn for a test-only batch; revisit if later batches need it.*

**D2 — Helper lives in `eol_model.rs` for this batch.**
Keep the two helpers file-local as the pilot did. Promoting to `common/mod.rs` is a one-line move later if the next batch wants it; duplicating the decision across files now spreads review surface.
*Alternative (promote to `common` immediately): rejected — premature sharing before the second consumer exists.*

**D3 — EOL batch is all 10, single commit scope.**
EOL is the coherent native-only family Quasar named a good first batch. Converting fewer than 10 leaves a mixed file (two fixture shapes, weaker signal); converting more (other families) mixes dependency inventories and breaks per-batch Windows evidence.
*Alternative (convert 9, leave the pilot test to a later cherry-pick): rejected — master never received the pilot, so all 10 are nb-backed there; the batch is 10 on master.*

**D4 — Postconditions assert, never settle.**
`assert_notebook_settled` runs plain `git rev-parse --verify HEAD^{commit}` and `git status --porcelain` with `scrub_git_env_std`, asserting non-empty HEAD and empty status. No loops, no sleeps, no retries — a dirty tree fails loudly instead of settling.
*Alternative (reuse the fixture's settle loop): rejected — that loop exists to absorb `nb`'s background checkpoint, which hermetic setup never creates.*

## Risks / Trade-offs

- [Risk] A test actually depends on `nb`-created layout (not just git layout) and fails hermetic → Mitigation: the failure is signal, not noise — file it, keep the test nb-backed, and note the inventory exception in the change; EOL logic is not expected to hit this.
- [Risk] Windows greens on diag do not transfer to master tip → Mitigation: per-batch Windows evidence is required (named tests on the exact tip), not assumed from the pilot.
- [Risk] Helper drift if later batches copy-paste variants → Mitigation: note the promotion trigger (second consumer) in tasks; no action now.

## Migration Plan

Test-only change on a working branch off `master`, single commit (plus Tier-1 fixups held unsquashed per review procedure). No flags, no rollback beyond `git revert`. Validation: full Linux suite with `--all-features`, clippy both configs, fmt, `openspec validate`, then named Windows execution for the file before Tier-1 review.

## Open Questions

None. The pilot answered feasibility; causation for the index.lock family was settled separately by the handshake SHA audit (pre-fix SHAs red, post-fix SHAs green). Remaining Windows reds are the pre-existing oracle/checkpoint families with first-red logs preserved.
