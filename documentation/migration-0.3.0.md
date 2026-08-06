# Migration: nb-api 0.2.x → 0.3.0

## Breaking removals

- `NbClient::edit_note` and `EditMode` are **gone**.
  - Full body replace → `replace_note_body(target, new_body, fingerprint)`
  - Substring edit → `edit_note_substring(...)`
  - Line edit → `edit_note_lines(...)` with `LineEdit` + `LineRef` anchors from `show_note_lines`
  - Append/prepend → express as line insert at `$` / `^` or substring/replace_body

## Return-type changes

| Method | 0.2.x | 0.3.0 |
|--------|-------|-------|
| `show_note` | `String` | `ShowNote` |
| `add_note` / `add_todo` / `add_bookmark` / `add_folder` / `delete_note` / `move_note` / `mark_task_done` / `unmark_task_done` | `String` | `CommitOutcome` |

List/search/status methods still return `String`.

## Transaction model (Nbspec)

Earlier design sketches used begin/stage/rollback. **0.3.0 ships collect-then-commit only:**

```rust
let mut tx = client.transaction(Some("nb")).await?;
tx.add_folder("proposals/chg")?;
tx.add_note("proposals/chg/proposal.md", Some("Title"), "body\n", &[])?;
let outcome = tx.commit().await?; // ≤1 Git commit; drop == discard
```

- Creates on `Transaction` require an **explicit final relative path**.
- One-shot wrappers may auto-name.
- `import_note` remains one-shot under the gate; **not** a plan op.
- `commit` refuses a dirty notebook worktree/index (`DirtyBaseline`).
- Unknown checkpoint completion → `IndeterminateCommit` (do not claim clean rollback).
- Failed cleanup verification → `RecoveryRequired`.

## Contiguous body

`show_note_lines`, `search_note_lines`, `edit_note_lines`, `edit_note_substring`,
and `replace_note_body` require `body_fragments.len() <= 1`. Multi-fragment
Bookmarks fail with `FragmentedBody`. Metadata ops (`retitle_note`,
`edit_note_tags`) still work.

## Errors / JSON

`NbError` is internally tagged: `{"type":"fragmented_body", ...}`.
New discriminants include `dirty_baseline`, `indeterminate_commit`,
`recovery_required`, `fingerprint_mismatch`, `anchor_mismatch`,
`occurrence_mismatch`, `overlapping_edits`, `invalid_line_window`,
`empty_substring_pattern`, `fragmented_body`, `gate_timeout`, `path_collision`.

## Config

New field: `gate_timeout: Duration` (default 60s).

## Consumers

- **nb-mcp-server**: lockstep tool schemas to the wire types in the
  body-aware-editing specification; drop `edit`/`EditMode`.
- **nbspec**: adapt active import to multi-op `Transaction` + explicit paths
  (not `import_note` batching). Supersedes begin/stage/rollback inventory.
