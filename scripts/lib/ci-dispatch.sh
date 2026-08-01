#!/usr/bin/env bash
#
# scripts/lib/ci-dispatch.sh
#
# Sourced library for CI whitespace-gate base SHA selection.
#
# Public API
# ----------
#   dispatch_base_sha
#     Echoes the base SHA to use for
#     `scripts/check-whitespace.sh <base>..HEAD`. Exits with a
#     documented status on error.
#
# Inputs (environment variables)
# ------------------------------
#   GITHUB_EVENT_NAME  "pull_request" | "push" | "workflow_dispatch"
#   PR_BASE_SHA        set on pull_request events
#   PUSH_BEFORE        set on push events (may be all-zero for initial commit)
#   DISPATCH_BASE      workflow_dispatch input "base" (optional)
#   EMPTY_TREE_SHA     SHA of the empty Git tree
#                      (default: 4b825dc642cb6eb9a060e54bf8d69288fbee4904)
#
# Exit codes
# ----------
#   0  base SHA determined
#   2  invalid input (empty base, unsupported event, unresolvable SHA)
#   3  plumbing error (git cannot resolve the base inside this checkout)
#
# Rationale
# ---------
# The freeze-point cycle-4a and cycle-4b verdicts require that
# the whitespace gate covers the EXACT range of commits that
# just landed, never an empty range and never a single-commit
# subset of a multi-commit push. Direct `HEAD^..HEAD` fails on
# multi-commit pushes; `GITHUB_SHA_BEFORE` is not the event
# payload wiring. A workflow_dispatch run also has no defined
# base input.
#
# This library implements the dispatch logic separately from
# the workflow YAML so the behavior can be exercised by a
# shell integration test without spinning up GitHub Actions.
# The YAML delegates to `dispatch_base_sha` after sourcing
# this file.
#
# See `nb-api:reviews/2` (cycle-4b verdict C4B-V1-1) and
# `nb-api:coordination/general/5`.

# Resolve the empty-tree SHA once at source time. The literal
# is the SHA-1 of an empty git tree (`git mktree </dev/null`).
EMPTY_TREE_SHA_DEFAULT="4b825dc642cb6eb9a060e54bf8d69288fbee4904"

dispatch_base_sha() {
    local event="${GITHUB_EVENT_NAME:-}"
    local base=""
    case "$event" in
        pull_request)
            base="${PR_BASE_SHA:-}"
            ;;
        push)
            # On a fresh repo's first push, `github.event.before`
            # is all zeros (40 hex `0`s). That does NOT resolve
            # to a commit. Treat all-zero as "no parent" and
            # fall back to the empty-tree SHA so the range
            # covers the whole new history.
            if [[ -z "${PUSH_BEFORE:-}" ]] || [[ "${PUSH_BEFORE}" =~ ^0+$ ]]; then
                base="${EMPTY_TREE_SHA:-$EMPTY_TREE_SHA_DEFAULT}"
            else
                base="${PUSH_BEFORE}"
            fi
            ;;
        workflow_dispatch)
            base="${DISPATCH_BASE:-}"
            if [[ -z "$base" ]]; then
                # No dispatch input. Fall back to HEAD^ if it
                # is a valid commit; otherwise the empty-tree
                # SHA covers the whole history.
                if git rev-parse --verify --quiet HEAD^ >/dev/null 2>&1; then
                    base="HEAD^"
                else
                    base="${EMPTY_TREE_SHA:-$EMPTY_TREE_SHA_DEFAULT}"
                fi
            fi
            ;;
        *)
            echo "::error::unsupported event: ${event}" >&2
            return 2
            ;;
    esac
    if [[ -z "$base" ]]; then
        echo "::error::could not determine base SHA for event ${event}" >&2
        return 2
    fi
    # Validate that base resolves to a commit or the empty
    # tree. If neither, surface a clear error rather than
    # passing an invalid rev to the whitespace script.
    if ! git rev-parse --verify --quiet "${base}^{commit}" >/dev/null 2>&1 \
       && ! git rev-parse --verify --quiet "${base}^{tree}" >/dev/null 2>&1; then
        echo "::error::base SHA '${base}' does not resolve to a commit or tree" >&2
        return 2
    fi
    printf '%s' "$base"
}