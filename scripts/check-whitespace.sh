#!/usr/bin/env bash
#
# scripts/check-whitespace.sh
#
# Two-pass endpoint-wise whitespace validator for the full-stack gate.
#
# Rationale
# ---------
# The P1 note-document-model fixtures under
# `tests/integration/fixtures/*.md` are exact-byte writer output from
# pinned `nb 7.24.0`. The trailing blank line at EOF is intentional
# `nb` writer output (a final `\n\n` after the body), NOT a formatting
# defect, and the round-trip identity property depends on those bytes
# remaining untouched.
#
# `git diff --check` flags the intentional EOF blank lines as
# `blank-at-eof` errors. The `.gitattributes` `whitespace` attribute
# is documented to silence `git apply` and `git diff` for
# working-tree/index/staged comparisons, but does NOT silence
# `git diff --check` between two commits. Empirical testing confirmed
# this: even with the corrected `.gitattributes` committed at HEAD,
# `git diff --check dbb857d..HEAD` still reports the EOF blanks.
#
# This script is the narrow replacement. It runs the full repository
# under the default whitespace policy, then exempts the fixtures only
# from the `blank-at-eof` check via a per-invocation
# `core.whitespace=-blank-at-eof` override. All other whitespace
# errors in the fixtures (trailing-space, blank-at-eol, etc.) still
# fail.
#
# Two-pass design (always both passes)
# ------------------------------------
# Both passes run regardless of whether the fixtures directory is
# present in the current checkout. The historical endpoints are the
# authority; a quoted unmatched pathspec succeeds harmlessly. Pass 1
# covers the whole tree except fixtures; Pass 2 covers fixtures only
# with `blank-at-eof` disabled. All `git` invocations run through
# `git -C "$repo_root"` and use top-anchored pathspecs (`:(top)`) so
# the behavior is invariant under caller-subtree invocation.
#
# Usage
# -----
#   scripts/check-whitespace.sh <base>..<head>   # one revision range
#   scripts/check-whitespace.sh <base> <head>    # two revisions
#
# Exit codes
# ----------
#   0  both passes succeeded
#   1  one or both passes failed (whitespace errors detected)
#   2  invalid arguments (bad usage, bad revision, empty/three-dot/option-shaped rev)
#   3  plumbing error (git could not run the diff even after rev validation)
#
# See `nb-api:reviews/2` and
# `nb-api:coordination/general/5` for the originating decisions.

set -eu

# --- argument parsing -----------------------------------------------

if [[ $# -eq 1 ]]; then
    base_ref="${1%..*}"
    head_ref="${1#*..}"
    if [[ "$base_ref" == "$1" || "$head_ref" == "$1" || "$1" == *"..."* ]]; then
        echo "error: argument '$1' is not a <base>..<head> range" >&2
        echo "usage: $0 <base>..<head>" >&2
        echo "       $0 <base> <head>" >&2
        exit 2
    fi
elif [[ $# -eq 2 ]]; then
    base_ref="$1"
    head_ref="$2"
    if [[ "$1" == *".."* || "$2" == *".."* || "$1" == *"..."* || "$2" == *"..."* ]]; then
        echo "error: two-arg form rejects range / three-dot inputs" >&2
        echo "usage: $0 <base>..<head>" >&2
        echo "       $0 <base> <head>" >&2
        exit 2
    fi
else
    echo "usage: $0 <base>..<head>" >&2
    echo "       $0 <base> <head>" >&2
    exit 2
fi

# --- repository resolution ------------------------------------------

# Find the repository root from wherever the caller invoked
# the script. Use `git -C` so subsequent git invocations are
# pinned to that root, not the caller's current directory.
if ! repo_root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
    echo "error: not a git repository (or no .git in any parent)" >&2
    exit 3
fi
if [[ ! -d "$repo_root/.git" ]] && [[ ! -f "$repo_root/.git" ]]; then
    echo "error: not a git repository (no .git at $repo_root)" >&2
    exit 3
fi

# --- revision validation ---------------------------------------------

# Validate that base resolves to a commit, OR is the literal
# empty-tree SHA (which we accept as a special base for
# initial-commit / fresh-history pushes). Arbitrary tree
# objects (e.g., a non-empty subtree) are NOT accepted as
# bases — only commits and the literal empty tree qualify.
empty_tree_sha="4b825dc642cb6eb9a060e54bf8d69288fbee4904"

if ! git -C "$repo_root" rev-parse --verify --quiet "${base_ref}^{commit}" >/dev/null 2>&1; then
    # Base is not a commit. Only the literal empty-tree SHA is
    # accepted as a tree base; reject everything else.
    if [[ "$base_ref" != "$empty_tree_sha" ]]; then
        echo "error: base revision '$base_ref' is not a valid commit; only the literal empty-tree SHA is accepted as a tree base" >&2
        exit 2
    fi
    # Even when the base is the literal empty-tree SHA,
    # verify it resolves to a tree (defensive: catches SHA
    # typos and library drift).
    if ! git -C "$repo_root" rev-parse --verify --quiet "${base_ref}^{tree}" >/dev/null 2>&1; then
        echo "error: literal empty-tree SHA '$base_ref' does not resolve to a tree" >&2
        exit 2
    fi
fi
if ! git -C "$repo_root" rev-parse --verify --quiet "${head_ref}^{commit}" >/dev/null 2>&1; then
    echo "error: head revision '$head_ref' is not a valid commit" >&2
    exit 2
fi

range="${base_ref}..${head_ref}"

# --- run passes ------------------------------------------------------
# Always run both passes. Each pass uses `git -C "$repo_root"`
# and top-anchored pathspecs so behavior is invariant under
# caller-subtree invocation.

failures=0
plumbing_errors=0

run_pass() {
    local label_text="$1"
    shift
    echo "==> $label_text"
    local rc
    local outfile
    outfile="$(mktemp)"
    set +e
    "$@" >"$outfile" 2>&1
    rc=$?
    set -e
    if [[ $rc -eq 0 ]]; then
        echo "  OK"
    elif grep -qE '^(fatal|error|BUG): ' "$outfile"; then
        # Plumbing-level failure: git could not perform the
        # diff. This is distinct from a whitespace error
        # (where git completes the diff but reports findings).
        echo "  FAIL (plumbing error, exit $rc)"
        plumbing_errors=$(( plumbing_errors + 1 ))
    else
        # Any other non-zero exit is treated as a whitespace
        # finding. Different git versions return different
        # exit codes for whitespace (1, 2, 128+signal); the
        # stderr-marker heuristic is more robust than a hard
        # exit-code match.
        echo "  FAIL (whitespace errors detected)"
        failures=$(( failures + 1 ))
    fi
    rm -f "$outfile"
    echo
}

# Pass 1: full repository except exact-byte fixtures.
# The pathspec is QUOTED; the magic
# `:(exclude,top)tests/integration/fixtures/*.md` is consumed by
# git itself, not the shell. `:(top)` anchors the exclude
# pattern to the repo root.
run_pass "Pass 1: full repository (except fixtures)" \
    git -C "$repo_root" diff --check "$range" \
    -- . ':(exclude,top)tests/integration/fixtures/*.md'

# Pass 2: fixtures only with blank-at-eof disabled. Other
# whitespace errors (trailing-space, blank-at-eol, etc.)
# still fail. The `:(top)` magic anchors the pathspec to
# the repo root; the glob is QUOTED to prevent shell
# expansion.
run_pass "Pass 2: fixtures only (blank-at-eof disabled)" \
    git -C "$repo_root" -c core.whitespace=-blank-at-eof \
    diff --check "$range" -- ':(top)tests/integration/fixtures/*.md'

if [[ "$plumbing_errors" -gt 0 ]]; then
    echo "Whitespace gate: PLUMBING ERROR ($plumbing_errors pass(es) had plumbing errors)"
    exit 3
fi
if [[ "$failures" -gt 0 ]]; then
    echo "Whitespace gate: FAIL ($failures pass(es) failed)"
    exit 1
fi

echo "Whitespace gate: OK"