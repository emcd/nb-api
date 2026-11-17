# native-notebook-lifecycle Specification

## Purpose
Define native notebook lifecycle management: create, ensure/exists, and path resolution directly against `NB_DIR` without spawning the `nb` CLI.

## ADDED Requirements

### Requirement: Native notebook creation
The system SHALL create a notebook by initializing its on-disk layout under `NB_DIR` (git worktree plus the marker/index files the native index code understands) without spawning any subprocess other than `git`.

#### Scenario: Create new notebook
- **WHEN** a caller creates a notebook that does not exist under `NB_DIR`
- **THEN** the notebook directory SHALL be initialized and subsequent operations SHALL treat it as an existing notebook

#### Scenario: Create existing notebook is refused
- **WHEN** a caller creates a notebook that already exists under `NB_DIR`
- **THEN** the system SHALL return a typed already-exists failure and SHALL NOT alter the existing notebook

### Requirement: Native notebook existence check
The system SHALL determine notebook existence by probing the `NB_DIR` directory entry, returning a typed result without spawning the `nb` CLI (git verification permitted).

#### Scenario: Existing notebook resolves
- **WHEN** a caller ensures a notebook that exists under `NB_DIR`
- **THEN** the system SHALL succeed without creating, modifying, or checkpointing anything

#### Scenario: Missing notebook is terminal NotFound on read paths
- **WHEN** a read-path caller references a notebook missing under `NB_DIR`
- **THEN** the system SHALL return `NotFound` and SHALL NOT create the notebook as a side effect

### Requirement: Native notebook path resolution
The system SHALL resolve a notebook's path from the `NB_DIR` layout without spawning `nb`, returning the canonical directory path.

#### Scenario: Path of existing notebook
- **WHEN** a caller requests the path of an existing notebook
- **THEN** the system SHALL return `$NB_DIR/<name>` without invoking the `nb` CLI (git verification permitted)

#### Scenario: Path of missing notebook
- **WHEN** a caller requests the path of a missing notebook
- **THEN** the system SHALL return `NotFound` (or create it, only on explicit create paths)

### Requirement: Pinned notebook layout and immediate usability
A notebook SHALL be a `$NB_DIR/<name>` directory containing `.git` (the existing validity probe) with a resolvable `HEAD` (initial commit at create time). `.index` files are created lazily by the index code as today; no other marker files are required.

#### Scenario: Created notebook is immediately committable
- **WHEN** a notebook is created and a `Transaction` adding a note commits against it with no intervening steps
- **THEN** the commit SHALL succeed and `HEAD` SHALL advance past the initial commit

#### Scenario: Initialization commits without signing
- **WHEN** the isolated git config requires commit signing
- **THEN** native creation SHALL still succeed: the initial commit is made with signing explicitly suppressed, matching the transaction commit policy

#### Scenario: Existing unrelated directory is never initialized
- **WHEN** ensure/create targets a path that exists but is not a valid notebook (plain directory without `.git`)
- **THEN** the system SHALL refuse with a typed not-a-notebook failure and SHALL NOT run `git init` inside it or remove anything

#### Scenario: Malformed repo surfaces verbatim
- **WHEN** the notebook directory has `.git` but `HEAD` is unresolvable or git operations fail
- **THEN** the underlying git failure SHALL surface unmasked (no reclassification to `NotFound`)

#### Scenario: Discovery I/O propagates
- **WHEN** probing the notebook directory hits an I/O error other than missing (permissions, etc.)
- **THEN** the system SHALL return a typed `Io` failure rather than treating it as absence

### Requirement: NB_DIR and create_notebook matrix
`NB_DIR` unset or pointing at a non-directory SHALL be a configuration failure reported before any probe. Creation SHALL be explicit or write-path ensured per `Config.create_notebook`; reads SHALL never create.

#### Scenario: Missing NB_DIR fails fast
- **WHEN** `NB_DIR` is unset or not a directory
- **THEN** every notebook operation SHALL fail with a typed configuration failure before probing anything

#### Scenario: Write path ensures when allowed
- **WHEN** `create_notebook` is true and a write path targets a missing notebook
- **THEN** the system SHALL create it and proceed

#### Scenario: Write path refuses when creation disabled
- **WHEN** `create_notebook` is false and a write path targets a missing notebook
- **THEN** the system SHALL return `ValidationError` naming explicit creation (no `nb` CLI reference)

#### Scenario: Read path never creates
- **WHEN** any read path targets a missing notebook regardless of `create_notebook`
- **THEN** the system SHALL return `NotFound` and create nothing

### Requirement: Name and root confinement
Notebook names SHALL pass the existing field validation plus explicit dot-segment rejection (`.` and `..` are never valid names); absolute, drive-letter, and separator forms SHALL be rejected. Resolution must stay under `NB_DIR`; symlink notebook roots are refused.

#### Scenario: Dot names refused
- **WHEN** a caller uses `.` or `..` as a notebook name
- **THEN** the system SHALL refuse with `ValidationError` before any filesystem access

#### Scenario: Symlink root refused
- **WHEN** `$NB_DIR/<name>` is a symlink (including one escaping `NB_DIR`)
- **THEN** the system SHALL refuse with `UnsupportedStructure` and follow nothing

### Requirement: Concurrent creation safety
Directory creation is atomic. A creator that did not create the directory SHALL NOT initialize or remove anything; simultaneous explicit creates resolve to exactly one winner with the rest refused as already-exists; ensure stays idempotent. On init/commit failure the partial directory is retained for inspection and reported; nothing is recursively deleted.

#### Scenario: Competing creators
- **WHEN** two creators race to create the same notebook
- **THEN** exactly one SHALL succeed and the other SHALL observe the valid notebook (ensure) or receive already-exists (explicit create), with no clobbering

#### Scenario: Partial-init failure retains for inspection
- **WHEN** creation fails after creating the directory but before the initial commit
- **THEN** the system SHALL retain the partial directory for inspection and report its location, and SHALL never recursively delete it

#### Scenario: Slow initializer still converges
- **WHEN** creation takes longer than one convergence interval to reach a resolvable `HEAD`
- **THEN** concurrent ensures SHALL still converge on the finished notebook instead of misclassifying it as not-a-notebook

### Requirement: Typed notebook enumeration
The system SHALL provide typed notebook enumeration returning the sorted names of `NB_DIR` entries satisfying the validity probe (directory with `.git`), replacing the enumeration half of the retired `list_notebooks`.

#### Scenario: Enumerate notebooks
- **WHEN** a caller enumerates notebooks
- **THEN** the system SHALL return the sorted valid notebook names with zero `nb` subprocesses (git verification permitted)

### Requirement: Two-way lifecycle interop (extended profile)
The retained oracle SHALL verify that native-created layouts open in true `nb` and `nb`-created layouts are recognized natively, since self-authored structural assertions cannot detect upstream layout drift.

#### Scenario: Native-created opens in nb
- **WHEN** the extended profile creates a notebook natively and opens it with true `nb`
- **THEN** `nb` SHALL operate on it normally

#### Scenario: Nb-created recognized natively
- **WHEN** the extended profile creates a notebook with true `nb`
- **THEN** native ensure/path/transaction flows SHALL accept it as valid
