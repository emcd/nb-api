## Why

The `eol_model` suite (10 document-EOL scenarios) still builds its notebooks through `NbTestEnv::new()`, which spawns real `nb`. That setup path preserves the Windows checkpoint-race family Quasar scoped in `nb-api:issues/13`, and keeps native-behavior coverage gated on a live-`nb` harness it does not need. The pilot conversion of `insert_appends_document_crlf` to the hermetic fixture passed on Windows twice (diag runs 45302943296, 45305290891) plus exact-tip qualification (master run 45637617332 for the handshake family), proving the pattern restores deterministic native coverage.

## What Changes

- Convert the full EOL batch (all 10 `eol_model` tests) from `NbTestEnv::new()` to `NbTestEnv::hermetic()`, reusing the pilot's shape: synchronous-git setup, no `nb` spawn, HEAD-peels-to-commit plus clean-tree postconditions around the existing EOL assertions.
- Test-only change: no production code, no API, no behavior change. Existing EOL assertions stay byte-identical; only fixture construction and added postconditions change.
- Keep the conversion native-only: oracle and coexistence tests stay Unix-gated per doctrine; no `#[cfg(unix)]` changes outside `eol_model.rs` in this batch.

## Capabilities

### New Capabilities

- `eol-native-coverage`: EOL document-model scenarios run hermetically with explicit settled-repo postconditions.

### Modified Capabilities

- None: no product requirement changes.

## Impact

- `tests/integration/eol_model.rs` only (fixture lines + shared postcondition helper). No `src/` changes expected; if the helper wants a shared home, `tests/integration/common/mod.rs` is the only second file.
- CI: removes 10 live-`nb` setups from the Windows path for this file; evidence for this batch is named-test execution on Windows plus the full Linux suite. Later batches (mixed families, case-by-case) are out of scope.
