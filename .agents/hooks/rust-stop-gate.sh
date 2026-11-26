#!/bin/sh
# rust-stop-gate: check the Rust files an agent changed before it ends its turn or reports.
#
# Why: LLM-written Rust fails to compile or lint more often than other languages (RS-AGENT-01,
# RS-AGENT-05). "It compiles" is not done; this feeds rustfmt and clippy findings back to the
# agent while the change is still in its context.
#
# Two callers:
#   - The Claude Code Stop hook (.claude/settings.json). Claude Code runs Stop hooks for the main
#     session only; subagents and workflow agents fire SubagentStop, which is not registered,
#     because their worktrees are not the hook's directory. Input: the hook JSON on stdin. When
#     stop_hook_active is true (the agent already continues because of this hook) it exits 0 at
#     once, so it can never loop. Its cwd field names the directory to check.
#   - A lane, by hand in its own worktree before it commits and before its report (rule lanes):
#       cd <worktree> && sh .agents/hooks/rust-stop-gate.sh --base <lane base branch> </dev/null
#     --base adds the files the lane committed since its fork point from that branch.
# Changed files: .rs files that differ from HEAD (or from the fork point with --base) in the work
# tree or the index, plus untracked ones that are not ignored, outside `vendor/` folders (vendored
# third-party code keeps its upstream style, rule rust, Dependencies). None: exit 0 at once.
# Checks, in the repository of that directory:
#   1. rustfmt --check on the changed files (rustfmt.toml applies).
#   2. cargo clippy --all-targets on the crates that own them, at the levels of [workspace.lints]
#      (no -D warnings, so lint findings stay warnings and keep their file). Warnings are reported
#      only in the changed files, because other files may still carry warnings that another lane
#      fixes; errors (compile errors, deny-level lints) are reported in every file.
#      When another cargo job holds the build directory lock (a background build or test), clippy
#      is stopped at once instead of waiting for it, and the gate says that clippy did not run.
# Exit 2 = findings on stderr (the Stop hook makes the agent continue and fix them). Exit 1 =
# clippy could not run (lock held, unknown --base); stderr says why, the turn may end. Exit 0 =
# clean. A missing git or cargo, or a repository without Cargo.toml, exits 0: the gate never
# blocks a session it cannot check. POSIX sh and awk.

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

since=HEAD
if [ -n "$base" ]; then
  since=$(git merge-base "$base" HEAD 2>/dev/null) || {
    echo "rust-stop-gate: no common ancestor of $base and HEAD; pass the lane's base branch" >&2
    exit 1
  }
fi

changed=$( {
  git -c core.quotepath=off diff --name-only "$since" -- '*.rs'
  git -c core.quotepath=off ls-files --others --exclude-standard -- '*.rs'
} 2>/dev/null | sort -u | while IFS= read -r f; do
  case /$f in */vendor/*) continue ;; esac
  [ -f "$f" ] && printf '%s\n' "$f"
done)
[ -n "$changed" ] || exit 0

# The package name of the nearest Cargo.toml above each file; the virtual root has none.
crates=$(printf '%s\n' "$changed" | while IFS= read -r f; do
  dir=${f%/*}
  [ "$dir" = "$f" ] && dir=.
  while [ "$dir" != "." ] && [ ! -f "$dir/Cargo.toml" ]; do
    case $dir in */*) dir=${dir%/*} ;; *) dir=. ;; esac
  done
  awk '/^\[package\]/ { p = 1; next } /^\[/ { p = 0 }
       p && /^name[ \t]*=/ { sub(/^[^"]*"/, ""); sub(/".*/, ""); print; exit }' "$dir/Cargo.toml"
done | sort -u)

report=""
skipped=""

# rustfmt prints absolute paths (verbatim \\?\ ones on Windows); show them repo-relative.
fmt=$(printf '%s\n' "$changed" | tr '\n' '\0' | xargs -0 rustfmt --check -l 2>&1 |
  sed -e 's|^\\\\?\\||' -e 's|\\|/|g' -e "s|^$root/||")
if [ -n "$fmt" ]; then
  report="rustfmt: these files are not formatted; run rustfmt --edition 2024 on them:
$fmt
"
fi

if [ -n "$crates" ]; then
  packages=$(printf '%s\n' "$crates" | sed 's/^/-p /' | tr '\n' ' ')
  log=$(mktemp 2>/dev/null) || log="${TMPDIR:-/tmp}/rust-stop-gate.$$"
  # Not --quiet: cargo's "Blocking waiting for file lock" line is the signal to stop.
  # shellcheck disable=SC2086 # $packages is a list of -p flags
  cargo clippy $packages --all-targets --message-format=short >"$log" 2>&1 &
  pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    if grep -q 'Blocking waiting for file lock' "$log" 2>/dev/null; then
      kill "$pid" 2>/dev/null
      skipped="clippy did not run: another cargo job holds the build directory lock. Run cargo clippy $packages--all-targets -- -D warnings once it has finished."
      break
    fi
    sleep 1
  done
  wait "$pid" 2>/dev/null
  if [ -z "$skipped" ]; then
    relevant=$(CHANGED=$changed awk '
      BEGIN { n = split(ENVIRON["CHANGED"], list, "\n"); for (i = 1; i <= n; i++) keep[list[i]] = 1 }
      {
        line = $0; gsub(/\\/, "/", line)
        path = line; sub(/:[0-9]+:[0-9]+: .*/, "", path)
        if (line ~ /^[^ ]+:[0-9]+:[0-9]+: error/) print line
        else if (path in keep && line ~ /^[^ ]+:[0-9]+:[0-9]+: warning/) print line
        else if (line ~ /^error(\[|:)/ && line !~ /could not compile/ && line !~ /aborting due to/) print line
      }' "$log")
    if [ -n "$relevant" ]; then
      report="${report}clippy on $(printf '%s' "$crates" | tr '\n' ' '):
$relevant
"
    fi
  fi
  rm -f "$log"
fi

if [ -n "$report" ]; then
  printf 'rust-stop-gate: fix these before you finish (rule rust, Lints and Format):\n%s' "$report" >&2
  [ -n "$skipped" ] && printf '%s\n' "$skipped" >&2
  exit 2
fi
if [ -n "$skipped" ]; then
  printf 'rust-stop-gate: %s\n' "$skipped" >&2
  exit 1
fi
exit 0
