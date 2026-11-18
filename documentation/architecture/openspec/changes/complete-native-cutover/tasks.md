## 1. Native notebook lifecycle

- [ ] 1.1 Replicate `nb` notebook on-disk layout for create (git init + initial commit so `HEAD` resolves; `.index` lazily as today) behind a single create entry point
- [ ] 1.2 Rewire ensure/exists/path resolution to the `NB_DIR` probe; remove the `nb notebooks show --path` fallback; implement the `create_notebook` true/false × read/write matrix with no `nb` CLI references
- [ ] 1.3 Enforce name/root confinement (dot-segment, absolute/drive, separator rejection; symlink roots refused) before any mutation
- [ ] 1.4 Implement atomic concurrent create (single winner, idempotent ensure) with owned-only failure cleanup
- [ ] 1.5 Add typed notebook enumeration (`list_notebook_names`, sorted valid names)
- [ ] 1.6 Add purely-native layout/validity/matrix/concurrency tests plus extended two-way `nb`/native interop tests
- [x] 1.9 (0.4.6 hotfix) Resolve the notebook store as explicit `NB_DIR`, else parsed `~/.nbrc` `NB_DIR`, else `~/.nb`, with strict mode, no shell, warn-and-fallback for undeterminable rc content, and visible failures for missing HOME / unreadable rc / relative selection

## 2. Native import

- [ ] 2.1 Implement the `import_note(&Path, folder, filename, notebook) -> CommitOutcome` signature reading file bytes natively
- [ ] 2.2 Enforce the admission contract: regular files only (dirs/symlinks/special refused), UTF-8 text stored byte-identical, binary refused typed, `://` sources refused with no network, destination confined
- [ ] 2.3 Remove the `convert` parameter (no silent ignore); refuse `PathCollision` destinations; guarantee refusal purity (`HEAD`/index/worktree unchanged)
- [ ] 2.4 Cover with refusal/rollback/collision/content-identity/confinement tests for both happy and refusal paths

## 3. String surface retirement

- [ ] 3.1 Remove `nb`-backed String methods (`list_notebooks`, `list_notes`, `list_folders`, `list_tasks`, `search_notes`); add typed `list_notebook_names`; document each retired method's typed replacement
- [ ] 3.2 Audit `nb-mcp-server` call sites: record exact impacted String, lifecycle, import, and `convert` call sites with migration disposition for each, including scope/query/filter deltas, in the change

## 4. Machinery removal and test conversion

- [ ] 4.1 Add purely-native layout and behavior assertions to the default suite, replacing `nb`-oracle differentials
- [ ] 4.2 Add a dedicated Nextest profile running the retained true-`nb` oracle checks as the complete/extended suite
- [ ] 4.3 Remove `exec`/`exec_vec`, `nb_program` resolution, and `nb`-only fixture spawns from production paths; retain `ExecutableNotFound` and the integration harness
- [ ] 4.4 Re-evaluate the Windows-viability spike gating against the hermetic suite and record what remains

## 5. Validation

- [ ] 5.1 Default suite green on the exact entry points with `--all-features` throughout: `cargo test --locked --all-features`, `cargo nextest run --locked --all-features --no-tests=warn`, doctests, release run; clippy clean (default + testing), fmt clean, specs validate; align hook commands to match
- [ ] 5.2 Classify true independent `nb` comparisons into an explicit oracle set (`#[ignore]`-gated; pure-native safety tests stay unignored in default); extended command `cargo nextest run --locked --all-features --profile oracle --run-ignored all -E '<oracle-set filter>' --no-tests=fail`, failing on empty oracle selection and visibly when `nb` is missing; define `[profile.oracle]` in `.config/nextest.toml`
- [ ] 5.3 Prove default hermeticity with the logging dead-`nb` tripwire (zero invocations while git-backed tests pass)
- [ ] 5.4 CI green on ubuntu and Windows before requesting review
