<!-- nbspec: change=add-0-2-0-foundation notebook=nb-api note=proposals/add-0-2-0-foundation/designs/design__nb-api_0.2.0_public-api_refinements_foundation.md hash=sha256:a7dd4bda16f1c51cd51d63c9308e8cd14a7b95a103275d3a64a504ae54579c1d -->
### D2. Output sanitization — empty-result hint block (list-style methods)

Sanitize the trailing hint block (e.g., `0 items.` followed
by `Add a note:` and help suggestions) from raw `nb` output
before returning to callers. **Scope: list-style methods
only** (`list_notes`, `list_folders`).

**Detection approach:** require structural evidence of a
native hint block before truncating. All three conditions
must hold:
1. First line is a `0 <kind>.` signal (starts with `0 `,
   ends with `.`).
2. The line following the signal is blank (blank separator).
3. At least one line in the trailing block is a recognized
   hint marker (`Add a `, `Add an `, `Import a `,
   `Help information:`).

If any condition fails, return input unchanged. False
negatives are preferred over destructive output loss —
user-authored content may legitimately begin with
`0 items.` and must not be wrongly truncated.

**Terminator preservation:** the helper preserves the
signal's exact terminator (LF, CRLF, or none) so callers
downstream see the same line terminator they would have
seen without sanitization.

**Placement:** in `NbClient` method level (return clean
output per call), inside a new `output.rs` helper module.
Lean toward a small dedicated helper rather than
per-method logic.

**`nb` CLI flag check:** as of `nb 7.24.0`, `nb ls` /
`nb ls --type folder` / `nb bookmark` have NO flag to
suppress the hint block. The wrapper approach is canonical.
The spec scenario "Hint block is preserved if nb CLI exposes
a flag to suppress it natively" leaves room for switching
to a native flag in a future `nb` release.

**Method scope (intentionally narrow):**
- `NbClient::list_notes` — applies the helper.
- `NbClient::list_folders` — applies the helper.
- `NbClient::list_tasks` — NO change; existing
  `is_empty_tasks_error` / `empty_tasks_message` handles
  the single-line `! 0 ... tasks.` format.
- `NbClient::search_notes` — NO change; `! Not found in
  `<notebook>`: `<query>`` propagates as `CommandFailed`,
  preserved as a distinct contract.
- User-content methods (`show_note`, `add_note`,
  `edit_note`, `delete_note`, `move_note`, `import_note`,
  etc.) — NOT processed by the helper. A note whose body
  matches the empty-result hint-block pattern is returned
  verbatim.

**Coordination with MCP:** `nb-mcp-server:todos/mcp/30`
MAY apply a thin error-presentation wrapper for
formatting empty-result signals in MCP tool responses,
but SHOULD NOT duplicate the parser logic verbatim.
Duplicating the parser risks drift from `nb-api`'s
helper. The git_env defense-in-depth pattern (independent
subprocess scrubbing) is a different shape than output
sanitization; it does NOT imply that output sanitization
must be independently parsed by both layers.

### D5. EditMode vocabulary-trap fix (`public-api-surface` capability)

`EditMode::Replace` is renamed to `EditMode::Overwrite` and the canonical
serialization is changed from `"replace"` to `"overwrite"`. The legacy
string `"replace"` is accepted as a serde alias for backward compatibility
with payloads produced before the rename, but it is **not** advertised in
the derived [`schemars`](https://docs.rs/schemars) JSON Schema. Append and
Prepend variants are unchanged.

#### Root cause: vocabulary trap (not missing default)

The destructive-incident root cause tracked at `nb-api:issues/api/6`
originated in a vocabulary trap, not in a missing-default bug. Callers
reading `mode: "replace"` reasonably expected a substring-style
replacement (analogous to `str::replace` /
`String::replace`), but `nb edit --overwrite` is destructive — it
replaces every byte of the note body. Praetor explicitly passed
`mode: "replace"` intending substring replacement; no omission or
default was involved (`nb-mcp-server:issues/mcp/6`).

The variant name `Replace` normalized this footgun into a one-character
disaster: it was the literal text a caller would type when reaching for
"do a replace", and `nb` already advertises its destructive behavior as
`--overwrite`. Renaming `Replace` → `Overwrite` aligns the Rust API name
with the `nb` CLI flag name and makes the destructive intent unambiguous
at the call site.

#### Defense layers

The fix is layered; Phase 5 implements only the **API-layer** defense.

1. **API-layer vocabulary fix (Phase 5, this proposal).** Variant rename,
   canonical serialization, serde alias for backward compat. Implemented
   in `src/lib.rs::EditMode`. Verification via
   `tests/nb_types.rs::edit_mode_*` and
   `tests/integration/edit_mode_required.rs`.

2. **MCP-layer requiredness (downstream consumer work).** Drop
   `#[serde(default)] mode` from the consumer's containing struct so a
   missing `mode` field is a schema-level rejection rather than a silent
   default. This is a cross-repo change in `nb-mcp-server`'s `EditArgs`
   (and any other consumer that wants to require `mode` explicitly).
   Tracked separately; not in scope for Phase 5.

3. **Server-side requiredness (MCP transport layer).** MCP tool schemas
   can be derived from the consumer's `EditArgs` struct. With layer 2 in
   place, missing `mode` becomes a schema-validation failure before the
   `NbClient::edit_note` call. Layer 1 (this proposal) provides the
   vocabulary fix needed at the type level.

#### Implementation

- Rename `EditMode::Replace` → `EditMode::Overwrite` in `src/lib.rs`.
- Add `#[serde(alias = "replace")]` on `EditMode::Overwrite` so legacy
  payloads continue to deserialize.
- Keep `Default` on `EditMode` with `EditMode::Overwrite` as the default
  variant. The variant name now carries the destructive intent; an
  explicit `Default` does not reintroduce the trap. **Requiredness is a
  consumer-layer concern** — downstream consumers that want to require
  `mode` explicitly should drop `#[serde(default)]` from their
  containing struct.
- Update the `EditMode` enum doc comment with a per-variant mapping table
  (canonical serialization + `nb edit` flags + effect on body) and an
  explicit note that the alias is not advertised in the derived schema.
- Update the `NbClient::edit_note` doc comment to reference `EditMode`
  for the vocabulary rationale.

#### Behavioral mapping

| Variant | Canonical | Alias | `nb edit` flag(s) | Effect on note body |
|---------|-----------|-------|-------------------|---------------------|
| `EditMode::Overwrite` | `"overwrite"` | `"replace"` (compat) | `--overwrite --content <content>` | Replaces every byte of the body with `<content>`. Destructive: any existing content is lost. |
| `EditMode::Append` | `"append"` | — | `--content <content>` | Appends `<content>` after the existing body. `nb` normalizes trailing newlines and may add a blank-line separator. |
| `EditMode::Prepend` | `"prepend"` | — | `--prepend --content <content>` | Prepends `<content>` before the existing body. `nb` normalizes trailing newlines. |

#### Schema-level verification

The derived JSON Schema must advertise only the canonical value
`"overwrite"`; the legacy `"replace"` alias must NOT appear in the
schema. If both appeared, MCP tool consumers would see both values in
the schema dropdown and the vocabulary trap would be re-introduced at
the schema layer. Verified by
`tests/nb_types.rs::edit_mode_schema_advertises_overwrite_canonical_only`
(under the `schemars` feature gate).

#### Compile-time and behavioral tests

Stable Rust does not support negative trait bounds, so a "negative
assertion" that `EditMode` does not derive `Default` cannot be expressed
in the type system. The integration test module
`tests/integration/edit_mode_required.rs` therefore uses:

- A `_compile_time_check_edit_signature_compiles` helper that calls
  `NbClient::edit_note` with each variant (`core::mem::drop` on the
  futures to silence clippy::let_underscore_future). If `mode` were
  ever made optional in the future, this helper would still compile
  (with `mode` defaulted), which would be a regression — but the
  structural defense is the variant name itself: `Overwrite` makes the
  destructivity unambiguous at the call site.

Behavioral tests use `NbTestEnv` end-to-end with each mode:

- `edit_overwrite_destroys_existing_body` — verifies `Overwrite`
  destroys original body content.
- `edit_append_preserves_and_extends_body` — verifies `Append` keeps
  original and adds new.
- `edit_prepend_preserves_and_prepends_body` — verifies `Prepend` adds
  new before original and preserves original.
- `edit_overwrite_with_single_byte_body` — boundary: overwrite
  multi-line body with single byte.

Plus serde tests in `tests/nb_types.rs`:

- `edit_mode_canonical_serialization_is_overwrite` — canonical
  serialization is `"overwrite"`.
- `edit_mode_deserializes_canonical_overwrite` — `"overwrite"`
  deserializes to `EditMode::Overwrite`.
- `edit_mode_deserializes_legacy_replace_as_alias` — legacy
  `"replace"` deserializes to `EditMode::Overwrite` (backward compat).
- `edit_mode_rejects_unknown_values` — unknown strings (including
  `"Overwrite"` wrong-case, `"REPLACE"`, `"delete"`, empty string) fail
  to deserialize.
- `edit_mode_schema_advertises_overwrite_canonical_only` (under
  `schemars` feature) — derived JSON Schema contains `"overwrite"`,
  `"append"`, `"prepend"` and NOT `"replace"`.

#### Boundary notes

- Empty content (`--content ""`) is rejected by `nb` itself regardless of
  mode, so there is no meaningful empty-content boundary to test at the
  `nb-api` layer. The integration tests use qualitative assertions
  (`contains`, `starts_with`, `find`) rather than exact equality because
  `nb` normalizes whitespace during append/prepend.
- The integration tests use `client.add_note(None, body, ...)` so no
  `# title\n\n` header is prepended, giving clean control over the
  stored body.
- `EditMode` derives both `Serialize` (added) and `Deserialize` so JSON
  round-trips for logging, debugging, and MCP tool schemas work
  symmetrically. The legacy alias is on deserialization only; the
  canonical serialization is always emitted.

#### Out of scope

- `edit_note_lines` (insert/replace by line range) — tracked at
  `nb-api:todos/api/4`.
- `edit_note_substring` (replace by match) — tracked at
  `nb-api:todos/api/4`.
- Rename of `edit` to `edit_note` — DONE in Phase 6
  (`public-api-surface` capability).
- MCP-layer requiredness — downstream consumer work
  (`nb-mcp-server`'s `EditArgs` struct).

### D6. API surface renames (`public-api-surface` capability)

Phase 6 hard-renames the `NbClient` public API to a uniform
`<action>_<resource>` shape. The pre-Phase-6 method names do NOT survive
as `#[deprecated]` aliases; `nb-api 0.2.0` ships only the new names.
This is a deliberate hard break: the 0.1.x API has no production
consumers beyond `nb-mcp-server` and `nbspec`, both of which will be
updated in lockstep with the release. Keeping `#[deprecated]` aliases
would preserve the vocabulary ambiguity the renames are designed to
remove.

#### Vocabulary pattern

The rename applies a uniform `<action>_<resource>` shape across all
three subsystems. The resource noun makes the target kind explicit;
the action verb names the operation.

- **Notes API** uses `<action>_note` for single-resource and
  `<action>_notes` for collection operations, with `note`/`notes`
  making the resource explicit (`add_note`, `show_note`, `edit_note`,
  `delete_note`, `move_note` for single-resource; `list_notes`,
  `search_notes` for collections).
- **Todos API** follows the same resource-qualified pattern for
  collection operations (`add_todo`, `list_tasks`), with symmetric
  `mark_task_done` / `unmark_task_done` for task state transitions.
  The pre-Phase-6 names `do_task` / `undo_task` rely on generic
  do/undo vocabulary that does not name the task state transition
  explicitly; the new names make the transition ("mark done" /
  "unmark done") the unambiguous verb phrase. **`list_tasks` invokes
  `nb tasks`** (lists checklist items within todos); the future
  `list_todos` for todo **container** listing (invoking `nb todos`)
  is tracked separately as deferred work.
- **Organization API** methods likewise use explicit action + resource
  names (`add_bookmark`, `add_folder`, `import_note`, `list_folders`,
  `list_notebooks`, `show_notebook_status`, `show_notebook_path`).
  Where the operation is on an attribute rather than a resource-kind
  noun (notebook status/path), the action verb carries the leading
  position with the attribute as the resource qualifier.

#### Renames (17 total)

Notes API (6 renames + `move_note` unchanged):

| New | Old |
|-----|-----|
| `add_note` | `add` |
| `show_note` | `show` |
| `edit_note` | `edit` |
| `delete_note` | `delete` |
| `list_notes` | `list` |
| `search_notes` | `search` |
| `move_note` | `move_note` (unchanged) |

Todos API (4 renames):

| New | Old |
|-----|-----|
| `add_todo` | `todo` |
| `mark_task_done` | `do_task` |
| `unmark_task_done` | `undo_task` |
| `list_tasks` | `tasks` |

Organization API (7 renames):

| New | Old |
|-----|-----|
| `add_bookmark` | `bookmark` |
| `import_note` | `import` |
| `list_folders` | `folders` |
| `add_folder` | `mkdir` |
| `list_notebooks` | `notebooks` |
| `show_notebook_status` | `status` |
| `show_notebook_path` | `notebook_path` |

#### Internal call sites

All internal call sites within `src/lib.rs::NbClient` and the
integration tests under `tests/integration/*.rs` are updated to
the new names in the same Phase 6 commit. The pre-commit hook
(which runs `cargo test --locked --all-features`) catches any
missed call site before the commit lands.

#### Documentation

`README.md` example code and the API surface tables (Notes,
Todos, Organization sections) are updated to the new names. The
`EditMode` type description in the Types table is updated to
reference `edit_note` instead of `edit`.

#### Coordination with MCP and Nbspec

Both `nb-mcp-server` Owner and `nbspec` Owner are looped in
for review of the diff. The cross-repo consumer updates are
tracked at `nb-mcp-server:todos/mcp/31` and the Nbspec rolling
handoff; the API rename in `nb-api` is a prerequisite for
those consumer updates.

#### Out of scope

- `edit_note_lines` (insert/replace by line range) — tracked at
  `nb-api:todos/api/4`.
- `edit_note_substring` (replace by match) — tracked at
  `nb-api:todos/api/4`.
- `list_todos` (todo **container** listing, invoking
  `nb todos`) — tracked as deferred work. The current
  `list_tasks` method invokes `nb tasks` (checklist items); a
  future `list_todos` method for todo container listing is the
  mirror-image deferred item.
- Partial-edit primitives (`replace_note`, `append_to_note`,
  `edit_substring`, `edit_lines`) — future 0.3.0+ work; see
  `nb-api:ideas/2`.
- MCP-side requiredness on the `mode` field of consumer
  `EditArgs` — downstream consumer work (`nb-mcp-server`'s
  switch commit).
