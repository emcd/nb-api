# nb-api

Typed Rust interface to the [nb](https://github.com/xwmx/nb) note-taking CLI.

## Purpose

`nb-api` provides a programmatic Rust client for [`nb`](https://github.com/xwmx/nb), the command-line
note-taking tool. It wraps `nb` as a subprocess, handling argument escaping,
notebook qualification, output parsing, and error recovery.

This crate is designed for use by:

- **`nb-mcp-server`** — the MCP server that exposes `nb` to LLM assistants
- **`nbspec`** — notebook-first OpenSpec orchestration
- Any Rust application that needs to drive `nb` programmatically

`nb-api` is intentionally free of MCP-specific dependencies (`rmcp`,
`schemars`). The `schemars` feature is available as an optional add-on for
consumers that need JSON Schema generation (e.g., MCP tool parameters).

## Quick Start

### Prerequisites

Install `nb` by following the official instructions:
[nb installation guide](https://github.com/xwmx/nb#installation).

### Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
nb-api = "0.3"
```

With optional JSON Schema support:

```toml
[dependencies]
nb-api = { version = "0.3", features = ["schemars"] }
```

### Example

```rust
use nb_api::{Config, NbClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        notebook: Some("myproject".to_string()),
        ..Default::default()
    };
    let client = NbClient::new(&config)?;

    // Create a note
    let outcome = client.add_note(
        Some("My Note"),
        "Note content with `backticks` works fine.",
        &["design".to_string(), "api".to_string()],
        Some("docs"),
        None,
    ).await?;
    println!("{:?}", outcome.ops[0].path);

    let shown = client.show_note("docs/…", None).await?;
    println!("{}", shown.fingerprint);

    // Search notes
    let results = client.search_notes(
        &["API".to_string()],
        nb_api::SearchMode::Any,
        &[],
        None,
        None,
    ).await?;
    println!("{}", results);

    Ok(())
}
```

## API Surface

`0.3.0` centers on a collect-then-commit [`Transaction`] and structured
reads. Inventory mutators are plan ops on `Transaction` with one-shot
`NbClient` wrappers that return [`CommitOutcome`]. List/search-style
reads still return ANSI-stripped CLI text. `edit_note` / `EditMode` are
**removed** — use `replace_note_body`, `edit_note_substring`, or
`edit_note_lines`.

Concurrency: notebook-scoped reads and `Transaction::commit` serialize on a
**process-shared, in-process** gate keyed by the notebook Git common-dir
realpath. Cross-process `index.lock` wait is deferred.

### Notes

| Method | Description |
|--------|-------------|
| `transaction` | Build a collect-then-commit plan (no I/O until `commit`) |
| `add_note` | Create a note (one-shot transaction; optional auto-name) |
| `show_note` | Structured [`ShowNote`] (path, kind, body fragments, fingerprint, source) |
| `show_note_lines` | Windowed body lines with `b3l1:` anchors (contiguous body only) |
| `search_note_lines` | Byte search over body line text (contiguous body only) |
| `replace_note_body` | Replace contiguous body with fingerprint precondition |
| `edit_note_substring` | Substring edit with occurrence + expected_count |
| `edit_note_lines` | Batch insert/delete/replace lines by number+anchor |
| `retitle_note` | Change title without moving path |
| `edit_note_tags` | Add/remove tags |
| `delete_note` | Delete a note |
| `move_note` | Move or rename a note (path/basename only) |
| `list_notes` | List notes with optional filtering |
| `search_notes` | Full-text search with OR/AND semantics |

### Todos

| Method | Description |
|--------|-------------|
| `add_todo` | Create a todo item with optional checklist |
| `mark_task_done` | Mark a todo (or specific task) as complete |
| `unmark_task_done` | Reopen a completed todo (or specific task) |
| `list_tasks` | List checklist items within todos, with optional status filter |

### Organization

| Method | Description |
|--------|-------------|
| `add_bookmark` | Save a URL as a bookmark |
| `import_note` | Import a file or URL (**one-shot only**, not a plan op) |
| `list_folders` | List folders in a notebook |
| `add_folder` | Create a folder |
| `list_notebooks` | List available notebooks |
| `show_notebook_status` | Show notebook status |
| `show_notebook_path` | Get the filesystem path for a notebook |

### Types

| Type | Description |
|------|-------------|
| `NbClient` | Async client for invoking nb commands |
| `Transaction` | Collect-then-commit plan; drop discards; `commit` validates-all / apply-all / ≤1 Git checkpoint |
| `CommitOutcome` / `OpOutcome` | Structured commit results |
| `ShowNote` / `ShowNoteLines` / `SearchNoteLines` | Structured read results |
| `NoteTarget` / `ByteString` / `LineEdit` / `Occurrence` / … | Wire types for MCP lockstep (see body-aware-editing spec) |
| `NbError` | Structured errors including `DirtyBaseline`, `IndeterminateCommit`, `RecoveryRequired`, `FragmentedBody`, fingerprint/anchor/occurrence mismatches. Serde is internally tagged (`"type": "…"`). |
| `Config` | Configuration for constructing `NbClient` |
| `SearchMode` | Query matching mode (any, all) |
| `TaskStatus` | Todo status filter (open, closed) |

### Document model (new in 0.3.0)

| Type | Description |
|------|-------------|
| `NoteDocument` | Lossless byte-range partition of a parsed `nb` note, todo, or bookmark. Accessors: `source`, `emit`, `kind`, `title`, `title_str`, `tags_prefix`, `tag_section`, `tags`, `tags_str`, `body`, `url`, `url_str`, `todo_state`. |
| `DocumentKind` | Discriminator: `Note`, `Todo`, `Bookmark`. |
| `TodoState` | Checkbox state for Todo: `Open`, `Done`. |
| `ParseContext` | How the parser determines kind: `FromPath(PathBuf)` (inferred from extension) or `Explicit(DocumentKind)` (caller-specified). |
| `TagsIter` / `TagsStrIter` | Iterators over tag tokens (raw bytes or `&str` with UTF-8 errors surfaced). |
| `BodyFragments` | Iterator over body byte ranges in source order. |
| `parse(bytes, context)` | The parser entry point. Permissive acceptance per the P1 spec; refusal only on the no-nonblank-line case for Todo/Bookmark. |
| `Fingerprint` | Versioned public token authenticating exactly the concatenated body_ranges bytes. Canonical form `b3:<64 lowercase hex>` (BLAKE3-256). |
| `fingerprint(&NoteDocument)` | Compute the BLAKE3-256 fingerprint over body_ranges bytes in source order. |

## Configuration

`Config` contains only nb-relevant fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `notebook` | `Option<String>` | `None` | Default notebook name (overrides Git-derived fallback) |
| `create_notebook` | `bool` | `true` | Automatically create missing notebooks |
| `allow_top_level_notes` | `bool` | `false` | Allow notes at notebook root without a folder |
| `disable_git_signing` | `bool` | `false` | Disable Git commit/tag signing for nb subprocesses |
| `gate_timeout` | `Duration` | `60s` | Max wait on the process-shared gate queue |

### Notebook Resolution

Priority order:

1. Per-command `notebook` argument (highest)
2. `Config.notebook` field
3. Git-derived default from the master worktree path

The `NB_MCP_NOTEBOOK` environment variable is **not** read by `nb-api`.
That is an MCP-server-specific convention resolved by `nb-mcp-server` before
constructing `Config`.

## Features

| Feature | Default | Description |
|---------|---------|-------------|
| `schemars` | disabled | Adds `JsonSchema` derive to `EditMode`, `SearchMode`, `TaskStatus` |
| `testing` | disabled | Exposes the `nb_api::testing` module with `NbTestEnv` and friends; pulls in `tempfile` as a dependency. Use for integration tests of consumers. |
| `testing-tokio` | disabled | Within `nb_api::testing`, reveals the async helpers `NbTestEnv::configure_tokio` and `NbTestEnv::nb_command_async`. The crate's own tokio usage (in `NbClient`) is unconditional and does not depend on this flag. Pair with `testing` to reach the async helpers. |

## License

[Apache 2.0](https://github.com/emcd/nb-api/blob/master/LICENSE)

## Repository History

This crate was extracted from
[`emcd/nb-mcp-server`](https://github.com/emcd/nb-mcp-server), where it lived
as a workspace member at `nb-api/`. The `nb-api 0.1.0` release was published
from that repository.

Starting with `0.1.1`, `nb-api` is developed and published from this
repository (`emcd/nb-api`). The split is governed by the
[`split-nb-api-repository`](https://github.com/emcd/nb-mcp-server/blob/master/openspec/changes/split-nb-api-repository)
OpenSpec proposal.

Git history begins fresh in this repository. Pre-split history (the work
extracted from `nb-mcp-server/nb-api/`) is preserved at
[`emcd/nb-mcp-server`](https://github.com/emcd/nb-mcp-server) on the
`master` branch. Archaeologists tracing the lineage of a particular change
should look there first.
