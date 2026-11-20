## Context

`resolve_with(mode, strict, ...)` in `src/nbrc/mod.rs` branches on `strict` before the mode match: strict forces `IgnoreNbrc` semantics plus refusal when `NB_DIR` is unset. The flag threads from `Config.require_explicit_nb_dir` through `NbClient` into `notebook_root_dir(mode, strict)`. Operator decision (2026-11-20): drop the refusal capability entirely rather than folding it into the enum. Constraints: pre-1.0, no compat shims; the in-flight `complete-native-cutover` change is unaffected (verified: no strict references).

## Goals / Non-Goals

**Goals:**

- One control (`ResolutionMode`) governs store resolution; after mode-specific resolution, a still-unselected store falls back to `~/.nb` when home is computable.
- Remove the public `Config.require_explicit_nb_dir` field and every `strict` parameter.

**Non-Goals:**

- No replacement refusal mechanism (explicitly declined by the operator).
- No change to how each mode resolves a configured or rc-supplied store, how rc failures surface, or how uncomputable fallbacks and non-directory selections fail. Only the unset-with-nothing-selected outcome changes.
- No spec changes beyond the added `nbrc-resolution` contract.

## Decisions

- **Delete the strict arm, keep the Ignore arm as-is.** The strict arm was `Ignore` + refusal; deleting it leaves the existing `IgnoreNbrc` arm (preset-or-default) as the only unset-handling. No merge of behaviors, no new branches.
- **Remove parameters, don't deprecate.** Pre-1.0 policy forbids compat shims; the field default was `false`, so default-constructed callers are behaviorally unaffected. Any use of the field breaks at compile time with a clear error — even a `false` literal, since the field no longer exists.
- **Update the three integration users in place** (`show_probe.rs`, `native_nbrc.rs`, `native_reads_guard.rs`): remove the field literal entirely (even `false` breaks once the field is gone) and repurpose strict-only assertions into default-fallback assertions.
- **Supersede, don't duplicate, the cutover matrix.** The in-flight `complete-native-cutover` lifecycle spec said unset `NB_DIR` fails fast; amended at proposal stage (reviews/95) so the repo never holds two contradictory specs regardless of landing order. The nbspec-repo proposal needs its own follow-up (coordination task 3.2).

## Risks / Trade-offs

- [Risk] A downstream caller relied on strict refusal to catch misconfiguration, and now silently lands in `~/.nb`. → Mitigation: **BREAKING** is flagged in the proposal; the compile break forces each such caller to confront the removal and add its own explicit-unset check if it still wants refusal.
- [Risk] Leftover `strict` mentions in comments/docs drift from the code. → Mitigation: tasks include a repo-wide sweep of `strict`/`require_explicit` mentions in resolution-adjacent code and docs.

## Migration Plan

Land as a normal commit on master (no release-line involvement; 0.4.x frozen and untouched). No rollback beyond revert-the-commit; behavior change is confined to previously-strict callers, of which there are none in-tree besides tests.

## Open Questions

None.
