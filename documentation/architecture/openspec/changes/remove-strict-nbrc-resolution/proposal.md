## Why

Native NBRC resolution now selects the store through the `ResolutionMode` enum (`IgnoreNbrc` / `ParseNbrc` / `EvaluateNbrc`). The parallel `strict: bool` (`Config.require_explicit_nb_dir`) — "ignore the rc AND refuse the file default when `NB_DIR` is unset" — overlaps the enum confusingly: two orthogonal controls where one reads as a modifier of the other.

## What Changes

- **BREAKING**: Remove `Config.require_explicit_nb_dir` and all `strict: bool` resolution parameters (`resolve_nbrc_store`, `resolve_with`, `notebook_root_dir`).
- Delete the strict short-circuit arm: after mode-specific resolution leaves the store unselected (no preset, rc absent or selecting nothing), resolution falls back to the `~/.nb` default when the home directory is computable. Rc-supplied stores, rc failures, uncomputable fallbacks, and selected-path validation are unchanged.
- Update unit and integration tests that exercised strict (precedence-matrix strict cases, strict absolute-gate cases, the three integration users).
- Update code comments that document strict semantics.

## Capabilities

### New Capabilities

- `nbrc-resolution`: the store-resolution contract — after mode-specific resolution, a still-unselected store resolves to the default store when home is computable; no refusal mode exists.

### Modified Capabilities

None — no committed local spec pinned strict behavior. The in-flight `complete-native-cutover` lifecycle matrix did (unset fails fast); amended at proposal stage (reviews/95) and reframed to NBRC delegation (reviews/96), so no two specs contradict. The nbspec-repo proposal that originally specified strict lives outside this repo and needs its own follow-up (tracked below).

## Impact

- `Config` loses a public field; `NbClient` loses the corresponding internal field. Any downstream use of the `require_explicit_nb_dir` field literal — even `false` — fails to compile and must be deleted by the caller, which must then decide its own unset-`NB_DIR` handling (fall back, or check before constructing the client).
- Default behavior is unchanged (`require_explicit_nb_dir` defaulted to `false`).
- Pre-1.0: no compat shim per project policy.
