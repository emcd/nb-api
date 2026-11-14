# native-import Specification

## Purpose
Define native file import into a notebook: ingesting external content as a committed note without spawning the `nb` CLI.

## ADDED Requirements

### Requirement: Native file import
The system SHALL import a local file into a notebook folder as a committed note, reading bytes natively and committing through the single-checkpoint transaction pipeline under the process-shared gate.

#### Scenario: Import local file
- **WHEN** a caller imports an existing, readable regular file into a notebook folder
- **THEN** a new committed note SHALL exist with the file's content and a typed outcome SHALL be returned

#### Scenario: Import missing file fails typed
- **WHEN** a caller imports a local path that does not exist or is not readable
- **THEN** the system SHALL return a typed I/O failure and SHALL NOT create any note or commit

#### Scenario: Concurrent import is contained
- **WHEN** an external writer races the import commit
- **THEN** the system SHALL apply the same collision/rollback discipline as `Transaction::commit` and SHALL never silently overwrite

### Requirement: Concrete import signature and outcome
The public entry point SHALL be `import_note(source: &Path, folder: Option<&str>, filename: Option<&str>, notebook: Option<&str>) -> Result<CommitOutcome, NbError>`: path-typed source, optional destination folder, optional filename override, same outcome shape as every other one-shot writer.

#### Scenario: Signature shape
- **WHEN** a caller reviews the public API
- **THEN** import SHALL take a `&Path` source with optional folder/filename and return `CommitOutcome`

### Requirement: Byte and format policy
The system SHALL store valid UTF-8 text files with content byte-identical to the source (same semantics as a `Transaction` add-note create with no title); the pipeline cannot represent anything else. Non-UTF-8 or binary sources SHALL be refused with a typed format failure; no bytes are transformed, transcoded, or silently dropped.

#### Scenario: Text import preserves bytes
- **WHEN** a valid UTF-8 text file is imported
- **THEN** the stored note content SHALL be byte-identical to the source file

#### Scenario: Binary import refused
- **WHEN** a non-UTF-8 or binary file is imported
- **THEN** the system SHALL refuse with a typed format failure and SHALL create nothing

### Requirement: Source admission policy
The system SHALL import only regular files. Directories, symlinks, and special files (sockets, FIFOs, devices) SHALL be refused with a typed failure. Import copies; the source file is never moved, modified, or deleted.

#### Scenario: Directory source refused
- **WHEN** a directory is passed as the import source
- **THEN** the system SHALL refuse with a typed failure without touching the tree

#### Scenario: Symlink and special sources refused
- **WHEN** a symlink, socket, FIFO, or device is passed as the import source
- **THEN** the system SHALL refuse with a typed failure without following anything

### Requirement: Destination naming and confinement
The system SHALL resolve the destination name with the filename override winning when given, else the source basename. Names SHALL pass through the existing mangling rules with the extension preserved. Folder and filename SHALL be confined with the exact-path policy: no escape from the destination notebook.

#### Scenario: Override and default names
- **WHEN** a filename override is supplied versus omitted
- **THEN** the override SHALL be used, else the source basename, mangled per existing rules

#### Scenario: Destination cannot escape
- **WHEN** folder or filename inputs attempt traversal outside the notebook
- **THEN** the system SHALL refuse before any mutation

### Requirement: Explicit destination collision
Import names are explicit, so the system SHALL refuse taken destinations with hard `PathCollision` (explicit-`Transaction`-plan semantics), never an auto-suffix.

#### Scenario: Taken destination refuses
- **WHEN** the destination path already exists
- **THEN** the system SHALL return `PathCollision` and change nothing

### Requirement: Validate-before-create order and refusal purity
All notebook-independent admission checks (source readability/type/format, destination name validity and confinement) SHALL run before any notebook creation. A refused import SHALL produce no note, no index edit, and no commit: import-owned effects are exactly none.
When the notebook is missing and creation is enabled, creation (directory plus initial commit) is an explicit persistent effect: if post-create steps refuse or fail, the newly created notebook persists with its initial state plus all legitimate concurrent-writer changes — a post-create collision may itself be the racing writer creating and committing the target — and is removed only if it is still exactly as created AND no other writer has touched it. Absence of effects means absence of IMPORT-owned effects throughout. The system SHALL never reset, clean, or otherwise disturb another writer's commits or worktree contents to satisfy purity wording.

#### Scenario: Invalid source creates nothing
- **WHEN** source admission fails and the notebook is also missing
- **THEN** the import SHALL be refused and no notebook directory SHALL be created

#### Scenario: Post-create refusal persists the notebook plus legitimate external changes
- **WHEN** the source is valid, the missing notebook is created, and the destination then collides — including a collision caused by a racing writer committing the target
- **THEN** the import SHALL be refused with `PathCollision`, no IMPORT-owned note/index/commit effects SHALL exist, and the created notebook SHALL persist with its initial state plus all legitimate external changes

#### Scenario: Raced writer is preserved
- **WHEN** an external writer commits during a failing import
- **THEN** their commit SHALL survive; purity assertions apply to import-owned effects only

#### Scenario: Exact unchanged state without creation or racers
- **WHEN** an import is refused before any creation with no concurrent writer
- **THEN** `HEAD`, `.index` lines, and worktree contents SHALL be byte-identical to before the call

### Requirement: File-only sources, URL refusal in spec
The system SHALL accept only local filesystem paths. URL-shaped sources (containing `://`) SHALL be refused with `ValidationError` before any network or subprocess activity; no bytes are fetched and `nb` is never consulted.

#### Scenario: URL source refused without network
- **WHEN** a URL-shaped source is passed
- **THEN** the system SHALL refuse with `ValidationError` and perform zero network I/O and zero subprocess spawns

### Requirement: Import filename index placement
The system SHALL map the source filename through the existing mangling and index-placement rules, so imported notes behave identically to transaction-created notes. (Destination name choice itself is governed by Destination naming and confinement above; collisions by Explicit destination collision.)

#### Scenario: Imported note is indexed
- **WHEN** a file is imported into a folder
- **THEN** the folder `.index` SHALL gain the note's basename under the same rules as `add_note`

## REMOVED Requirements

### Requirement: `convert`-assisted import
**Reason**: Native import never converts (no pandoc, no `nb --convert`); the `convert: bool` parameter cannot be honored and must not be silently ignored.
**Migration**: Remove the argument. Callers needing conversion convert beforehand and import the result; import itself is byte-faithful for accepted text.
