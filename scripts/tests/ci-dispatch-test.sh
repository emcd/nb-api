#!/usr/bin/env bash
#
# scripts/tests/ci-dispatch-test.sh
#
# Shell integration test for scripts/lib/ci-dispatch.sh.
# Per cycle-4b verdict C4B-V1-1, the dispatch library must be
# exercised through each event kind and edge case without
# relying on a live GitHub Actions runner.
#
# Tests:
#   1. pull_request event uses PR_BASE_SHA.
#   2. push event uses PUSH_BEFORE.
#   3. push event with all-zero PUSH_BEFORE uses empty-tree SHA.
#   4. push event with missing PUSH_BEFORE uses empty-tree SHA.
#   5. workflow_dispatch with DISPATCH_BASE uses DISPATCH_BASE.
#   6. workflow_dispatch without DISPATCH_BASE and with HEAD^ uses HEAD^.
#   7. workflow_dispatch without DISPATCH_BASE and without HEAD^ uses empty-tree SHA.
#   8. unsupported event exits 2.

set -u

# Locate repo root and the dispatch library under test.
repo_root="$(git rev-parse --show-toplevel)"
lib="$repo_root/scripts/lib/ci-dispatch.sh"
if [[ ! -r "$lib" ]]; then
    echo "FAIL: $lib is not readable" >&2
    exit 1
fi

# Each test sources the library fresh and runs dispatch_base_sha
# in a subshell with the documented env vars set. We capture the
# exit code and stdout independently.
tmpdir="$(mktemp -d)"
trap "rm -rf '$tmpdir'" EXIT

pass=0
fail=0

run_case() {
    local label="$1"
    local expected_rc="$2"
    local expected_base="$3"
    shift 3
    local outfile="$tmpdir/out.$$"
    local rcfile="$tmpdir/rc.$$"
    (
        # shellcheck disable=SC1090
        source "$lib"
        # Apply the test's env vars by overriding the function's
        # input contract. Use the documented variable names.
        "$@" >"$outfile" 2>&1
        echo $? >"$rcfile"
        dispatch_base_sha
    ) >"$tmpdir/out2.$$" 2>&1
    # The dispatch_base_sha output is what we actually want;
    # the `"$@" >"$outfile"` invocation is just for env setup.
    # Re-run with the right env to capture dispatch output.
    local actual_rc actual_base
    actual_rc="$(cat "$rcfile")"
    actual_base="$(cat "$tmpdir/out2.$$")"
    if [[ "$actual_rc" == "$expected_rc" ]] \
       && { [[ -z "$expected_base" ]] || [[ "$actual_base" == "$expected_base" ]]; }; then
        echo "  ok: $label (rc=$actual_rc, base=$actual_base)"
        pass=$(( pass + 1 ))
    else
        echo "  FAIL: $label (expected rc=$expected_rc base=$expected_base, got rc=$actual_rc base=$actual_base)"
        echo "    output: $(cat "$outfile") $(cat "$tmpdir/out2.$$")"
        fail=$(( fail + 1 ))
    fi
}

# Helper: set env vars in the current shell, then call
# dispatch_base_sha. Returns via stdout.
with_env() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            *=*) export "$1"; shift ;;
            *) break ;;
        esac
    done
    dispatch_base_sha
}

# --- 1. pull_request event ------------------------------------------------
# Use a real commit SHA from the current repo so the validation
# step in dispatch_base_sha can resolve it.

real_sha="$(git rev-parse --verify HEAD)"

echo "Test 1: pull_request event uses PR_BASE_SHA"
(
    unset GITHUB_EVENT_NAME PUSH_BEFORE DISPATCH_BASE
    export GITHUB_EVENT_NAME="pull_request"
    export PR_BASE_SHA="$real_sha"
    source "$lib"
    with_env >"$tmpdir/t1.out" 2>&1
    echo $? >"$tmpdir/t1.rc"
)
actual="$(cat "$tmpdir/t1.out")"
rc="$(cat "$tmpdir/t1.rc")"
if [[ "$rc" == "0" && "$actual" == "$real_sha" ]]; then
    echo "  ok: pull_request uses PR_BASE_SHA (rc=0, base=$actual)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: pull_request (rc=$rc, base=$actual)"
    fail=$(( fail + 1 ))
fi

# --- 2. push event uses PUSH_BEFORE ---------------------------------------

echo "Test 2: push event uses PUSH_BEFORE"
(
    unset PR_BASE_SHA DISPATCH_BASE
    export GITHUB_EVENT_NAME="push"
    export PUSH_BEFORE="$real_sha"
    source "$lib"
    with_env >"$tmpdir/t2.out" 2>&1
    echo $? >"$tmpdir/t2.rc"
)
actual="$(cat "$tmpdir/t2.out")"
rc="$(cat "$tmpdir/t2.rc")"
if [[ "$rc" == "0" && "$actual" == "$real_sha" ]]; then
    echo "  ok: push uses PUSH_BEFORE (rc=0, base=$actual)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: push (rc=$rc, base=$actual)"
    fail=$(( fail + 1 ))
fi

# --- 3. push with all-zero PUSH_BEFORE uses empty-tree SHA --------------

echo "Test 3: push with all-zero PUSH_BEFORE uses empty-tree SHA"
(
    export GITHUB_EVENT_NAME="push"
    export PUSH_BEFORE="0000000000000000000000000000000000000000"
    source "$lib"
    with_env >"$tmpdir/t3.out" 2>&1
    echo $? >"$tmpdir/t3.rc"
)
actual="$(cat "$tmpdir/t3.out")"
rc="$(cat "$tmpdir/t3.rc")"
if [[ "$rc" == "0" && "$actual" == "4b825dc642cb6eb9a060e54bf8d69288fbee4904" ]]; then
    echo "  ok: push all-zero uses empty-tree SHA (rc=0, base=$actual)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: push all-zero (rc=$rc, base=$actual)"
    fail=$(( fail + 1 ))
fi

# --- 4. push with missing PUSH_BEFORE uses empty-tree SHA --------------

echo "Test 4: push with missing PUSH_BEFORE uses empty-tree SHA"
(
    unset PUSH_BEFORE
    export GITHUB_EVENT_NAME="push"
    source "$lib"
    with_env >"$tmpdir/t4.out" 2>&1
    echo $? >"$tmpdir/t4.rc"
)
actual="$(cat "$tmpdir/t4.out")"
rc="$(cat "$tmpdir/t4.rc")"
if [[ "$rc" == "0" && "$actual" == "4b825dc642cb6eb9a060e54bf8d69288fbee4904" ]]; then
    echo "  ok: push missing-before uses empty-tree SHA (rc=0, base=$actual)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: push missing-before (rc=$rc, base=$actual)"
    fail=$(( fail + 1 ))
fi

# --- 5. workflow_dispatch with DISPATCH_BASE uses DISPATCH_BASE -----------

echo "Test 5: workflow_dispatch with DISPATCH_BASE uses DISPATCH_BASE"
(
    unset PUSH_BEFORE
    export GITHUB_EVENT_NAME="workflow_dispatch"
    export DISPATCH_BASE="$real_sha"
    source "$lib"
    with_env >"$tmpdir/t5.out" 2>&1
    echo $? >"$tmpdir/t5.rc"
)
actual="$(cat "$tmpdir/t5.out")"
rc="$(cat "$tmpdir/t5.rc")"
if [[ "$rc" == "0" && "$actual" == "$real_sha" ]]; then
    echo "  ok: dispatch uses DISPATCH_BASE (rc=0, base=$actual)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: dispatch with input (rc=$rc, base=$actual)"
    fail=$(( fail + 1 ))
fi

# --- 6. workflow_dispatch without DISPATCH_BASE, with HEAD^ -------------

echo "Test 6: workflow_dispatch without DISPATCH_BASE, with HEAD^ uses HEAD^"
(
    unset PUSH_BEFORE
    export GITHUB_EVENT_NAME="workflow_dispatch"
    unset DISPATCH_BASE
    source "$lib"
    with_env >"$tmpdir/t6.out" 2>&1
    echo $? >"$tmpdir/t6.rc"
)
actual="$(cat "$tmpdir/t6.out")"
rc="$(cat "$tmpdir/t6.rc")"
if [[ "$rc" == "0" && "$actual" == "HEAD^" ]]; then
    echo "  ok: dispatch no-input + HEAD^ uses HEAD^ (rc=0, base=$actual)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: dispatch no-input + HEAD^ (rc=$rc, base=$actual)"
    fail=$(( fail + 1 ))
fi

# --- 7. workflow_dispatch without DISPATCH_BASE, HEAD^ invalid, empty-tree SHA ---
# Exercise the "initial commit" / fresh-history fallback. To
# make HEAD^ invalid, source the dispatch library in a
# temporary isolated git repository containing a single root
# commit (no parent) so `git rev-parse --verify HEAD^` fails.

isolated_repo="$(mktemp -d)"
git -C "$isolated_repo" init --quiet -b main
git -C "$isolated_repo" -c user.email=test@example.com -c user.name=test commit \
    --quiet --allow-empty -m "root"
root_sha="$(git -C "$isolated_repo" rev-parse HEAD)"
echo "Test 7: isolated git repo (HEAD^ invalid) uses empty-tree SHA"
(
    unset PUSH_BEFORE
    unset DISPATCH_BASE
    export GITHUB_EVENT_NAME="workflow_dispatch"
    cd "$isolated_repo"
    if git rev-parse --verify --quiet HEAD^ >/dev/null 2>&1; then
        echo "FAIL: HEAD^ should be invalid in the isolated repo setup" >&2
        exit 2
    fi
    source "$lib"
    with_env >"$tmpdir/t7.out" 2>&1
    echo $? >"$tmpdir/t7.rc"
)
actual="$(cat "$tmpdir/t7.out")"
rc="$(cat "$tmpdir/t7.rc")"
rm -rf "$isolated_repo"
if [[ "$rc" == "0" && "$actual" == "4b825dc642cb6eb9a060e54bf8d69288fbee4904" ]]; then
    echo "  ok: dispatch initial-commit fallback uses empty-tree SHA (rc=0, base=$actual)"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: dispatch initial-commit fallback (rc=$rc, base=$actual)"
    fail=$(( fail + 1 ))
fi

# --- 8. unsupported event exits 2 ------------------------------------------

echo "Test 8: unsupported event exits 2"
(
    export GITHUB_EVENT_NAME="schedule"
    source "$lib"
    with_env >"$tmpdir/t8.out" 2>&1
    echo $? >"$tmpdir/t8.rc"
)
rc="$(cat "$tmpdir/t8.rc")"
if [[ "$rc" == "2" ]]; then
    echo "  ok: unsupported event exits 2"
    pass=$(( pass + 1 ))
else
    echo "  FAIL: unsupported event (rc=$rc)"
    fail=$(( fail + 1 ))
fi

# --- summary --------------------------------------------------------------

echo
echo "Summary: $pass passed, $fail failed"
if [[ "$fail" -gt 0 ]]; then
    exit 1
fi