<!-- nbspec: change=add-body-aware-note-editing notebook=nb-api note=proposals/add-body-aware-note-editing/designs/design__body-aware_editing_foundation.md hash=sha256:22b38277348f703e05a5de26db31e4bd2bcd3cd2578df40100a37a55fab6fc92 -->
# Design: notebook transaction foundation

#nbspec #design

## Goals / Non-Goals

### Goals

- Public **collect-then-commit** `Transaction` as the sole multi-op
  durable write path for the **finite v1 inventory**.
- One Git checkpoint per successful `commit` (zero on pure no-op).
- No durable side effects until `commit`; drop == discard.
- Clean-worktree baseline; typed indeterminate-commit when HEAD
  stability cannot be known.
- Process-shared gate keyed by canonical notebook Git identity.
- Explicit final paths on creates; path-at-final-location virtual tree.
- Wire-level body-edit types for MCP lockstep.
- One-shot wrappers for inventory ops; hard-remove `edit_note`.

### Non-Goals

- begin/stage/rollback inventory (superseded).
- Cross-process `index.lock` wait (`todos/api/6`).
- `import_note` as a plan op in 0.3 (one-shot only).
- Plan-local handles in v1.
- History/restore; full native rewrite of all reads.

## Architecture

```
Process-wide gate registry
  key = realpath(notebook git dir)
  global gate + per-repo notebook gates

NbClient (many instances share registry)
  ├── reads → notebook gate
  ├── import_note (one-shot only) → notebook gate → single nb import
  ├── transaction() → Transaction { plan }
  └── inventory one-shots → tx → single op → commit

Transaction::commit
  → gates
  → refuse if dirty baseline
  → pre_revision = HEAD
  → validate plan on snapshot + virtual tree
  → apply (native multi-op / single nb when safe)
  → ≤1 checkpoint OR known failure restore OR IndeterminateCommit
  → CommitOutcome
```

## Decisions

### D1–D7

Unchanged in substance: collect-then-commit; ordinary names;
transaction-primary; commit owns lock; multi-op ≠ N× nb checkpoints;
reads on client; hard-remove edit_note; body fingerprint = P1.

### D8 — Path-at-final-location (no handles)

Creates and later ops use explicit paths; virtual tree at commit.
Nbspec confirmed. MCP F3 (old create finding) closed via Nbspec F1.

### D9 — Explicit final path on creates

As in `notebook-transaction` spec.

### D10 — Clean-worktree baseline (MCP F1)

**Choice:** `commit` refuses if the notebook repo is dirty (any
staged/unstaged/untracked-as-status noise per `git status`
porcelain). No merge with caller dirty state in v1.

**Rationale:** Avoid destroying caller WIP with reset, and avoid
contaminating the transaction commit with unrelated index state.

### D11 — Isolation + indeterminate commit (MCP F1)

**Choice:** On known failure before a new commit exists, restore
`HEAD == pre_revision` and clean worktree. Prefer private staging
outside the notebook then single publish. On unknown commit
completion, return `IndeterminateCommit { pre_revision,
post_revision_observed, guidance }` — never claim rollback/HEAD
stability when unverified.

### D12 — Process-shared gate by repo identity (MCP F2)

**Choice:** Process-wide registry; key = realpath of notebook Git
dir after name resolution. Covers independent clients and aliases.

### D13 — Finite inventory; import deferred from Transaction (MCP F4)

**Choice:** Closed table in `notebook-transaction` spec. `import_note`
is one-shot under gate only; not a plan op. Tree rebuild uses
`add_*` + explicit paths + content bytes.

### D14 — Wire-level body contracts (MCP F3)

**Choice:** `b3l1:` 128-bit BLAKE3 line anchors; inclusive line
ranges; `Occurrence` First|All|Nth; body byte domain; virtual `^`/`$`;
typed errors — as in `body-aware-editing` spec.

## Implementation sequence

1. Process-shared gate + independent-client concurrency test.
2. `Transaction` + clean baseline + indeterminate error + creates
   with explicit paths + multi-op single checkpoint.
3. Port inventory mutators + wrappers; exclude import from plan.
4. Structured show + lines/search + body plan ops.
5. Remove edit_note; 0.3.0; migration notes (Nbspec + MCP).

## Review history

- Operator difit LGTM; `todos/api/6` defer.
- Nbspec Owner APPROVE after path F1.
- MCP Owner REVISE `reviews/8` F1–F4 — addressed D10–D14 + specs.
- Nbspec ack: path F1 stands; no new nbspec verdict unless regression.


### D15 — Contiguous-body-only line/substring/replace (MCP reviews/9 F1)

**Choice:** v1 `show_note_lines`, `search_note_lines`,
`edit_note_lines`, `edit_note_substring`, and `replace_note_body`
require `body_fragments.len() <= 1`. Fragmented Bookmarks fail with
`FragmentedBody`. `show_note` still exposes `body_fragments`,
`body_contiguous`, and concatenated `body` for fingerprint parity.
Metadata ops still work on fragmented docs.

**Rejected for v1:** silent concat editing; full multi-fragment
mutation model (future).

### D16 — Exact wire types (MCP reviews/9 F2)

**Choice:** Fixed public names (`ShowNote`, `NoteTarget`,
`ByteString` as `{base64}`, internally tagged enums with `type`,
etc.) as specified in `body-aware-editing`. No "may match later"
escape hatches.

### D17 — Gate lifecycle + RecoveryRequired (MCP reviews/9 F3)

**Choice:** Identity via resolve → `git-common-dir` realpath;
insert under global gate; missing notebook no insert. Cleanup
failure → `RecoveryRequired` with evidence fields; never claim
verified-clean unless verified.
