## 1. Shared postcondition helper

- [x] 1.1 Add file-local `assert_notebook_settled` + `git_capture` to `tests/integration/eol_model.rs` in the pilot shape (plain `git rev-parse --verify HEAD^{commit}` non-empty, `git status --porcelain` empty, `scrub_git_env_std`, no client, no `nb`, no loops).
- [x] 1.2 Convert `insert_appends_document_crlf` to `NbTestEnv::hermetic()` with settled postconditions after setup and after the insert (pilot replay; proves the helper on master).

## 2. Read-path EOL conversions

- [x] 2.1 Convert `lf_body_reports_document_eol`, `cr_only_body_is_single_line_with_preserved_cr`, `stray_cr_before_first_supported_eol_preserved`, `mixed_body_uses_first_supported_eol` to `hermetic()` with setup postconditions; keep all EOL assertions byte-identical.
- [x] 2.2 Convert `final_line_without_eol_uses_null_marker` and `insert_after_on_cr_only_adopts_lf` to `hermetic()` with setup and post-mutation postconditions; keep anchor and terminator assertions byte-identical.

## 3. Edit-path EOL conversions

- [x] 3.1 Convert `insert_at_dollar_without_final_eol_normalizes` and `replace_final_line_without_eol_normalizes` to `hermetic()` with setup and post-mutation postconditions; keep body-equality assertions byte-identical.
- [x] 3.2 Convert `insert_at_caret_into_empty_body` to `hermetic()` with setup and post-mutation postconditions; keep the Caret-accepts / Dollar-refuses split byte-identical.

## 4. Validation

- [x] 4.1 Run `cargo test --locked --all-features` (default suite), `cargo clippy --all-targets` (default + testing), `cargo fmt --check`, and `openspec validate --all --strict`; record named `eol_model` execution on Linux.
- [x] 4.2 Record named `eol_model` execution on Windows for the exact tip and attach the run reference to the change.
