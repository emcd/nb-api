# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.4] - 2026-11-10

### Fixed

- Subcommand-colliding selectors (`todos`, `notebooks`, `list`, ...)
  resolve existing filesystem entries directly instead of shelling to
  `nb show <sel> --path` (which misdispatches into tasks/notebooks/list
  listings). `note_target_for_selector` and `resolve_item_path` now
  fast-path existing files and directories, falling back to `nb` only
  for numeric ids/titles; `show_note` on a folder is a typed
  `UnsupportedShowTarget` without spawning. Removes reconciling CLI
  reads from the hot path.

## [0.4.3] - 2026-11-10

### Added

- `delete_folder` plan op + one-shot (`recursive` flag, no `force`):
  removes a folder subtree, blanks the parent `.index` line, and
  refuses non-empty folders without `recursive` (contents listed).
  `delete_note` and `move_note` now refuse folder targets.
- `Transaction::assert_revision` optimistic-concurrency
  precondition on notebook HEAD: mismatch refuses cleanly with
  `revision_mismatch` and deletes nothing.
- Index-wedge guard relaxation: `commit` absorbs tracked-only
  `.index` dirt instead of `DirtyBaseline`-refusing; live
  `.nb-api-index.lock`, untracked files, and note-content dirt
  still refuse. `show_notebook_status` annotates the harmless case.

## [0.4.2] - 2026-11-02

### Fixed

- `list_tasks` invokes `nb todos tasks` per scope instead of legacy
  `nb tasks` (identical output; notebook/folder scope resolves on
  the split verb), and an empty scope returns the normalized empty
  signal (`0 open tasks.`) instead of a `ValidationError`. Recursive
  notebook-scope listing now covers subfolder todos.
- Shrinking Note tags to a single tag no longer orphans it into the
  body: a single `#tag` line directly after the title is recognized
  as the tags prefix (nb parity), so `show` tags and file bytes
  agree. Titleless position keeps the two-token rule.

## [0.4.1] - 2026-09-17

### Fixed

- `add_folder` now records the folder basename in the parent `.index`,
  mirroring `nb` (nb records folders it creates the same way). Without
  the folder line, `nb list <notebook>:<folder>/` fails at depth >= 2,
  so `list_notes` on empty subfolders of indexed folders errored.
- Numeric ids count every `.index` line including folders, matching
  `nb show <id>` numbering. In trees containing folders, `numeric_id`
  and numeric selectors may differ from 0.4.0 values.

## [0.4.0] - 2026-09-05

Breaking pre-1.0 release. See `documentation/migration-0.4.0.md` for the
full migration guide.

### Changed

- Text-first wire types: `ByteString` removed entirely; all structured
  textual fields are JSON `String` (`ShowNote` title/body/source,
  `BodyFragment` bytes plus `start_byte`/`end_byte` offsets into `source`,
  `NoteLine` text, bare-text `LineEdit` content, search hits, substring
  pattern/replacement). The `base64` dependency is dropped.
- Per-line terminators replaced by document-level
  `ShowNoteLines { eol: Option<LineEol>, has_final_eol }` with
  first-supported-occurrence detection; bare `\r` preserved verbatim;
  exact-span line anchors; edits on `eol: None` documents adopt `Lf`.
- One-shot creates use nb-faithful filenames (ASCII-locale title mangling
  with Unicode preserved, local-time titleless names, `-N` collision
  retries); the opaque `{epoch}-{seq}` scheme is removed.
- `Transaction` maintains folder `.index` files with stable positional ids
  (append, blank-on-delete, in-place rename, blank+append across folders).
- Outcomes and reads expose numeric `<folder>/<id>` selectors plus
  `numeric_id`; `NoteTarget::Selector` accepts numeric ids. Same-folder
  growing renames are refused at plan validation.
- Cross-process `.index` safety: `O_APPEND` appends, atomic range removal
  where the platform offers it, nonce lock with heartbeat
  (`IndexLockTimeout` on contention).

### Added

- `NbError::NonUtf8` for non-UTF-8 files on the text-only surface, with
  `NbClient::read_note_source_bytes` as the raw-bytes escape hatch.
- `NbError::IndexLockTimeout` for `.index` lock contention.

## [0.3.1] - 2026-08-15

### Added

- Windows support: `CreateProcess`-aware `nb` resolution, `setup-nb`
  GitHub Action, and a Windows live-`nb` test harness, with hardened
  `NbTestEnv` fixture initialization.

## [0.3.0] - 2026-08-13

### Added

- Collect-then-commit `Transaction` plan model (validates-all, applies-all,
  at most one Git checkpoint) with one-shot `NbClient` wrappers returning
  `CommitOutcome`.
- Body-aware editing: contiguous-body reads (`ShowNote`, `ShowNoteLines`),
  fingerprint preconditions, anchor-checked line edits, substring edits, and
  `FragmentedBody` refusal for multi-fragment documents.
- Lossless note document model with BLAKE3 body fingerprints.
- `gate_timeout` configuration for the process-shared notebook gate.

### Removed

- `NbClient::edit_note` and `EditMode`; use `replace_note_body`,
  `edit_note_substring`, or `edit_note_lines`.

## [0.2.1] - 2026-07-18

### Fixed

- Git commit-signing overrides when the parent process defines Git
  configuration environment entries.

## [0.2.0] - 2026-07-17

### Added

- Native textual-classification show probe with typed errors for
  non-textual targets.
- Empty-result hint block sanitization for list and folder responses.
- Duplicate-title H1 rejection on note creation.
- Hermetic `NbTestEnv` integration test fixture.

## [0.1.3] - 2026-07-11

### Fixed

- Scrubbed inherited `GIT_*` routing variables at every spawn site so
  notebook Git operations cannot redirect into the caller's repository.

## [0.1.2] - 2026-07-11

### Fixed

- Passed `--print` to `nb show` to preserve stored bytes (e.g. long lines)
  on read.

## [0.1.1] - 2026-07-04

### Changed

- First release developed and published from this repository after the
  split from `nb-mcp-server` (where `0.1.0` was published).
