## 1. Production removal

- [ ] 1.1 Remove `strict` from `resolve_nbrc_store` / `resolve_with` in `src/nbrc/mod.rs`, deleting the short-circuit arm; a store left unselected after mode-specific resolution (no preset, rc absent or selecting nothing) falls back to the default store when home is computable. Rc-supplied stores, rc failures, uncomputable fallbacks, and selected-path validation stay as they are.
- [ ] 1.2 Remove `strict` from `notebook_root_dir` in `src/client/lifecycle.rs` and update its doc comment.
- [ ] 1.3 Remove `Config.require_explicit_nb_dir` (field, default, builder) in `src/lib.rs` and the `NbClient` threading in `src/client/core.rs` / `src/client/read.rs`.
- [ ] 1.4 Sweep resolution-adjacent `strict` / `require_explicit` mentions in comments and docs (`lifecycle.rs` module docs, `nbrc/walk/mod.rs`, error strings).

## 2. Test updates

- [ ] 2.1 Update `src/nbrc/tests.rs`: remove strict precedence-matrix cases and strict absolute-gate cases; keep the Ignore/Parse absolute-gate coverage.
- [ ] 2.2 Repurpose strict-only test assertions: remove the `require_explicit_nb_dir` field literal entirely from the three integration users (`show_probe.rs`, `native_nbrc.rs`, `native_reads_guard.rs`) — the field will not exist, so even a `false` literal breaks compilation; convert "strict unset refuses" assertions into default-fallback assertions, preserving the underlying coverage.
- [ ] 2.3 Run fmt, clippy (both configs), full suite, and the Windows target check.

## 3. Proposal hygiene and coordination

- [ ] 3.1 Validate the change (`openspec validate --strict` for the change) and file it for Tier-1 review per the reviews procedure.
- [ ] 3.2 Coordinate the nbspec follow-up: the approved `add-native-nbrc-resolution` proposal (reviews/88, nbspec repo) specifies strict; notify nbspec-o with the precise amendment ask (unset falls back, no refusal) so the specs do not drift.
- [x] 3.3 Done at proposal stage (reviews/95): amended the in-flight `complete-native-cutover` lifecycle matrix (unset `NB_DIR` now falls back; non-directory still fails fast) so no two local specs contradict.
