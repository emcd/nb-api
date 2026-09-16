# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
