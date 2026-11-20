## 1. Production removal

- [ ] 1.1 Remove `strict` from `resolve_nbrc_store` / `resolve_with` in `src/nbrc/mod.rs`, deleting the short-circuit arm; unset `NB_DIR` falls through to the default store in every mode.
- [ ] 1.2 Remove `strict` from `notebook_root_dir` in `src/client/lifecycle.rs` and update its doc comment.
- [ ] 1.3 Remove `Config.require_explicit_nb_dir` (field, default, builder) in `src/lib.rs` and the `NbClient` threading in `src/client/core.rs` / `src/client/read.rs`.
- [ ] 1.4 Sweep resolution-adjacent `strict` / `require_explicit` mentions in comments and docs (`lifecycle.rs` module docs, `nbrc/walk/mod.rs`, error strings).

## 2. Test updates

- [ ] 2.1 Update `src/nbrc/tests.rs`: remove strict precedence-matrix cases and strict absolute-gate cases; keep the Ignore/Parse absolute-gate coverage.
- [ ] 2.2 Update the three integration users (`show_probe.rs`, `native_nbrc.rs`, `native_reads_guard.rs`) to default resolution, preserving their underlying coverage.
- [ ] 2.3 Run fmt, clippy (both configs), full suite, and the Windows target check.

## 3. Proposal hygiene

- [ ] 3.1 Validate the change (`openspec validate --strict` for the change) and file it for Tier-1 review per the reviews procedure.
