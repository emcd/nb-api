## Why

`nb-api` still shells out to the `nb` CLI for notebook lifecycle, file import, and every verbatim String rendering surface. Each spawn is a correctness risk (auto-checkpoint races, `GIT_*` inheritance, Windows `PATHEXT`/launcher gaps) and the direct source of the Windows CI instability: the failing jobs are all live-`nb` harness races, and the oracle differential tests themselves spawn real `nb`. Removing the last subprocess dependency makes the crate hermetic, makes CI deterministic, and unblocks the Windows-viability work on a fully native tree.

## What Changes

- Native notebook lifecycle under `NB_DIR`: create (`notebooks add` equivalent), ensure/exists, and path resolution without spawning `nb`. `ensure_notebook` / `show_notebook_path_unguarded` stop shelling out; the `NB_DIR`-directory fast path becomes the only path.
- Native file import with the same gate + checkpoint discipline as `Transaction::commit`, including containment for the import race window that `nb import` cannot provide (no `--wait`).
- **BREAKING**: Retire the `nb`-backed verbatim String rendering surfaces (`list_notebooks`, `list_notes`, `list_folders`, `list_tasks`, `search_notes` String forms). Typed APIs are the supported surface; byte-faithful `nb` CLI text is `nb`'s job, not the library's.
- **BREAKING**: Retire `import_note`'s `nb`-backed implementation in favor of the native import (same method name, native behavior; output text changes).
- Remove the `nb` spawn machinery from production paths once no caller remains (`exec`/`exec_vec`, `nb_program` resolution, `nb`-only fixture spawns in production setup). `git` subprocesses and their env scrubbing stay. The `nb` integration test harness is retained for true-oracle checks (see design D4).
- Convert `nb`-oracle differential tests to purely-native assertions in the default suite; keep the true-`nb` oracle behind a dedicated Nextest profile as the complete/extended suite.

## Capabilities

### New Capabilities

- `native-notebook-lifecycle`: create, ensure/exists, and path resolution for notebooks directly against `NB_DIR` and the filesystem, with typed errors (`NotFound` for missing, no auto-create on read paths) and no subprocess.
- `native-import`: import a local file into a notebook folder as a committed note, under the process-shared gate with single-checkpoint semantics and a typed outcome; race containment equivalent to `Transaction::commit`. File-only v1; URL sources are refused (tracked as `nb-api:todos/4`).
- `string-surface-retirement`: removal schedule and migration notes for the retired String APIs (what to call instead for each one).

### Modified Capabilities

- None: no existing product capability specs exist (current specs cover repository mechanics only).

## Impact

- `src/client/core.rs` (ensure/path/exec machinery), `src/client/read.rs` (String lists, search), `src/client/write.rs` (`list_tasks` String, `import_note`), `src/nb_program.rs` (retire production spawn paths), `src/testing/` (fixture no longer needs to spawn `nb notebooks add`; oracle helpers retained behind the Nextest profile).
- `NbError::ExecutableNotFound` is retained for the integration harness (design D4).
- Downstream: `nb-mcp-server` consumes this crate — audit its use of the String, lifecycle, import, and `convert` call sites (exact impacted call sites and migration disposition for each) before release (pre-1.0, no compat shims per operator direction).
- CI: Windows job loses its live-`nb` dependency; the viability-spike gating work gets re-evaluated against a hermetic suite.
