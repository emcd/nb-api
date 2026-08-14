#!/usr/bin/env bash
# Spike: does nb 7.24.0's core command surface work under Git Bash on
# windows-latest? Mirrors the exact commands nb-api spawns (see
# src/client.rs exec sites, src/git.rs git_capture sites, and
# NbTestEnv::initialize_notebook).
#
# Run under Git Bash on windows-latest with:
#   - NB_BIN=<path to the real nb bash script>
#   - NB_BIN_CMD=<path to a .cmd launcher that runs NB_BIN via bash>
#   - NB_DIR, HOME set by the caller (fixture-like)
set -uo pipefail

PASS=0
FAIL=0
declare -a FAILURES=()

note_fail() {
  local what="$1"
  local detail="${2:-}"
  FAIL=$((FAIL + 1))
  FAILURES+=("$what")
  printf 'FAIL: %s%s\n' "$what" "$([ -n "$detail" ] && printf ' :: %s' "$detail")"
}

note_pass() {
  local what="$1"
  PASS=$((PASS + 1))
  printf 'PASS: %s\n' "$what"
}

run_expect_ok() {
  local what="$1"
  shift
  local out
  if out="$("$@" 2>&1)"; then
    note_pass "$what"
  else
    note_fail "$what" "$out"
  fi
}

run_expect_exit() {
  local what="$1"
  local want="$2"
  shift 2
  local code=0
  "$@" >/dev/null 2>&1 || code=$?
  if [ "$code" -eq "$want" ]; then
    note_pass "$what"
  else
    note_fail "$what" "want exit $want, got $code"
  fi
}

printf '=== probe env ===\n'
printf 'bash: %s\n' "$(bash --version | head -1)"
printf 'OSTYPE: %s\n' "$OSTYPE"
printf 'NB_BIN: %s\n' "${NB_BIN:-<unset>}"
printf 'NB_BIN_CMD: %s\n' "${NB_BIN_CMD:-<unset>}"
printf 'NB_DIR: %s\n' "${NB_DIR:-<unset>}"
printf 'HOME: %s\n' "${HOME:-<unset>}"
command -v git || echo 'git NOT on PATH'
command -v bash || echo 'bash NOT on PATH'

# --- Spawnability (the nbspec killer) ---
run_expect_ok 'spawn nb.cmd via PATH lookup (cmd.exe CreateProcess path)' \
  cmd.exe //c "nb --version"
run_expect_ok 'spawn nb bash script via Git Bash directly' \
  bash "$NB_BIN" --version

# --- Fixture init (NbTestEnv::initialize_notebook) ---
# nb's main loop short-circuits first-run `_init` when both NB_DIR and
# NB_NOTEBOOK_PATH exist (mirrors the fixture's hidden `.init_stub`).
mkdir -p "$NB_DIR/.init_stub"
run_expect_ok 'nb notebooks add scratch' \
  env NB_NOTEBOOK_PATH="$NB_DIR/.init_stub" bash "$NB_BIN" notebooks add scratch
printf '%s' 'scratch' > "$NB_DIR/.current"
run_expect_ok 'nb notebooks show scratch --path' \
  bash "$NB_BIN" notebooks show scratch --path

# --- Core command surface exercised by NbClient ---
run_expect_ok 'nb add note' \
  bash "$NB_BIN" add 'hello from probe'

# Derive the note id from the on-disk notebook root (robust to ls
# ANSI/CRLF/formatting differences): first .md file in the notebook.
# The client passes full relative filenames as selectors (e.g.
# show_note("n.md")), so use the filename, not a bare timestamp id.
ROOT_FOR_ID="$(bash "$NB_BIN" notebooks show scratch --path)"
printf 'root for id: %s\n' "${ROOT_FOR_ID:-<none>}"
NOTE_FILE="$(
  ls -1 "$ROOT_FOR_ID" 2>/dev/null \
    | grep -iE '\.(md|txt)$' \
    | grep -v '^\.' \
    | head -1
)"
ID="${NOTE_FILE%.*}"
printf 'probe note id: %s (file: %s)\n' "${ID:-<none>}" "${NOTE_FILE:-<none>}"
printf 'current marker: %s\n' "$(cat "$NB_DIR/.current" 2>/dev/null || echo '<none>')"
printf 'scratch root ls -la:\n'
ls -la "$ROOT_FOR_ID" 2>/dev/null | sed 's/^/  |/'
printf 'NB_DIR ls -la:\n'
ls -la "$NB_DIR" 2>/dev/null | sed 's/^/  |/'
printf 'nb ls --no-color raw first 5 lines:\n'
bash "$NB_BIN" ls --no-color 2>&1 | tr -d '\r' | head -5 | sed 's/^/  |/'
printf 'nb show with filename selector (client-style):\n'
bash "$NB_BIN" show "${NOTE_FILE}" --type 2>&1 | tr -d '\r' | head -3 | sed 's/^/  |/' || true
printf 'nb show qualified scratch:<filename>:\n'
bash "$NB_BIN" show "scratch:${NOTE_FILE}" --type 2>&1 | tr -d '\r' | head -3 | sed 's/^/  |/' || true

run_expect_ok 'nb ls --no-color' bash "$NB_BIN" ls --no-color
run_expect_ok 'nb show <file> --type' bash "$NB_BIN" show "$NOTE_FILE" --type
run_expect_ok 'nb show <file> --path' bash "$NB_BIN" show "$NOTE_FILE" --path
run_expect_ok 'nb show <file> (content)' bash "$NB_BIN" show "$NOTE_FILE"
run_expect_ok 'nb edit <file> --content' bash "$NB_BIN" edit "$NOTE_FILE" --content 'edited body'
run_expect_ok 'nb notebooks --no-color' bash "$NB_BIN" notebooks --no-color

# --- Direct git surface (src/git.rs), run inside the notebook dir ---
NOTEBOOK_ROOT="$(bash "$NB_BIN" notebooks show scratch --path)"
printf 'notebook root: %s\n' "${NOTEBOOK_ROOT:-<none>}"
if [ -n "$NOTEBOOK_ROOT" ] && [ -d "$NOTEBOOK_ROOT" ]; then
  # nb auto-commits on add, so after `nb add note` the tree is already
  # clean. Create an uncommitted change ourselves to exercise the staged
  # diff / commit path (notebook_commit_all).
  printf 'dirty marker\n' > "$NOTEBOOK_ROOT/dirty-probe.txt"
  run_expect_exit 'git status --porcelain (dirty -> non-empty ok)' 0 \
    git -C "$NOTEBOOK_ROOT" status --porcelain -uall --ignored=no
  run_expect_ok 'git add -A' git -C "$NOTEBOOK_ROOT" add -A
  # diff-index compares index against HEAD; the file is now staged, so a
  # difference exists -> exit 1 (this is what notebook_is_dirty relies on).
  run_expect_exit 'git diff-index --quiet HEAD -- (staged -> exit 1)' 1 \
    git -C "$NOTEBOOK_ROOT" diff-index --quiet HEAD --
  run_expect_exit 'git diff --cached --quiet (staged -> exit 1)' 1 \
    git -C "$NOTEBOOK_ROOT" diff --cached --quiet
  run_expect_ok 'git commit -m probe' \
    git -C "$NOTEBOOK_ROOT" -c user.name='probe' -c user.email='probe@localhost' commit -m 'probe commit'
  run_expect_exit 'git diff-index --quiet HEAD -- (clean -> exit 0)' 0 \
    git -C "$NOTEBOOK_ROOT" diff-index --quiet HEAD --
else
  note_fail 'notebook root resolution' "root=${NOTEBOOK_ROOT:-<empty>}"
fi

printf '\n=== probe result: %d pass, %d fail ===\n' "$PASS" "$FAIL"
if [ "$FAIL" -gt 0 ]; then
  printf 'failed probes: %s\n' "${FAILURES[*]}"
  exit 1
fi
exit 0
