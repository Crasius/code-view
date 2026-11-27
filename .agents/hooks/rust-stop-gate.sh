#!/bin/sh
# rust-stop-gate: check the Rust files an agent changed before it ends its turn or reports.
#
# Why: LLM-written Rust fails to compile or lint more often than other languages (RS-AGENT-01,
# RS-AGENT-05). "It compiles" is not done; this feeds rustfmt and clippy findings back to the
# agent while the change is still in its context.
#
# The adapter between the Claude Code Stop hook (.claude/settings.json) and
# `sh scripts/check.sh --changed`, which holds the checks and their exit codes: 0 = clean,
# 2 = findings on stderr (the Stop hook makes the agent continue and fix them), 1 = clippy did not
# run (lock held, unknown --base; stderr says why, the turn may end).
# Claude Code runs Stop hooks for the main session only; subagents and workflow agents fire
# SubagentStop, which is not registered, because their worktrees are not the hook's directory.
# Input: the hook JSON on stdin. When stop_hook_active is true (the agent already continues
# because of this hook) it exits 0 at once, so it can never loop. Its cwd field names the
# checkout to check. --base <rev> is passed on to check.sh.
# A missing git or cargo, or a repository without Cargo.toml, exits 0: the gate never blocks a
# session it cannot check. POSIX sh.

base=""
while [ $# -gt 0 ]; do
  case $1 in
    --base)
      [ $# -ge 2 ] || { echo "rust-stop-gate: --base needs a revision" >&2; exit 1; }
      base=$2
      shift 2 ;;
    *) echo "rust-stop-gate: unknown argument $1 (usage: rust-stop-gate.sh [--base <rev>])" >&2; exit 1 ;;
  esac
done

# check.sh of the checkout this hook belongs to, found before the cd below.
case $0 in */*) hooks=${0%/*} ;; *) hooks=. ;; esac
check=$(cd "$hooks/../.." && pwd)/scripts/check.sh
[ -f "$check" ] || { echo "rust-stop-gate: $check is missing; the gate did not run" >&2; exit 1; }

input=$(cat 2>/dev/null)
case $input in
  *'"stop_hook_active":true'*|*'"stop_hook_active": true'*) exit 0 ;;
esac

# The hook's cwd field (backslashes, JSON-escaped or not, become slashes); a run by hand keeps
# its own directory.
cwd=$(printf '%s\n' "$input" | sed -n 's/.*"cwd"[ ]*:[ ]*"\([^"]*\)".*/\1/p' | sed -n '1{s|\\\\*|/|g;p;}')
if [ -n "$cwd" ] && [ -d "$cwd" ]; then
  cd "$cwd" || exit 0
fi

root=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
cd "$root" || exit 0
[ -f Cargo.toml ] || exit 0
command -v cargo >/dev/null 2>&1 || exit 0

if [ -n "$base" ]; then
  exec sh "$check" --changed --base "$base" </dev/null
fi
exec sh "$check" --changed </dev/null
