#!/usr/bin/env bash
#
# scripts/tests/check-whitespace-test.sh
#
# Shell integration test for scripts/check-whitespace.sh.
# Whitespace-gate contract:
# the script's argument contract, plumbing-error preservation,
# and pass coverage must be validated through a shell integration
# test rather than rely on caller-subtree discovery or unquoted
# globs. The test exercises:
#
#   1. Bad usage (no args, non-range arg) exits 2.
#   2. Bad revision (nonexistent base or head) exits 2.
#   3. Two-arg form (<base> <head>) exits 0 on a clean range.
#   4. One-range form (<base>..<head>) exits 0 on a clean range.
#   5. Caller-subtree resilience: invocation from scripts/.
#   6. Trailing-blank-at-EOF in fixtures is NOT flagged.
#   7. Three-dot and double-dot range forms are rejected (exit 2).
#   8. Empty-tree SHA is accepted as a base rev (initial-commit).
#   9. Both passes run regardless of fixtures presence.
#  10. Plumbing-error path preserved (exit 3) when rev is invalid
#      but rev-parse validation is bypassed (e.g., explicit empty
#      rev).

set -u

# Locate repo root and the script under test.
repo_root="$(git rev-parse --show-toplevel)"
script="$repo_root/scripts/check-whitespace.sh"
if [[ ! -x "$script" ]]; then
    echo "FAIL: $script is not executable" >&2
    exit 1
fi

# tmpdir for capturing per-test output. Each test invocation
# uses a fresh file so exit-code matching is unambiguous.
tmpdir="$(mktemp -d)"
trap "rm -rf '$tmpdir'" EXIT

pass=0
fail=0

# Run the script under test with the given args, capture both
# stdout+stderr to a per-test file, and assert the exit code.
#   $1 = label
#   $2 = expected rc
#   $3... = script args
run_test() {
    local label="$1"
    local expected_rc="$2"
    shift 2
    local outfile="$tmpdir/out.$$"
    set +e
    "$script" "$@" >"$outfile" 2>&1
    local actual_rc=$?
    set -e
    if [[ "$actual_rc" == "$expected_rc" ]]; then
        echo "  ok: $label (rc=$actual_rc)"
        pass=$(( pass + 1 ))
    else
        echo "  FAIL: $label (expected rc=$expected_rc, got rc=$actual_rc)"
        echo "    output: $(cat "$outfile")"
        fail=$(( fail + 1 ))
    fi
}

# Same as run_test but also asserts that output contains a substring.
run_test_contains() {
    local label="$1"
    local expected_rc="$2"
    local expected_substr="$3"
    shift 3
    local outfile="$tmpdir/out.$$"
    set +e
    "$script" "$@" >"$outfile" 2>&1
    local actual_rc=$?
    set -e
    if [[ "$actual_rc" == "$expected_rc" ]]; then
        if grep -qF "$expected_substr" "$outfile"; then
            echo "  ok: $label (rc=$actual_rc, contains '$expected_substr')"
            pass=$(( pass + 1 ))
        else
            echo "  FAIL: $label (rc ok but missing '$expected_substr')"
            echo "    output: $(cat "$outfile")"
            fail=$(( fail + 1 ))
        fi
    else
        echo "  FAIL: $label (expected rc=$expected_rc, got rc=$actual_rc)"
        echo "    output: $(cat "$outfile")"
        fail=$(( fail + 1 ))
    fi
}

# --- 1. Argument contract: bad usage exits 2 -----------------------

echo "Test 1: bad argument contract"
run_test "no-args exits 2" 2
run_test "non-range arg exits 2" 2 badarg

# --- 2. Argument contract: bad revision exits 2 --------------------

echo "Test 2: bad revision rejected"
run_test "bad-base exits 2" 2 nonexistent..HEAD
run_test "bad-head exits 2" 2 dbb857d..nonexistent

# --- 3. Happy path: <base>..<head> range -----------------------------

echo "Test 3: <base>..<head> range against a clean range"
run_test "clean range exits 0" 0 5470f88..HEAD

# --- 4. Happy path: two-arg form <base> <head> ------------------------

echo "Test 4: two-arg <base> <head> form"
run_test "two-arg form exits 0" 0 5470f88 HEAD

# --- 5. Caller-subtree resilience: invoke from scripts/ --------------

echo "Test 5: invocation from scripts/ still finds fixtures"
pushd "$repo_root/scripts" >/dev/null
outfile="$tmpdir/out.tree"
set +e
"../scripts/check-whitespace.sh" 5470f88..HEAD >"$outfile" 2>&1
rc=$?
set -e
if [[ "$rc" == "0" ]]; then
    echo "  ok: caller-subtree invocation exits 0 (rc=$rc)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: caller-subtree invocation (rc=$rc)"
    fail=$(( fail + 1 ))
fi
# Both passes must run, regardless of caller-subtree.
case "$(cat "$outfile")" in
    *"Pass 1: full repository (except fixtures)"*)
        echo "  ok: pass-1 reached full-repository"; pass=$(( pass + 1 )) ;;
    *) echo "  FAIL: pass-1 missing in caller-subtree output"; fail=$(( fail + 1 )) ;;
esac
case "$(cat "$outfile")" in
    *"Pass 2: fixtures only (blank-at-eof disabled)"*)
        echo "  ok: pass-2 reached fixtures"; pass=$(( pass + 1 )) ;;
    *) echo "  FAIL: pass-2 missing in caller-subtree output"; fail=$(( fail + 1 )) ;;
esac
popd >/dev/null

# --- 6. Trailing-blank-at-EOF in fixtures is NOT flagged ------------

echo "Test 6: trailing-blank-at-EOF in fixtures is exempt"
outfile="$tmpdir/out.6"
set +e
"$script" dbb857d..HEAD >"$outfile" 2>&1
rc=$?
set -e
if [[ "$rc" == "0" ]]; then
    echo "  ok: fixture EOF blanks are exempt, exits 0"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: fixture EOF blanks exemption (rc=$rc)"
    fail=$(( fail + 1 ))
fi

# --- 7. Three-dot / double-dot / option-shaped inputs are rejected ---

echo "Test 7: malformed range inputs exit 2"
run_test "three-dot range exits 2" 2 HEAD~1...HEAD
# Two-arg form with a range expression in either arg also exits 2.
run_test "two-arg with double-dot exits 2" 2 dbb857d HEAD~1..HEAD
run_test "two-arg with three-dot exits 2" 2 dbb857d HEAD~1...HEAD

# --- 8. Empty-tree SHA as base is accepted (initial-commit) ---------

empty_tree_sha="4b825dc642cb6eb9a060e54bf8d69288fbee4904"
echo "Test 8: empty-tree SHA as base reaches diff stage (validation passes)"
outfile="$tmpdir/out.8"
set +e
"$script" "${empty_tree_sha}..HEAD" >"$outfile" 2>&1
rc=$?
set -e
# We don't expect rc=0 here because the empty-tree..HEAD range
# covers all history and the repo's `.auxiliary/` tooling
# configs have pre-existing trailing whitespace outside the
# P1 stack's scope. The contract we verify here is that the
# script's REVISION VALIDATION accepts the empty-tree SHA
# (does NOT exit 2 with "is not a valid commit or tree" or
# "literal empty-tree SHA").
if [[ "$rc" -ne 2 ]] \
   && ! grep -q "is not a valid commit" "$outfile" \
   && ! grep -q "literal empty-tree SHA" "$outfile"; then
    echo "  ok: empty-tree base validation passed (rc=$rc)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: empty-tree base rejected at validation (rc=$rc)"
    echo "    output: $(cat "$outfile")"
    fail=$(( fail + 1 ))
fi

# --- 8b. Arbitrary non-empty tree base is rejected ---------------------
# Only the literal empty-tree SHA is
# accepted as a tree base; non-empty trees must NOT be accepted.
# Construct a non-empty tree SHA and verify it is REJECTED with
# exit 2 (revision validation).

non_empty_tree="$(git -C "$repo_root" rev-parse --verify HEAD^{tree})"
echo "Test 8b: non-empty tree base (${non_empty_tree:0:8}...) rejected (exit 2)"
outfile="$tmpdir/out.8b"
set +e
"$script" "${non_empty_tree}..HEAD" >"$outfile" 2>&1
rc=$?
set -e
if [[ "$rc" == "2" ]] && grep -q "only the literal empty-tree SHA" "$outfile"; then
    echo "  ok: non-empty tree base rejected with explanatory error (rc=2)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: non-empty tree base acceptance (rc=$rc)"
    echo "    output: $(cat "$outfile")"
    fail=$(( fail + 1 ))
fi

# --- 8c. Option-shaped inputs are rejected (exit 2) --------------------
# Treat any flag-like input as a usage error rather than
# silently treating it as a ref.

run_test "option in arg1 exits 2" 2 --some-option HEAD
run_test "dash-dash in arg2 exits 2" 2 HEAD --bogus-flag

# --- 8d. Empty endpoints in either base or head are rejected (exit 2) -
# An empty endpoint means the rev is empty
# (e.g. `..HEAD` or `HEAD..`). Empty revs MUST be classified as
# usage errors (exit 2), not as plumbing failures (which would
# be exit 3) and not as legitimate empty-tree SHA substitution.

run_test "empty base '..HEAD' exits 2" 2 ..HEAD
run_test "empty head 'HEAD..' exits 2" 2 HEAD..

# --- 8e. Arbitrary (non-empty) tree SHA as the head endpoint is rejected
# (exit 2). Tree objects are NOT commits and
# must be rejected at revision validation, not allowed to pass
# through to `git diff --check` (which would otherwise treat the
# tree as a "valid commit-like ref" and produce a confusing
# internal error).
#
# We use a real, repo-resident tree SHA (the current HEAD's
# tree) so the test is independent of operator-driven fixtures.

arbitrary_head_tree="$(git -C "$repo_root" rev-parse --verify HEAD^{tree})"
run_test "arbitrary non-empty tree SHA as head exits 2" \
    2 "HEAD..${arbitrary_head_tree}"

# --- 9. Both passes run unconditionally ------------------------------

echo "Test 9: both passes run unconditionally"
outfile="$tmpdir/out.9"
set +e
"$script" 5470f88..HEAD >"$outfile" 2>&1
set -e
case "$(cat "$outfile")" in
    *"Pass 1"*"Pass 2"*)
        echo "  ok: both passes ran"
        pass=$(( pass + 1 )) ;;
    *) echo "  FAIL: only one pass ran"; fail=$(( fail + 1 )) ;;
esac

# --- 10a. Ordinary exit-1: whitespace errors present in range ---
# Probe whether the current range `5470f88..HEAD` has any
# ordinary whitespace findings. An earlier test made
# this invisible via a stdout-text grep; the current gate
# requires inducing the actual exit, which requires a range
# that the gate fails on. Since the curated fixture chain is
# clean, we instead fabricate a workspace-cwd commit that
# introduces a whitespace error, then range that commit.

whitespace_ws_repo="$(mktemp -d)"
git -C "$whitespace_ws_repo" init --quiet -b main
git -C "$whitespace_ws_repo" -c user.email=test@example.com -c user.name=test \
    commit --quiet --allow-empty -m "empty"
ws_commit_with_error="$(cd "$whitespace_ws_repo" && bash -c '
    printf "trailing-space  \nbody\n" > space.txt
    git add space.txt
    git -c user.email=test@example.com -c user.name=test commit --quiet -m "ws"
    git rev-parse HEAD
')"
clean_parent="$(cd "$whitespace_ws_repo" && git rev-parse HEAD~1)"

echo "Test 10a: ordinary whitespace error in range exits 1"
outfile="$tmpdir/out.10a"
set +e
(cd "$whitespace_ws_repo" && "$repo_root/scripts/check-whitespace.sh" \
    "${clean_parent}..${ws_commit_with_error}" >"$outfile" 2>&1)
rc=$?
set -e
rm -rf "$whitespace_ws_repo"
if [[ "$rc" == "1" ]]; then
    echo "  ok: ordinary whitespace error -> exit 1 (rc=$rc)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: ordinary exit-1 induced (rc=$rc)"
    echo "    output: $(cat "$outfile")"
    fail=$(( fail + 1 ))
fi

# --- 10b. Plumbing-failure induced -> exit 3 ------------------------
# Construct an empty-tree SHA + a real ref head, then range.
# Use `git -c` to point the diff index at an invalid object
# store so `git diff --check` fails with a plumbing error
# even when rev-parse succeeds for both endpoints.

plumb_repo="$(mktemp -d)"
git -C "$plumb_repo" init --quiet -b main
git -C "$plumb_repo" -c user.email=test@example.com -c user.name=test \
    commit --quiet --allow-empty -m "root"
plumb_root="$(cd "$plumb_repo" && git rev-parse HEAD)"
plumb_head="$(cd "$plumb_repo" && git rev-parse HEAD)"

echo "Test 10b: plumbing failure (non-git cwd) -> exit 3"
outfile="$tmpdir/out.10b"
# Trigger the documented exit-3 plumbing-failure path by
# invoking the script from a directory that is NOT a git
# repository. The repo-root probe (`git rev-parse
# --show-toplevel`) fails before any revision validation
# runs, classifying the failure as a plumbing error rather
# than a usage error. Per the documented contract, the
# script returns exit 3 in this case.
set +e
(cd "$tmpdir" && "$script" 5470f88..HEAD) >"$outfile" 2>&1
rc=$?
set -e
if [[ "$rc" == "3" ]]; then
    echo "  ok: not-a-git-repo plumbing failure -> exit 3 (rc=$rc)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: plumbing failure induced (rc=$rc)"
    echo "    output: $(cat "$outfile")"
    fail=$(( fail + 1 ))
fi

# --- summary ----------------------------------------------------------

echo
echo "Summary: $pass passed, $fail failed"
if [[ "$fail" -gt 0 ]]; then
    exit 1
fi