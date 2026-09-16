<!-- nbspec: change=add-note-document-model notebook=nb-api note=proposals/add-note-document-model/decisions/20260730133144.md hash=sha256:e6d66066cd2902b365395a2391050fdc7e28c3c34d85c9ad34e58078b79f4c99 -->
#parser #architecture #refactor #p1-refinement

# Decision 17: Layered parser architecture (tokenizer + classifier)

Subject: refactor of `src/parser.rs` to separate format-agnostic
line scanning from format-specific classification.

## Status

Proposed. Captured 2026-07-30 from operator consultation
during MCP Owner review cycle-2 wrap-up. Refines P1 in flight;
not a new proposal.

## Context

The current `src/parser.rs` (1313 lines) interleaves three
orthogonal activities:

1. **Line-level scanning** — `consume_utf8_bom`,
   `consume_blank_lines`, `consume_leading_blank_intervals`,
   `line_end_at`, `terminator_end_at`, `consume_one_line`,
   `consume_atx_h1_title`, `consume_tags_prefix_line`,
   `consume_url_line`, `is_valid_atx_h1`, `is_tags_prefix_line`,
   `is_url_line`, `strip_leading_spaces`. All consume one line
   at a time and mutate a position cursor.
2. **Block-level scanning** — `sections_from_headings`,
   `scan_h2_sections_at_or_after`,
   `scan_h2_sections_at_or_after_with_fence_awareness`. Produce
   a `Vec<H2Section>` with full-section ranges and an
   `is_inner_body_h2` flag.
3. **Partition assembly** — `build_note_partition`,
   `build_todo_partition`, `build_bookmark_partition`. Mix
   line-level calls, block-level reasoning, and ad-hoc line-end
   awareness (the `is_complete_blank_line` check in Todo).

The R2-F4 fix exposed this: the Bookmark cursor had to reason
about line terminators inside body-fragmentation logic when the
architecture should keep those concerns separated.

## Decision

### Architecture

Three layers, top-down:

- **Tokenizer** — single forward pass over source; yields a
  `Vec<Line>` (or a `LineCursor` iterator). Format-agnostic.
  Never inspects `##`/`#`/`=`. Recognizes line terminators and
  emits one `Line` per physical line.
- **Classifier** — parameterized by format + document kind.
  Walks `Line`s and produces a classified token stream (e.g.,
  `ClassifiedToken::Heading { level, text, full_section }`,
  `ClassifiedToken::Body`, `ClassifiedToken::BlankLine`, etc.).
  Format-specific. Knows that "## Tags" in Markdown is a tags
  section header; "## Tags" in Org would be an H2 with body
  text "Tags."
- **Partition assembler** — consumes classified tokens and
  produces the `Partition` enum. Format-agnostic. Walks
  classified tokens, applies the partition rules (range
  partitioning, body fragmentation, canonical-tags selection).
  No byte-level reasoning.

### Token shape

```rust
pub struct Line<'a> {
    pub range: Range<usize>,        // includes terminator
    pub terminator: &'a [u8],       // b"", b"\n", b"\r\n", b"\r"
    pub content: &'a [u8],          // bytes preceding terminator
}
```

The `terminator` field is `&'a [u8]` borrowed from source, NOT
`&'static [u8]`. The empty terminator case (EOF-unterminated
line) carries `b""`.

For a source fragment `# Title\n`:
```
Line {
    range: 0..8,                   // includes \n
    terminator: b"\n",             // 1 byte
    content: b"# Title",           // 7 bytes
}
```

### Format dispatch

`parse()` and `parse_with_context()` inspect the extension
(or `ParseContext::Explicit`) and route to a format-specific
parser:

- `.md`, `.todo.md`, `.bookmark.md` → Markdown parser (current)
- `.org` → `ParseError::UnsupportedDocumentFormat` (todo)
- `.latex` → `ParseError::UnsupportedDocumentFormat` (todo)
- `.adoc`, `.asciidoc` → `ParseError::UnsupportedDocumentFormat` (todo)

The error is explicit and typed. Pre-refactor, P1 silently
misclassifies these as `Note` and applies Markdown parsing.

The dispatch is an explicit step in the public API, not a hidden
path. The format-routing decision is rendered before the
classifier is invoked.

### `is_inner_body_h2` rename

The current `H2Section::is_inner_body_h2` flag is a Markdown-
specific concept. Renamed to `is_section_internal_heading` to
reflect the format-neutral framing. The classifier computes
this property (it requires looking at the parent section, a
global property); the tokenizer does not.

For Bookmark, the flag is set when the Markdown classifier sees
a kebab-case heading (non-tags, non-Content, non-Source) inside
a `Content` or `Source` block. For Org/AsciiDoc, the concept
does not apply — those formats have no `Content`/`Source`
markers, so no heading is flagged.

### Partition invariants

The P1 invariants are unchanged:

- `NoteDocument::source()` is byte-identical to input.
- `verify_partition()` checks union covers `[0, source.len())`
  exactly, ranges pairwise disjoint, no gaps.
- Each `Range` in the partition includes its terminator (where
  any).

The token's `range` field is the byte range in source — no
transformation. The classifier and assembler consume `range`
directly when building `Partition` ranges.

### Mixed terminator handling

Tokens carry their per-line terminator. The classifier treats
all four (`b""`, `b"\n"`, `b"\r\n"`, `b"\r"`) as line terminators.
Mixed terminators in source are parseable.

The strict "one terminator per document" rule is NOT enforced.
This matches the current P1 semantics and is a non-goal; left
for a future proposal if ever needed.

## Rationale

### Why three layers, not two

A two-layer split (tokenizer + everything-else) would still
leave the classifier and partition assembler interleaved at
the boundary. The partition rules (body fragmentation, range
partitioning) need a structural view of the document; the
classifier needs to know "this heading is a tag section," "this
heading is internal to Content," etc. Folding them back
together re-creates the R2-F4 leak.

### Why format-agnostic tokenizer

The tokenizer must not look for `##`/`#`/`=` or for `Tags` text.
If it did, Org/LaTeX/AsciiDoc routers would still need to bypass
it, defeating the purpose. The tokenizer is the most reusable
layer; locking it down to Markdown semantics now forces re-work
later.

This is also a hard rule to fuzz-test: a tokenizer that emits
`Line`s only is round-trip-checkable (no semantic state).

### Why rename `is_inner_body_h2` to `is_section_internal_heading`

The flag is set by the classifier in a way that's specific to
Markdown's `Content`/`Source`/`Tags` section model. Other
formats have no equivalent. Calling it "inner body" presumes
the Bookmark model. `is_section_internal_heading` is
format-neutral: the classifier decides what "section" means.

### Why format dispatch now, not later

The P1 spec is currently silent on non-Markdown formats. It
silently misclassifies `.org` as `Note`. Adding explicit format
dispatch with a typed error is a small, well-scoped improvement
that:

1. Makes the failure mode for unsupported formats clear instead
   of confusing.
2. Sets up the routing infrastructure for when Org/LaTeX/AsciiDoc
   parsers are added in P3+.
3. Fits within the cycle-3 refactor scope.

The actual Org/LaTeX/AsciiDoc parsers are deferred to P3+,
tracked via `nb-api:todos/format/{org-mode-parser,latex-parser,
asciidoc-parser}`.

## Consequences

### Positive

- The R2-F4 leak is structurally fixed: the curator's `flush`
  helper and the body-fragmentation logic no longer reason
  about line terminators.
- The `is_complete_blank_line` check (Todo, R2-F1) is no longer
  needed: the tokenizer has already partitioned the source into
  `Line`s, and the classifier can ask "is this line a blank
  line?" without re-deriving terminator bytes from source.
- Format dispatch is explicit and typed.
- Future format support is additive: a new parser implementation
  slots into the dispatch table without modifying the token/
  partition layers.

### Negative

- Three-layer architecture is more learning surface than the
  current single-file design. Documentation needs to be
  updated.
- The fuzz-test surface expands (tokenizer round-trip + classifier
  property tests).
- Performance is unchanged in the common case (still O(n) forward
  pass), but the boundary adds one indirection. Acceptable per
  operator direction (notes are <200 lines, not a continuous
  service).

### Constraints maintained

- Public API is unchanged: `NoteDocument`, `DocumentKind`,
  `TodoState`, `ParseContext`, all accessors, `verify_partition`.
- Test suite grows but does not shrink: 204 tests pass after
  refactor.
- `verify_partition` invariants are unchanged.
- Permissive acceptance contract is preserved.

## Open questions

- **Q-A**: Should the `Line` struct's `terminator` field be
  `&'a [u8]` (borrowed from source) or `&'static [u8]` (with
  empty as a sentinel)? The current draft is `&'a [u8]`; needs
  confirmation that the iterator lifetime works out.
- **Q-B**: Does the format dispatch error need to be
  distinguishable from `ParseError::MissingTitle`? Currently
  planned as a separate `ParseErrorKind::UnsupportedDocumentFormat`
  variant.
- **Q-C**: The pre-refactor `build_note_partition` returns
  `Result<Partition, NbError>`; should the new
  `build_markdown_partition` (or whatever it's called) return
  `Result<Partition, NbError>` or a typed sub-error that the
  outer dispatch translates?

## Cross-references

- Spec: `nb-api:proposals/add-note-document-model/specifications/note-document-model_specification.md`
- Design: `nb-api:proposals/add-note-document-model/designs/design__note-document-model.md`
- Review: `nb-api:reviews/2` (cycle-2 findings R2-F4, R2-F1, V1)
- Algorithm sketch: `nb-api:reviews/3` (Bookmark double-cursor)
- Follow-up todos: `nb-api:todos/format/{org-mode-parser, latex-parser, asciidoc-parser}`

## Stakeholders

- Operator: Eric McDonald (recording this decision)
- Nbspec Owner: TBD (next review cycle)
- MCP Owner: TBD (next review cycle)

## Status history

- 2026-07-30: Captured from operator consultation.



## Revision 2 (2026-07-30): synchronized with the R3 format-dispatch spec

Per the R3 format-dispatch verdict (`nb-api:reviews/1`,
"R3 format-dispatch specification re-review" section),
several sections of this decision need to be synchronized with
the corrected spec. The original Position 17 text above is
preserved for chronology; the corrections below supersede the
affected clauses.

### Format dispatch — corrected

The original section names `parse_with_context()` (a
nonexistent function) and uses `ParseError::UnsupportedDocumentFormat`
(incorrect placement) and an incomplete supported list. The
corrected spec replaces these with:

- The public API has only `parse(&[u8], ParseContext) -> Result<NoteDocument, NbError>`.
  There is no `parse_with_context()` function.
- The error is `NbError::UnsupportedDocumentFormat` (a top-level
  variant), NOT `ParseError::UnsupportedDocumentFormat`.
- The supported list (per `SUPPORTED_DOCUMENT_EXTENSIONS`) is:
  `md`, `markdown`, `todo.md`, `bookmark.md`.
- The recognized-but-unsupported list is: `org`, `latex`,
  `tex`, `adoc`, `asciidoc`.
- Extension matching uses literal dotted suffix boundaries
  with longest-first precedence against the **final filename**
  (the component after the last `/` or `\` separator).

The corrected spec text is at:
`nb-api:proposals/add-note-document-model/specifications/3`
under `## ADDED Requirements (R3 revision)`.

### Constraints maintained — corrected

The original "Constraints maintained" section states:
> Public API is unchanged: `NoteDocument`, `DocumentKind`,
> `TodoState`, `ParseContext`, all accessors, `verify_partition`.

This is **no longer correct**. The R3 revision adds
`NbError::UnsupportedDocumentFormat` as a new top-level
variant of `NbError`, which is a **public-API breaking change**
for downstream consumers that match `NbError` exhaustively.
The corrected constraint is:

- Public API is **changed** by the addition of
  `NbError::UnsupportedDocumentFormat` and
  `pub const SUPPORTED_DOCUMENT_EXTENSIONS: &[&str]`. All other
  types and accessors are unchanged.

### Open questions — resolution

The original open questions Q-A, Q-B, Q-C are partially or
fully resolved by the R3 work:

- **Q-A (resolved with private types)**: Per the R3 cycle-3
  review clarifications, the proposed `Line` struct is
  `pub(crate)` (private). Borrowed slices are technically
  viable only for transient tokens; the private types keep
  the type-state machine internal.
- **Q-B (resolved)**: The format dispatch error is
  distinguishable from `ParseError::MissingTitle` and is now
  a top-level `NbError::UnsupportedDocumentFormat` variant
  (NOT nested in `ParseError`, NOT a `ParseErrorKind`).
- **Q-C (resolved)**: The format dispatch returns
  `NbError::UnsupportedDocumentFormat` directly. Lower parser
  layers return `NbError` (or a narrow private parse failure
  translated once at the public boundary). The `.org`/`.latex`
  rejection happens before any byte parsing; the byte-parsing
  failure path is unchanged.

### `$SUPPORTED_DOCUMENT_EXTENSIONS` location

The supported-extension list is exposed as a `pub const`
on `parser` (or `DocumentKind`):

```rust
pub const SUPPORTED_DOCUMENT_EXTENSIONS: &[&str] =
    &["md", "markdown", "todo.md", "bookmark.md"];
```

The error variant carries an owned `Vec<String>` populated
from this constant at construction time, so the derived
`Serialize`/`Deserialize` round-trip holds.

### Status history (continued)

- 2026-07-30 (Revision 2): Synchronized with the R3 format-
  dispatch spec. Corrected `parse_with_context()` reference,
  `ParseError::UnsupportedDocumentFormat` placement, supported
  list (.markdown added), recognized-but-unsupported list
  (.tex added), and the "Public API is unchanged" claim. Q-A,
  Q-B, Q-C marked resolved.


## Revision 3 (2026-07-30): R3-D1 architectural clarifications

Per the R3-D1 verdict (`nb-api:reviews/1`, "R3 format-dispatch
correction verification" section), Decision 17 Revision 2 did
not incorporate the cycle-3 architecture approval conditions
already recorded in `nb-api:reviews/2`. Revision 3 adopts those
conditions. The original Position 17 text and Revision 2 are
preserved for chronology; the corrections below supersede the
affected clauses.

### Tokenizer emits physical-line facts plus an explicit BOM/preamble range

The tokenizer is the **preamble-aware** pass. It emits:

- A `Preamble` (BOM plus any leading whitespace) as a single
  range, owned by the tokenizer. The assembler consumes this
  range; it is not re-derived from source bytes.
- One `Line` token per physical line, with `range`, `terminator`,
  and `content` fields. The `Line` type is `pub(crate)` (private
  to the crate); library consumers do not see it.

The intermediate types (`Line`, `Preamble`, `ClassifiedToken`,
`HeadingRole`) are **private** to the crate. The
`NoteDocument::source()` byte-faithful representation is the
only public byte surface.

### Classifier emits per-line semantic roles/facts, not full-section ranges

The classifier consumes `Line` tokens and emits one
`ClassifiedToken` per line. Tokens carry **per-line semantic
roles** but **NOT full-section ranges**. Section extents are
computed later by the kind-specific assembler.

The previously proposed classifier output
`ClassifiedToken::Heading { level, text, full_section }` is
rejected. The corrected token shape is:

```rust
pub(crate) enum ClassifiedToken {
    Preamble(PreambleToken),
    BlankLine(LineId),
    Heading { level: u8, text: String, line_id: LineId },
    TagsPrefix(LineId),
    Url(LineId),
    Body(LineId),
}
```

(`LineId` is an internal opaque index into the tokenizer's
`Line` vector.) Heading tokens carry the heading text and
level only; the assembler computes section extents from these
tokens.

### Kind-specific assembler owns section extents, canonical metadata selection, separator ownership, and final partition ranges

The kind-specific assembler (Note, Todo, Bookmark) is the
sole owner of:

- **Section extents**: each non-Tags H2 section's full range
  (heading + body) is computed by the assembler from the
  classified heading tokens.
- **Canonical Tags selection**: for Bookmark, the assembler
  picks the canonical Tags section from the heading tokens;
  for Todo, the assembler picks the terminal H2 if and only
  if it is Tags.
- **Separator ownership**: blank-line separators between
  metadata regions are emitted by the assembler based on the
  classification.
- **Final partition ranges**: the assembler's output is the
  `Partition` enum consumed by the public `NoteDocument`.

The classifier **does not** compute section extents or select
canonical Tags. The tokenizer **does not** compute separators
or partition ranges.

### Replace bool/kebab-case rule with `HeadingRole` enum and exact Bookmark transitions

The previously proposed boolean flag
`is_section_internal_heading` (with a kebab-case rule) is
rejected. `## Tags in body` is not kebab-case, and a boolean
cannot express "reserved boundary" vs. "opaque body" semantics.

The corrected representation is a `HeadingRole` enum:

```rust
pub(crate) enum HeadingRole {
    SectionBoundary,  // Tags, Content, Source (or any structural heading)
    InternalBody,     // ordinary H2-looking lines inside Content/Source (frozen E10.1)
}
```

The classifier tags each H2 heading with a `HeadingRole`. The
Bookmark classifier applies the following **exact transitions**:

- **Tags** recognized outside fences, **before** any
  Content/Source: `SectionBoundary`. This is the canonical
  metadata Tags section.
- **Content** or **Source** recognized outside fences:
  `SectionBoundary`. The body fragment for this section
  includes the heading + ordinary body lines.
- **Any H2-looking line inside Content** (e.g., frozen
  E10.1's `## Tags in body`): `InternalBody`. The body
  fragment for the Content section absorbs this line.
- **Any H2-looking line inside a fenced Source payload**:
  `InternalBody` (or treated as opaque body). The body
  fragment for the Source section absorbs the entire fenced
  payload including inner H2-looking lines.
- **First H2 Tags after Content/Source** (the sole terminal
  Tags case): The current cycle-2 Bookmark double-cursor
  algorithm handles this case; the heading token is tagged
  `SectionBoundary` so the assembler can recognize it.

Note kind: all H2 headings are `InternalBody` (Note has no
Tags/Content/Source section model). The Todo kind has
section-boundary recognition for terminal Tags only; ordinary
H2 headings inside Todo body are `InternalBody`.

Org/AsciiDoc/LaTeX classifiers (deferred to P3+): the
`HeadingRole` enum is a Markdown-specific concept; future
format classifiers will define their own role taxonomy.

### Resolve Q-C: narrow private parse failure translated once at the public boundary

Q-C was previously unresolved: "Result<Partition, NbError>
(or a typed sub-error that the outer dispatch translates)."
The R3-D1 verdict resolves Q-C as follows:

- **Lower parser layers** (tokenizer, classifier, kind-specific
  assembler) return a **narrow private parse failure** type
  (`ParseFailure` or similar), not `NbError`. This keeps the
  parser's internal vocabulary separate from the public error
  surface.
- **The public boundary** in `parse()` translates the narrow
  private parse failure into the appropriate `NbError` variant
  once. Callers never see the private error type.
- **Format dispatch** may construct the top-level
  `NbError::UnsupportedDocumentFormat` directly **before
  parsing** (the recognized-but-unsupported extensions are
  rejected without invoking any parser layer).
- **Byte-parsing failures** (e.g., empty Todo returning
  `MissingTitle`) are translated to `NbError::ParseError`
  at the public boundary.

The narrow private parse failure type is `pub(crate)` and
not part of the public API.

### Status history (continued)

- 2026-07-30 (Revision 3): R3-D1 architectural clarifications.
  Preamble-aware tokenizer; classifier emits per-line semantic
  roles without full-section ranges; kind-specific assembler
  owns section extents, canonical metadata selection, separator
  ownership, and final partition ranges; `HeadingRole` enum
  replaces the boolean/kebab-case rule; exact Bookmark
  transitions stated; Q-C resolved to a narrow private parse
  failure translated once at the public boundary.


## Revision 4 (2026-07-30): R3-D2/D3/D4 consistency edits

Per the R3 final corrections verification verdict
(`nb-api:reviews/1`, "R3 final corrections verification" section),
the corrected R3 format-dispatch specification is accepted and
S7/S8 are closed. Decision 17 Revision 3 needs three direct
consistency edits before the combined gate can be approved.
The original Position 17 text, Revision 2, and Revision 3 are
preserved for chronology; the corrections below supersede the
affected clauses.

### R3-D2 (High): Preamble ownership violates the approved partition

Revision 3 defined one `Preamble` range as "BOM plus any leading
whitespace." This conflicts with the P1 partition invariant:
`prefix_range` owns the UTF-8 BOM only; leading blank lines
belong to `separator_ranges`; spaces on a nonblank first line
belong to that line / title / body. Combining them into one
intermediate range prevents the assembler from preserving
those ownership distinctions without re-splitting source bytes,
which the architecture forbids.

The corrected preamble shape is **BOM-only**:

- The tokenizer emits an optional BOM-only preamble range
  (zero bytes if no BOM, three bytes if UTF-8 BOM is present).
- Every byte after the BOM, including leading blank lines and
  leading spaces on a content line, remains in ordinary `Line`
  tokens emitted by the tokenizer.
- The kind-specific assembler maps the BOM range to
  `prefix_range` and classifies leading blank `Line` tokens as
  separators per the existing P1 partition invariant.

The `Preamble` intermediate is the BOM-only span. The
assembler consumes it once and emits `prefix_range`. The
tokenizer does not collapse leading blank lines or leading
spaces into the preamble.

### R3-D3 (High): proposed Heading token does not carry `HeadingRole`

Revision 3 stated the classifier tags each H2 with
`HeadingRole`, but the proposed token shape was
`Heading { level, text, line_id }` without a `role` field.
The assembler therefore cannot consume the classification
without an unspecified side table.

The corrected token shape carries the role explicitly:

```rust
pub(crate) enum ClassifiedToken {
    Preamble(PreambleToken),         // BOM-only span (D2)
    BlankLine(LineId),
    Heading { level: u8, text: String, line_id: LineId, role: HeadingRole },
    TagsPrefix(LineId),
    Url(LineId),
    Body(LineId),
}

pub(crate) enum HeadingRole {
    SectionBoundary,  // Tags, Content, Source (or any structural candidate)
    InternalBody,     // ordinary H2-looking lines inside Content (frozen E10.1)
}
```

The classifier attaches `role: HeadingRole` to every Heading
token. The assembler reads the role directly from the token —
no side table is required.

Alternative representation (for future consideration): the
classifier could emit internal headings as `Body(LineId)` and
reserve Heading tokens for structural candidates only. The
current corrected shape carries the role explicitly so the
assembler has full information; the Body-only alternative is
left as a future optimization if the role field proves
unnecessary.

### R3-D4 (High): Bookmark transitions still classify exact terminal Tags both ways

Revision 3 said "Any H2-looking line inside Content" is
`InternalBody`, while also saying "First H2 Tags after
Content/Source" is `SectionBoundary`. Markdown Content has no
explicit closing delimiter, so those statements overlap for an
exact `## Tags` line and recreate R3-F1.

The corrected deterministic lexical state machine is specified
without canonical selection in the classifier:

- **Outside fences**: exact reserved `## Tags`, `## Content`,
  and `## Source` lines are `SectionBoundary` candidates. This
  includes exact terminal Tags after Content/Source; the
  classifier does not know which Tags is "the canonical" one.
- **Inside Content**: non-reserved H2-looking lines such as
  `## Tags in body` are `InternalBody`. An exact reserved
  `## Tags` inside Content (an unusual but possible case) is
  `SectionBoundary` candidate; the assembler then decides
  whether to treat it as a metadata Tags or as content.
- **Inside a fenced Source payload**: every H2-looking line is
  opaque body (the classifier emits `Body(LineId)` for these,
  not Heading); the Source payload is treated as a single body
  fragment.
- **The assembler, not the classifier, chooses canonical Tags**
  among the reserved boundary candidates. The classifier
  surfaces all candidates; the assembler applies the canonical-
  selection algorithm (first before Content/Source, else last).

The same separation applies to Todo:

- The classifier marks exact Tags headings as structural
  candidates (`SectionBoundary`).
- The assembler alone decides whether the final H2 is the
  terminal metadata Tags section. (For Todo, the assembler
  sets `tag_section_range` only when the final H2 itself is
  Tags; otherwise the only Tags candidates are body.)

This preserves the Revision 3 rule that canonical metadata
selection belongs solely to the assembler. The classifier is
purely lexical / structural-recognition; the assembler owns
canonical selection.

### Status history (continued)

- 2026-07-30 (Revision 4): R3-D2/D3/D4 consistency edits.
  Preamble is BOM-only (D2); Heading token carries `role:
  HeadingRole` explicitly (D3); Bookmark/Todo classifier emits
  boundary candidates only — no canonical selection in the
  classifier; the assembler alone picks canonical Tags (D4).

## Status history

- 2026-07-30: Captured from operator consultation.
- 2026-07-30 (Revision 2): Synchronized with the R3 format-
  dispatch spec. Corrected `parse_with_context()` reference,
  `ParseError::UnsupportedDocumentFormat` placement, supported
  list (.markdown added), recognized-but-unsupported list
  (.tex added), and the "Public API is unchanged" claim. Q-A,
  Q-B, Q-C marked resolved.
- 2026-07-30 (Revision 3): R3-D1 architectural clarifications.
  Preamble-aware tokenizer; classifier emits per-line semantic
  roles without full-section ranges; kind-specific assembler
  owns section extents, canonical metadata selection, separator
  ownership, and final partition ranges; `HeadingRole` enum
  replaces the boolean/kebab-case rule; exact Bookmark
  transitions stated; Q-C resolved to a narrow private parse
  failure translated once at the public boundary.
- 2026-07-30 (Revision 4): R3-D2/D3/D4 consistency edits.
  Preamble is BOM-only (D2); Heading token carries `role:
  HeadingRole` explicitly (D3); Bookmark/Todo classifier emits
  boundary candidates only — no canonical selection in the
  classifier; the assembler alone picks canonical Tags (D4).
- 2026-07-30 (FINAL): R3 combined gate **APPROVED**. Decision 17
  Revision 4 passes the frozen five-point closure checklist.
  Approval recorded in `nb-api:reviews/1`. Combined R3
  specification/architecture gate is open. Implementation
  chain proceeds per `nb-api:coordination/general/5`.
