## Context

`resolve_with(mode, strict, ...)` in `src/nbrc/mod.rs` branches on `strict` before the mode match: strict forces `IgnoreNbrc` semantics plus refusal when `NB_DIR` is unset. The flag threads from `Config.require_explicit_nb_dir` through `NbClient` into `notebook_root_dir(mode, strict)`. Operator decision (2026-11-20): drop the refusal capability entirely rather than folding it into the enum. Constraints: pre-1.0, no compat shims; the in-flight `complete-native-cutover` change is unaffected (verified: no strict references).

## Goals / Non-Goals

**Goals:**

- One control (`ResolutionMode`) governs store resolution; unset `NB_DIR` uniformly falls back to `~/.nb`.
- Remove the public `Config.require_explicit_nb_dir` field and every `strict` parameter.

**Non-Goals:**

- No replacement refusal mechanism (explicitly declined by the operator).
- No changes to the three resolution modes' semantics.
- No spec changes (no spec pins this behavior).

## Decisions

- **Delete the strict arm, keep the Ignore arm as-is.** The strict arm was `Ignore` + refusal; deleting it leaves the existing `IgnoreNbrc` arm (preset-or-default) as the only unset-handling. No merge of behaviors, no new branches.
- **Remove parameters, don't deprecate.** Pre-1.0 policy forbids compat shims; the field default was `false`, so default-constructed callers are behaviorally unaffected and only explicit `require_explicit_nb_dir: true` users break at compile time with a clear error.
- **Update the three integration users in place** (`show_probe.rs`, `native_nbrc.rs`, `native_reads_guard.rs`) rather than deleting their tests: their coverage (probe behavior, NBRC matrix, guard refusal) remains valid under default resolution; only the strict setup goes away.

## Risks / Trade-offs

- [Risk] A downstream caller relied on strict refusal to catch misconfiguration, and now silently lands in `~/.nb`. → Mitigation: **BREAKING** is flagged in the proposal; the compile break forces each such caller to confront the removal and add its own explicit-unset check if it still wants refusal.
- [Risk] Leftover `strict` mentions in comments/docs drift from the code. → Mitigation: tasks include a repo-wide sweep of `strict`/`require_explicit` mentions in resolution-adjacent code and docs.

## Migration Plan

Land as a normal commit on master (no release-line involvement; 0.4.x frozen and untouched). No rollback beyond revert-the-commit; behavior change is confined to previously-strict callers, of which there are none in-tree besides tests.

## Open Questions

None.
