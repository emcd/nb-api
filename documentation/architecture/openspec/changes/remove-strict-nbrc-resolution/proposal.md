## Why

Native NBRC resolution now selects the store through the `ResolutionMode` enum (`IgnoreNbrc` / `ParseNbrc` / `EvaluateNbrc`). The parallel `strict: bool` (`Config.require_explicit_nb_dir`) — "ignore the rc AND refuse the file default when `NB_DIR` is unset" — overlaps the enum confusingly: two orthogonal controls where one reads as a modifier of the other.

## What Changes

- **BREAKING**: Remove `Config.require_explicit_nb_dir` and all `strict: bool` resolution parameters (`resolve_nbrc_store`, `resolve_with`, `notebook_root_dir`).
- Delete the strict short-circuit arm: with no `NB_DIR` set, every mode falls back to the `~/.nb` default store (the previous non-strict behavior).
- Update unit and integration tests that exercised strict (precedence-matrix strict cases, strict absolute-gate cases, the three integration users).
- Update code comments that document strict semantics.

## Capabilities

### New Capabilities

- `nbrc-resolution`: the store-resolution contract — unset `NB_DIR` resolves to the default store in every mode; no refusal mode exists.

### Modified Capabilities

None — no existing local spec pins strict behavior (verified by sweep); the nbspec-repo proposal that originally specified strict lives outside this repo and will need its own follow-up there.

## Impact

- `Config` loses a public field; `NbClient` loses the corresponding internal field. Any downstream caller setting `require_explicit_nb_dir: true` fails to compile and must decide its own unset-`NB_DIR` handling (check before constructing the client).
- Default behavior is unchanged (`require_explicit_nb_dir` defaulted to `false`).
- Pre-1.0: no compat shim per project policy.
