# Project Guidance

Project-owned knowledge the generated `AGENTS.md` entrypoint must not own.
Structured tracking stays in `nb`.

## Purpose

Typed Rust interface to the nb note-taking CLI: programmatic client
wrapping `nb` as a subprocess with argument escaping, notebook
qualification, output parsing, and error recovery. Consumers:
`nb-mcp-server`, `nbspec`, and any Rust application driving `nb`.

## Tech Stack

Rust (2024 edition), tokio, `nb` CLI subprocess. Text-first wire
surface; BLAKE3 fingerprints; `.index` numeric identity.

## Notes

- Release rule: CI must be green before tagging. (Bundle-wide,
  operator-endorsed 2026-11-10; closes `todos/api/11`.)
- Rolling handoff and team organization:
  `nb-api:coordination/general/5`.
- This project uses Nbspec (notebook-resident change orchestration),
  not vanilla filesystem OpenSpec. Always open
  `documentation/agents/nbspec.md` when a request mentions planning
  or proposals (words like proposal, spec, change, plan), introduces
  new capabilities, breaking changes, or architecture shifts, or
  sounds ambiguous enough to need the authoritative spec before
  coding. The template `procedures/` docs use generic OpenSpec
  wording; the Nbspec guide is authoritative here.
- Deviations
  - **Whitespace gate** is `scripts/check-whitespace.sh`, not
    `.gitattributes`. The `whitespace` attribute silences `git apply`
    and `git diff` for working-tree/index/staged comparisons, but
    does NOT silence `git diff --check` between two commits.
    Empirical commit-to-commit testing invalidated the `.gitattributes`
    exemption; the two-pass path-scoped script is the narrow
    replacement. See `scripts/README.md` and the embedded rationale
    in the script.
