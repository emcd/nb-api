<!-- nbspec: change=add-0-2-0-foundation notebook=nb-api note=proposals/add-0-2-0-foundation/specifications/output-behavior_specification.md hash=sha256:ea7d2af481a4cd0d2db4f1d1a551566ec7e773d016a50be228139ddd21bceab5 -->
## ADDED Requirements

### Requirement: NbClient::list_notes and NbClient::list_folders SHALL sanitize the empty-result hint block

When the structural evidence of a native `nb` empty-result hint block is
detected, `NbClient::list_notes` and `NbClient::list_folders` SHALL return
only the empty-result signal (e.g., `0 items.` or `0 folders.` plus its
terminator), stripping the trailing hint suggestions (e.g.,
`Add a note:`, help information). Non-empty results SHALL be returned
unchanged.

Structural evidence requires ALL of:
1. The first line is a `0 <kind>.` signal (starts with `0 `, ends with `.`).
2. The line following the signal is blank (blank separator).
3. At least one line in the trailing block is a recognized hint marker
   (`Add a `, `Add an `, `Import a `, `Help information:`).

If any condition fails, the input SHALL be returned unchanged. False
negatives are preferred over destructive output loss — user-authored
content may legitimately begin with `0 items.` and must not be wrongly
truncated.

#### Scenario: Empty notebook list returns only the signal

- **WHEN** `NbClient::list_notes` is called against an empty notebook
- **THEN** the returned string SHALL contain only the `0 items.` signal
and its terminator. Trailing hint lines SHALL be stripped.

#### Scenario: Non-empty notebook list returns full output

- **WHEN** `NbClient::list_notes` is called against a notebook with notes
- **THEN** the returned string SHALL contain the note titles. The
trailing hint block SHALL NOT be present.

#### Scenario: User content matching the pattern is returned verbatim

- **WHEN** a user-authored note body begins with `0 items.` followed by
arbitrary text
- **THEN** the `show_note` method SHALL return that body verbatim. The
hint-block sanitization SHALL only apply to `list_notes` and
`list_folders` outputs.

#### Scenario: Tasks output is not sanitized

- **WHEN** `NbClient::list_tasks` is called against an empty todos
notebook
- **THEN** the existing `is_empty_tasks_error` / `empty_tasks_message`
handling SHALL apply. The hint-block sanitization SHALL NOT modify the output.

#### Scenario: Search output is not sanitized

- **WHEN** `NbClient::search_notes` returns no matches
- **THEN** the `! Not found in <notebook>: <query>` error SHALL
propagate as `NbError::CommandFailed`. The hint-block sanitization SHALL NOT
modify the output.

#### Scenario: Terminator is preserved exactly

- **WHEN** the empty-result signal ends with LF, CRLF, or has no
terminator
- **THEN** the returned string SHALL preserve the exact terminator. The
sanitization SHALL NOT alter the line terminator.

### Requirement: AGENTS.md compliance — inline tests via the public API + shim

Per `AGENTS.md`, inline `#[cfg(test)]` modules are permitted only when the
inline block contains at most ONE `#[test]` function AND no existing public
interface exercises the same code path. The `strip_empty_result_hint`
helper is invoked publicly by `NbClient::list_notes` and
`NbClient::list_folders`, so inline tests fail the second condition. All
helper coverage SHALL be exercised via the public
list-notes/list-folders behavior in `tests/integration/empty_result_hints.rs`,
using a deterministic `nb` shim in `PATH` (see
`tests/integration/common/mod.rs::with_shim_nb_env`) to emit crafted list
output for the LF / CRLF / no-terminator / no-recognized-marker
/ no-blank-separator cases that real `nb 7.24.0` does not produce. The
shim checks the subcommand and only emits `SHIM_OUTPUT` for list-like
invocations (`list`, `<notebook>:list`); non-list invocations pass through
to real `nb`, so concurrent sibling tests' fixture initialization is
unaffected.

#### Scenario: Hint-block helper is exercised via public list_notes/list_folders, not inline

- **WHEN** a developer adds a regression test for the hint-block
sanitization
- **THEN** the test SHALL be added under
`tests/integration/empty_result_hints.rs` and SHALL exercise the helper via
`NbClient::list_notes` or `NbClient::list_folders`. No inline
`#[cfg(test)]` SHALL be added to the helper module.
