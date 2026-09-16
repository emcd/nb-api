<!-- nbspec: change=add-body-aware-note-editing notebook=nb-api note=proposals/add-body-aware-note-editing/specifications/per-notebook-gate.md hash=sha256:4ed74e705fc7d91bba4764e76b6b01f3cbf9b358af887b69850736eaad2a0690 -->
# Per-notebook operation gate specification

#nbspec #specification

## Purpose

Serialize notebook-scoped reads and `Transaction::commit` so
concurrent callers cannot interleave durable writes or observe
half-applied apply-to-checkpoint windows. In-process only
(`issues/api/3` v1); process-shared across all `NbClient` values.

## ADDED Requirements

### Requirement: Process-shared gate registry keyed by repository identity

The crate SHALL maintain a **process-wide** gate registry (not a
field private to one `NbClient` value). Independently constructed
`NbClient::new` instances, clones, and clients using different
notebook **name aliases** that resolve to the same notebook
repository SHALL share one gate entry.

The registry key SHALL be a **canonical notebook repository
identity**: the realpath of the notebook Git directory (the
directory that contains that notebook's `HEAD` / Git metadata),
obtained after notebook name resolution. Two names that resolve to
the same Git directory MUST share one lock. Two distinct Git
directories MUST NOT share one notebook lock.

Gate entry creation SHALL be atomic on first use of a canonical
identity (lazy insert). Lookups after resolution SHALL not create
duplicate entries for the same identity.

#### Scenario: Two NbClient values serialize on the same notebook

- **WHEN** two independently constructed `NbClient` values commit
  concurrent transactions against the same resolved notebook
  repository
- **THEN** the commits SHALL be strictly serialized
- **AND** neither SHALL fail with an in-process-caused Git
  `index.lock` error from the sibling client

#### Scenario: Notebook aliases share one gate

- **WHEN** two notebook name strings resolve to the same canonical
  Git directory
- **AND** concurrent operations use those different names
- **THEN** they SHALL contend on the same gate entry

#### Scenario: Distinct notebooks do not share a notebook gate

- **WHEN** concurrent operations target two different canonical
  notebook Git directories
- **THEN** they MAY proceed concurrently (aside from the global
  gate when required)

### Requirement: Reads and commit hold the notebook gate; enqueue does not

Every notebook-scoped read and every `Transaction::commit` SHALL
hold the exclusive notebook gate for the full critical section
(snapshot through checkpoint or read completion). `Transaction`
enqueue methods SHALL NOT acquire the gate.

#### Scenario: Building a large plan does not block readers

- **WHEN** a caller enqueues many ops without calling `commit`
- **THEN** concurrent `show_note` on that notebook SHALL still be
  able to acquire the gate without waiting on plan construction

#### Scenario: Read cannot straddle a commit apply window

- **WHEN** a `commit` holds the lock between first durable write and
  checkpoint completion
- **THEN** a concurrent read SHALL block until the lock is released
- **AND** SHALL NOT return intermediate multi-op apply state

### Requirement: Global lock and lock order

Operations not keyed to a single notebook (for example listing
notebooks) SHALL use a separate process-shared **global** gate.
When an operation requires both global and notebook gates, it
SHALL acquire **global first, notebook second**, and release in
reverse order. No path SHALL acquire notebook then global.

#### Scenario: Dual-key commit uses global-then-notebook order

- **WHEN** a commit requires both gates
- **THEN** it SHALL acquire global before notebook

### Requirement: Gate queue timeout is distinct from commit failure

Exceeding queue wait SHALL return a typed gate-timeout error without
mutating the notebook. v1 SHALL NOT cancel an in-flight commit
because a waiter timed out.

#### Scenario: Queue timeout leaves notebook unchanged

- **WHEN** a caller times out waiting for the notebook gate
- **THEN** that caller SHALL not have mutated the notebook



### Requirement: Gate identity acquisition lifecycle

Canonical notebook repository identity SHALL be computed as follows:

1. Resolve the caller notebook name (or default) to a notebook
   root path using the same resolution rules as
   `show_notebook_path`.
2. If the notebook does not exist or is not an initialized `nb`
   notebook Git repository, return a typed resolution error and
   **do not** insert a gate registry entry.
3. Determine the Git directory via `git rev-parse --git-common-dir`
   when available (so linked worktrees sharing a common dir share
   one gate); if only `--git-dir` is available, use that. The
   result path SHALL be converted to a **realpath** (physical path
   with symlinks resolved). That realpath string is the registry key.
4. **Registry insert** of a new key SHALL occur only after successful
   identity computation, while holding the **global** gate long
   enough to lookup-or-insert the notebook gate entry (global then
   already-held or newly inserted notebook gate — never notebook
   before global). Concurrent first-users of the same identity SHALL
   obtain the same `Arc` gate.
5. If the repository at a path is **replaced** (directory removed and
   recreated with a new Git identity), a subsequent resolve MAY
   produce a new realpath or same path with new repo; the key is
   whatever realpath `--git-common-dir` yields after replacement.
   Stale unused registry entries MAY remain until process exit (v1
   does not require GC).

#### Scenario: Missing notebook does not insert a gate key

- **WHEN** a caller invokes a notebook-scoped operation with a name
  that does not resolve to an initialized notebook
- **THEN** the call SHALL fail with a resolution/not-found error
- **AND** the process-wide registry SHALL NOT gain an entry for that
  name

#### Scenario: Linked worktrees share one gate via common dir

- **WHEN** two worktrees share the same Git common directory
- **AND** both are used as notebook roots that resolve to that
  common dir realpath
- **THEN** they SHALL share one notebook gate entry

#### Scenario: Global gate covers resolve-plus-insert

- **WHEN** two threads first-touch the same new notebook identity
  concurrently
- **THEN** only one registry entry SHALL exist for that realpath
- **AND** both SHALL share that entry's lock

### Requirement: v1 gate scope is in-process only

Cross-process exclusion relies on Git locks plus command errors.
Cross-process `index.lock` wait/timeout is deferred
(`nb-api:todos/api/6`). Docs SHALL state in-process scope and the
process-shared registry guarantee explicitly.

#### Scenario: Documentation states process-shared in-process scope

- **WHEN** a consumer reads concurrency docs
- **THEN** the docs SHALL state that serialization is in-process,
  process-shared across clients, and keyed by canonical repository
  identity
