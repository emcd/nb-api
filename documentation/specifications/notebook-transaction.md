<!-- nbspec: change=add-body-aware-note-editing notebook=nb-api note=proposals/add-body-aware-note-editing/specifications/notebook-transaction.md hash=sha256:6c401eed42135a60ebdfb3b5a5b0fbfc5c78435cd0f624a30a21d8a19f20b61f -->
# Notebook transaction specification

#nbspec #specification

## Purpose

Define the public `Transaction` collect-then-commit object: the sole
durable write foundation for NbApi 0.3 plan ops listed in the
inventory below. Body-aware editors are plan ops on the same type.

## ADDED Requirements

### Requirement: NbClient SHALL expose Transaction construction

`NbClient::transaction(notebook: Option<&str>)` SHALL return a
`Transaction` bound to the resolved notebook identity for planning.
Construction SHALL NOT acquire the notebook gate, SHALL NOT perform
durable I/O, and SHALL NOT create Git commits.

There SHALL be no public `begin` method and no public `rollback`
method.

#### Scenario: New transaction has empty plan and no side effects

- **WHEN** a caller constructs a `Transaction` and drops it without
  `commit`
- **THEN** the notebook filesystem and Git history SHALL be unchanged

### Requirement: Finite v1 Transaction mutator inventory

The v1 `Transaction` plan-op surface SHALL be exactly the following
set (ordinary names; no `stage_` prefix). No other durable mutator
SHALL be claimed as a plan op in 0.3.0.

| Plan op | One-shot wrapper | Notes |
|---|---|---|
| `add_note` | `NbClient::add_note` | Explicit final relative path required on `Transaction` |
| `add_todo` | `NbClient::add_todo` | Explicit final relative path required on `Transaction` |
| `add_bookmark` | `NbClient::add_bookmark` | Explicit final relative path required on `Transaction` |
| `add_folder` | `NbClient::add_folder` | Explicit notebook-relative folder path |
| `delete_note` | `NbClient::delete_note` | |
| `move_note` | `NbClient::move_note` | Path/basename only; not retitle |
| `mark_task_done` | `NbClient::mark_task_done` | |
| `unmark_task_done` | `NbClient::unmark_task_done` | |
| `replace_note_body` | `NbClient::replace_note_body` | New in 0.3 |
| `edit_note_substring` | `NbClient::edit_note_substring` | New in 0.3 |
| `edit_note_lines` | `NbClient::edit_note_lines` | New in 0.3 |
| `retitle_note` | `NbClient::retitle_note` | New in 0.3 |
| `edit_note_tags` | `NbClient::edit_note_tags` | New in 0.3 |

**Removed in 0.3.0 (not ported):** `edit_note`, `EditMode`.

**Explicitly not a Transaction plan op in 0.3.0:** `import_note`.
See import policy requirement below.

**Reads (not plan ops):** `show_note`, `show_note_lines`,
`search_note_lines`, `list_notes`, `search_notes`, `list_tasks`,
`list_folders`, `list_notebooks`, `show_notebook_status`,
`show_notebook_path`.

Each one-shot wrapper in the table SHALL be behaviorally equivalent
to `transaction → enqueue → commit` with the same arguments
(accounting for optional auto-name only on wrappers where documented).

#### Scenario: Inventory is closed

- **WHEN** a consumer inspects the 0.3.0 public `Transaction` API
- **THEN** every durable plan method SHALL appear in the inventory
  table above
- **AND** `import_note` and `edit_note` SHALL NOT be plan methods

### Requirement: import_note policy in 0.3.0

`import_note` SHALL remain available as a **one-shot**
`NbClient` method that:

1. Acquires the process-shared notebook gate.
2. Performs a single `nb import` (or equivalent single-checkpoint
   import) under that gate.
3. Releases the gate.

`import_note` SHALL **not** be a `Transaction` plan op in 0.3.0.
It therefore cannot participate in a multi-op validate-all/apply-all
plan with other mutators.

Rationale: source acquisition (local path vs URL), HTML convert,
and generated-path rules are owned by `nb import` today and do not
yet have a byte-snapshot plan representation suitable for virtual-
tree commit. Multi-op import batching is deferred to a later change.

When `filename` is supplied to one-shot `import_note`, the final
relative path SHALL be folder/filename as today. When omitted,
generated-path outcomes follow `nb import` and SHALL be echoed in
the one-shot result string/structured outcome as applicable.

Callers needing multi-file tree reconstruction in one commit SHALL
use `add_note` / `add_todo` / `add_folder` with **explicit paths**
and in-memory content, not `import_note`.

#### Scenario: import_note is not on Transaction

- **WHEN** a consumer lists `Transaction` methods
- **THEN** there SHALL be no `import_note` plan method

#### Scenario: one-shot import_note still serializes on the gate

- **WHEN** `import_note` runs concurrent with a `commit` on the same
  notebook repository
- **THEN** they SHALL be serialized by the process-shared gate

### Requirement: Transaction SHALL enqueue inventory mutators under ordinary names

Enqueue SHALL perform only cheap argument validation (empty
selector refused, empty substring target refused, contradictory tag
add/remove refused, invalid path components refused). Enqueue SHALL
NOT read notebook bytes, acquire the gate, or write.

#### Scenario: Enqueue add_note does not create a note

- **WHEN** `tx.add_note(...)` returns `Ok`
- **AND** the caller has not called `commit`
- **THEN** the notebook SHALL not show the new note

### Requirement: Create plan ops SHALL accept an explicit final relative path

Create-class plan ops (`add_note`, `add_todo`, `add_bookmark`,
`add_folder`) SHALL accept an explicit **notebook-relative final
path** (single relative path, or folder + filename). The final path
SHALL include the filename for file creates.

Create ops SHALL NOT rely on `nb` auto-generated timestamp basenames
as the only naming mechanism when an explicit path is supplied.

Cheap enqueue validation SHALL refuse empty paths, absolute paths,
parent-segment escape (`..`), and empty filenames. Commit-time
validation SHALL refuse collisions against the commit snapshot and
against the virtual tree of earlier plan ops.

One-shot wrappers MAY keep filename optional for auto-name
ergonomics; the **`Transaction` create surface SHALL always accept
an explicit final path** as first-class. `CommitOutcome` per-op
results for creates SHALL echo the final relative path and canonical
selector actually written.

#### Scenario: Explicit path create materializes stable schema filename

- **WHEN** a transaction enqueues `add_note` with final relative
  path `proposals/demo/proposal.md` and `commit` succeeds
- **THEN** the commit tree SHALL contain `proposals/demo/proposal.md`
- **AND** the per-op outcome SHALL report that path (or equivalent
  selector)

#### Scenario: Two creates to the same final path fail at commit

- **WHEN** a transaction enqueues two create ops with the same
  final relative path
- **THEN** `commit` SHALL fail with a typed collision error naming
  the failing plan index
- **AND** neither create SHALL be durable
- **AND** HEAD SHALL be unchanged

#### Scenario: Import-style tree in one commit

- **WHEN** a transaction creates folder `proposals/chg/`, note
  `proposals/chg/proposal.md`, and todo `proposals/chg/work.todo.md`
  via explicit paths and `commit` succeeds
- **THEN** exactly one new Git commit SHALL exist
- **AND** all three paths SHALL be present with caller-supplied names

### Requirement: commit baseline is a clean notebook worktree and index

After acquiring gates and before any apply, `commit` SHALL inspect
the notebook Git worktree and index. If there is any staged change,
unstaged tracked change, or untracked file that Git would include in
status for that repository (v1: **any** dirty state, not only
paths the plan would touch), `commit` SHALL refuse with a typed
**dirty-baseline** error, apply nothing, and create no checkpoint.

v1 does **not** attempt to preserve or merge with caller dirty
state. Callers MUST commit or clean the notebook before
`Transaction::commit`.

#### Scenario: Dirty worktree refuses before apply

- **WHEN** the notebook has an unstaged modification to any tracked
  file
- **AND** the caller invokes `commit` with a non-empty plan
- **THEN** `commit` SHALL fail with dirty-baseline
- **AND** HEAD and the worktree SHALL be unchanged by that call

#### Scenario: Clean baseline allows commit

- **WHEN** `git status` is clean for the notebook repository
- **AND** the plan is valid
- **THEN** `commit` MAY proceed to validate and apply

### Requirement: commit SHALL validate all then apply all then checkpoint once

`Transaction::commit(self)` SHALL, after a clean baseline check:

1. Record `pre_revision` = current notebook `HEAD`.
2. Read a commit-time snapshot sufficient for the plan.
3. Validate **every** plan op against that snapshot and a virtual
   tree that applies earlier plan ops in order (path collisions,
   folder-then-note, add-then-edit by path).
4. If any op fails validation, apply **no** durable changes, create
   **no** checkpoint, return a typed error with plan index; HEAD
   remains `pre_revision`; worktree remains clean.
5. If validation succeeds, apply all ops such that the notebook Git
   tree matches the full final intent (explicit final paths for
   creates).
6. Create **at most one** new notebook Git commit. Pure no-op plans
   SHALL create zero commits and leave HEAD at `pre_revision`.
7. On known success, return `CommitOutcome` with `revision_id` when
   a commit was created, per-op outcomes, fingerprints where
   applicable.
8. On known apply/checkpoint failure where Git did **not** create a
   new commit, restore isolation postcondition (below) and return a
   typed failed-commit error (not indeterminate).

#### Scenario: Multi-op plan produces one commit

- **WHEN** a transaction enqueues two `add_note` ops for distinct
  explicit final paths and `commit` succeeds
- **THEN** exactly one new Git commit SHALL exist
- **AND** both notes SHALL be present at those paths
- **AND** the worktree SHALL be clean

#### Scenario: Second op validation failure applies nothing

- **WHEN** op 0 would succeed alone but op 1 fails validation
- **THEN** `commit` SHALL return an error referencing plan index 1
- **AND** HEAD SHALL equal `pre_revision`
- **AND** the worktree SHALL be clean

#### Scenario: Drop without commit is discard

- **WHEN** a transaction with a non-empty plan is dropped
- **THEN** no commit SHALL be created

### Requirement: Apply isolation and rollback postcondition

Because v1 requires a clean baseline, apply isolation SHALL ensure:

- During apply, durable writes occur only under the held notebook
  gate.
- If apply or checkpoint fails **and** the implementation can
  determine that no new commit was created, `commit` SHALL attempt
  to restore the notebook to: `HEAD == pre_revision` and a **clean**
  worktree and index (equivalent to discarding only this attempt's
  writes). Restoration MUST NOT be implemented as an unconditional
  destructive reset that is claimed while commit success is unknown.

Private staging directories outside the notebook worktree (then a
single publish into the notebook) are a permitted isolation
mechanism. Direct in-worktree apply with deterministic cleanup on
failure is permitted only if the clean-baseline invariant holds.

After a known-failure cleanup attempt, `commit` SHALL verify
postcondition with Git status/HEAD read:

- If `HEAD == pre_revision` and status is clean, return the ordinary
  typed failed-commit / validation error for the original cause
  (cleanup verified).
- If cleanup or verification fails, return typed
  **`RecoveryRequired`** (not a claim of clean rollback). JSON
  discriminant `"type": "recovery_required"`. Fields SHALL include:
  `pre_revision`, `post_revision_observed` (optional),
  `status_observed` (optional concise porcelain or equivalent),
  `preserved_paths` (optional list of staging or backup paths still
  present), and `guidance` telling the caller not to retry blindly
  and to inspect HEAD/status and any preserved paths.

The clean rollback postcondition SHALL NOT be claimed in error
display or structured flags unless verification succeeded.

#### Scenario: Known failed apply leaves clean pre_revision

- **WHEN** apply fails before any new commit exists
- **AND** cleanup verification succeeds
- **THEN** HEAD SHALL equal `pre_revision`
- **AND** `git status` SHALL be clean
- **AND** the error kind SHALL NOT be `recovery_required`

#### Scenario: Cleanup failure is RecoveryRequired

- **WHEN** apply fails and subsequent cleanup cannot verify a clean
  worktree at `pre_revision`
- **THEN** the error type SHALL be `recovery_required`
- **AND** the error SHALL carry `pre_revision`
- **AND** the error SHALL NOT claim that rollback left a clean tree

### Requirement: Indeterminate commit outcome

When `commit` cannot determine whether a new Git commit was created
(for example a subprocess/transport timeout after the commit
subprocess may have started), it SHALL NOT return a generic error
that claims HEAD is unchanged or that rollback succeeded.

It SHALL return a typed **`IndeterminateCommit`** error (name
stable for matching) that includes at least:

- `pre_revision` (HEAD observed before apply)
- `post_revision_observed`: `Option` of HEAD after best-effort re-read
- concise recovery guidance: re-read notebook HEAD and status;
  do not retry the same plan blindly; reconcile manually or via
  show/list before a new transaction

#### Scenario: Transport failure after possible commit is indeterminate

- **WHEN** the checkpoint subprocess fails in a way that leaves
  commit success unknown
- **THEN** the error kind SHALL be indeterminate-commit
- **AND** the error SHALL carry `pre_revision`
- **AND** the error SHALL NOT claim the worktree was restored to a
  clean pre-commit state unless that was actually verified

### Requirement: Multi-op commit SHALL NOT use per-op auto-checkpointing calls

When applying a plan that requires more than one logical mutation,
`commit` SHALL NOT implement apply as a sequence of independent
`nb` mutator invocations that each create their own checkpoint.
The durable result SHALL be a single checkpoint of the final tree.

Single-op plans MAY delegate to one `nb` verb when that yields
exactly one checkpoint, honors explicit final path when required,
and equivalent semantics; otherwise the native apply path SHALL be
used even for a single op.

#### Scenario: Three adds do not create three commits

- **WHEN** a transaction successfully commits three `add_note` ops
  with distinct explicit paths
- **THEN** notebook history SHALL gain exactly one commit from that
  `commit` call

### Requirement: commit SHALL manage Git/index locking via the gate

While applying and checkpointing, `commit` SHALL hold the
process-shared notebook gate (and global gate when required).

#### Scenario: Commit holds gate for full apply-to-checkpoint

- **WHEN** a multi-op `commit` runs concurrent with another
  notebook-scoped read on the same repository identity
- **THEN** the read SHALL not observe intermediate apply state
- **AND** both operations SHALL serialize on the process-shared gate

### Requirement: CommitOutcome is structured

`CommitOutcome` SHALL expose at least: whether a commit was created,
`revision_id` if so, `pre_revision`, ordered per-op results (final
relative path and/or canonical selector, no-op flag, optional new
body fingerprint, op-specific fields).

#### Scenario: Successful create outcome echoes final path

- **WHEN** a create op with explicit final path succeeds
- **THEN** the per-op result SHALL include that final relative path

#### Scenario: Successful body edit outcome carries fingerprint

- **WHEN** a plan contains a successful body mutation
- **THEN** the per-op result SHALL include the new body fingerprint
  matching `fingerprint(parse(new_bytes))`

### Requirement: Plan errors are typed with index

Validation and apply errors originating from a specific plan entry
SHALL carry the zero-based plan index and a stable error kind
suitable for MCP/tool recovery branching (including path collision,
dirty-baseline, fingerprint/anchor/occurrence mismatch).

#### Scenario: Error names plan index

- **WHEN** the third enqueued op fails anchor validation
- **THEN** the error SHALL identify plan index 2
