#!/bin/sh
# worktree.sh: one git worktree per task or lane, with checked cleanup.
#
#   sh scripts/worktree.sh new <name> [base]   worktree ../<repo>-wt/<name> on new branch <name> from base (default: the branch of the main checkout)
#   sh scripts/worktree.sh done <name>         remove worktree and branch; refuses when dirty or not merged into its base
#   sh scripts/worktree.sh list                list worktrees
#   sh scripts/worktree.sh check               exit 1 when a worktree folder is missing or a merged branch still has a worktree
#   sh scripts/worktree.sh cache               point this checkout's cargo at sccache (what 'new' does for a new worktree)
#
# The base and the start commit are kept in git config (branch.<name>.worktreebase, branch.<name>.worktreestart);
# deleting the branch removes them. 'done' only removes branches that carry both records.
#
# Build cache: every worktree has its own target folder, so a new one would compile the whole workspace and its
# dependencies from nothing. 'new' warms the worktree in the background (test binaries, then clippy) while the lane
# reads. With sccache on PATH it first writes a git-ignored .cargo/config.toml that compiles through it, so cacheable
# dependencies come from the shared cache; our own crates build incrementally as before. Its log is tmp/warm.log, tmp/warm.done marks the end. Cargo's lock
# on the target folder makes a lane's own cargo command wait for the warm-up instead of building twice.
# WORKTREE_WARM=0 skips the warm-up. One script for macOS, Linux and Windows (Git Bash).

set -u

die() {
  printf 'worktree: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf 'usage: sh scripts/worktree.sh new <name> [base] | done <name> | list | check | cache\n' >&2
  exit 2
}

common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || die "not inside a git repository"
[ "$(basename "$common")" = ".git" ] || die "bare repositories are not supported"
main=$(dirname "$common")
repo=$(basename "$main")
root="$(dirname "$main")/$repo-wt"

check_name() {
  case "$1" in
    '' | -* | . | .. | *[!A-Za-z0-9._-]*) die "invalid name '$1': use letters, digits, '.', '_' and '-'" ;;
  esac
  git check-ref-format "refs/heads/$1" || die "invalid branch name '$1'"
}

# Prints "<path><TAB><branch ref>" for every worktree except the main checkout; branch is empty when detached.
linked_worktrees() {
  git -C "$main" worktree list --porcelain | awk '
    /^worktree / { if (n > 0) print p "\t" b; p = substr($0, 10); b = ""; n++ }
    /^branch /   { b = substr($0, 8) }
    END          { if (n > 0) print p "\t" b }
  ' | tail -n +2
}

path_of_branch() {
  linked_worktrees | awk -F '\t' -v ref="refs/heads/$1" '$2 == ref { print $1 }'
}

# Prints the recorded base (worktreebase) or start commit (worktreestart) of a branch; exit 1 when not recorded.
record_of() {
  git -C "$main" config --get "branch.$1.$2"
}

# Exit 0 when <tip> is on the first-parent line of <base> after <start>. Such a tip came from the base
# (the lane only fast-forwarded to it), so it is not the lane's own work.
on_base_line() {
  git -C "$main" rev-list --first-parent "$2" "^$3" | grep -qx "$1"
}

# Writes <dir>/.cargo/config.toml with sccache as rustc wrapper; returns 1 without sccache.
# The bare name: cargo looks it up on PATH, so an upgrade that moves sccache breaks no worktree.
use_cache() {
  command -v sccache >/dev/null 2>&1 || return 1
  mkdir -p "$1/.cargo" || return 1
  printf '# Written by scripts/worktree.sh (git-ignored): dependencies come from the shared sccache.\n[build]\nrustc-wrapper = "sccache"\n' \
    > "$1/.cargo/config.toml"
}

# Builds the test binaries and the clippy output of <dir> in the background.
warm() {
  rm -f "$1/tmp/warm.done"
  (
    cd "$1" || exit 1
    nohup sh -c 'cargo test -q --workspace --no-run; cargo clippy -q --workspace --all-targets; date > tmp/warm.done' \
      > tmp/warm.log 2>&1 &
  )
}

cmd_new() {
  [ $# -ge 1 ] && [ $# -le 2 ] || usage
  name=$1
  base=${2:-$(git -C "$main" branch --show-current)}
  [ -n "$base" ] || die "the main checkout is detached; name the base"
  check_name "$name"
  git -C "$main" show-ref --verify --quiet "refs/heads/$name" && die "branch '$name' exists"
  path="$root/$name"
  [ -e "$path" ] && die "folder '$path' exists"
  start=$(git -C "$main" rev-parse --verify --quiet "$base^{commit}") || die "base '$base' not found"
  mkdir -p "$root" || die "cannot create '$root'"
  git -C "$main" worktree add "$path" -b "$name" "$base" || die "git worktree add failed"
  git -C "$main" config "branch.$name.worktreebase" "$base"
  git -C "$main" config "branch.$name.worktreestart" "$start"
  use_cache "$path" || printf 'worktree: sccache not found; dependencies build without the shared cache (rule worktrees, Build cache)\n' >&2
  # Job logs, exit markers and commit messages go there, with or without the warm-up.
  mkdir -p "$path/tmp" || die "cannot create '$path/tmp'"
  [ "${WORKTREE_WARM:-1}" = 0 ] || warm "$path" || printf 'worktree: warm-up not started\n' >&2
  printf '%s\n' "$path"
}

cmd_cache() {
  [ $# -eq 0 ] || usage
  here=$(git rev-parse --show-toplevel) || die "not inside a checkout"
  use_cache "$here" || die "sccache not found on PATH (rule worktrees, Build cache)"
  printf 'cached %s\n' "$here"
}

cmd_done() {
  [ $# -eq 1 ] || usage
  name=$1
  check_name "$name"
  git -C "$main" show-ref --verify --quiet "refs/heads/$name" || die "no branch '$name'"
  base=$(record_of "$name" worktreebase) && record_of "$name" worktreestart >/dev/null \
    || die "'$name' was not created by scripts/worktree.sh"
  [ "$name" = "$base" ] && die "'$name' is its own base"
  path=$(path_of_branch "$name")
  if [ -n "$path" ] && [ -d "$path" ]; then
    here=$(git rev-parse --show-toplevel 2>/dev/null || true)
    [ "$here" = "$path" ] && die "run 'done' from another checkout, not from '$path'"
    dirty=$(git -C "$path" status --porcelain) || die "git status failed in '$path'"
    [ -n "$dirty" ] && die "worktree '$path' has uncommitted or untracked files"
  fi
  git -C "$main" rev-parse --verify --quiet "$base^{commit}" >/dev/null || die "base '$base' not found"
  git -C "$main" merge-base --is-ancestor "refs/heads/$name" "$base" || die "branch '$name' is not merged into '$base'"
  if [ -n "$path" ]; then
    if [ -d "$path" ]; then
      git -C "$main" worktree remove "$path" || die "git worktree remove failed"
    else
      git -C "$main" worktree prune
    fi
  fi
  # Safe: the merge into the base was checked above; -d would compare with the current HEAD instead.
  git -C "$main" branch -D "$name" >/dev/null || die "git branch -D failed"
  printf 'removed %s\n' "$name"
}

cmd_check() {
  [ $# -eq 0 ] || usage
  status=0
  list=$(linked_worktrees)
  tab=$(printf '\t')
  while IFS="$tab" read -r path ref; do
    [ -n "$path" ] || continue
    if [ ! -d "$path" ]; then
      printf 'missing folder: %s (%s); run: git worktree prune\n' "$path" "${ref:-detached}"
      status=1
      continue
    fi
    [ -n "$ref" ] || continue
    name=${ref#refs/heads/}
    start=$(record_of "$name" worktreestart) || continue
    base=$(record_of "$name" worktreebase) || continue
    tip=$(git -C "$main" rev-parse "$ref")
    [ "$tip" = "$start" ] && continue
    git -C "$main" rev-parse --verify --quiet "$base^{commit}" >/dev/null || continue
    if git -C "$main" merge-base --is-ancestor "$tip" "$base" && ! on_base_line "$tip" "$base" "$start"; then
      printf 'merged into %s but still has a worktree: %s; run: sh scripts/worktree.sh done %s\n' "$base" "$path" "$name"
      status=1
    fi
  done <<EOF
$list
EOF
  return $status
}

[ $# -ge 1 ] || usage
command=$1
shift
case "$command" in
  new) cmd_new "$@" ;;
  done) cmd_done "$@" ;;
  list) [ $# -eq 0 ] || usage; git -C "$main" worktree list ;;
  check) cmd_check "$@" ;;
  cache) cmd_cache "$@" ;;
  *) usage ;;
esac
