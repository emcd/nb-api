# scripts/

Repository-level execution scripts.

## check-whitespace.sh

Two-pass path-scoped whitespace validator for the full-stack
gate. This is the documented replacement for the
previously-incorrect `.gitattributes`-based exemption.

The full repository is checked under the default whitespace
policy, except the exact-byte `nb 7.24.0` writer-output
fixtures under `tests/integration/fixtures/*.md`, which are
exempted only from the `blank-at-eof` check via a per-invocation
`core.whitespace=-blank-at-eof` override. All other whitespace
errors in the fixtures (trailing-space, blank-at-eol, etc.) still
fail.

Runs in CI as the `whitespace-gate` step of the `lint` job.

### Usage

```sh
scripts/check-whitespace.sh <base>..<head>   # one revision range
scripts/check-whitespace.sh <base> <head>    # two revisions
```

### Exit codes

- `0` — both passes succeeded
- `1` — one or both passes failed
- `2` — invalid arguments
- `3` — git plumbing error

### Rationale

The P1 note-document-model fixtures under
`tests/integration/fixtures/*.md` are exact-byte writer output
from pinned `nb 7.24.0`. The trailing blank line at EOF is
intentional `nb` writer output (a final `\n\n` after the body),
NOT a formatting defect, and the round-trip identity property
depends on those bytes remaining untouched.

`git diff --check` flags the intentional EOF blank lines as
`blank-at-eof` errors. The `.gitattributes` `whitespace`
attribute is documented to silence `git apply` and `git diff`
for working-tree/index/staged comparisons, but does NOT silence
`git diff --check` between two commits. Empirical testing
confirmed this: even with the corrected `.gitattributes`
committed at HEAD, `git diff --check <base>..<head>` still
reports the EOF blanks.

This script is the narrow replacement for commit-to-commit
whitespace checking where `.gitattributes` is ineffective.

## tests/check-whitespace-test.sh

Small shell integration test that exercises the script's
argument contract, plumbing, and two-pass behavior end-to-end.
Run with:

```sh
bash scripts/tests/check-whitespace-test.sh
```

This is a plain `bash` script (not a Cargo test binary), so it
does not require widening crate visibility for whitebox access.
It:

- Verifies the argument contract: no-args and bad-arg both
  exit 2 with a usage message.
- Verifies revision validation: bad base or head revisions
  exit 2 with a clear error.
- Verifies the happy path: `<base>..<head>` and two-arg form
  both succeed against a clean range.
- Verifies caller-subtree resilience: invoking from `scripts/`
  still finds `tests/integration/fixtures/*.md` (regression
  test for the previous "skips fixtures from scripts/" bug).
- Verifies the fixture-EOF-blanks exemption: a range that
  DELIBERATELY has the EOF blank lines passes.
