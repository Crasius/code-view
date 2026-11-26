#!/bin/sh
# land-pr.sh: brings a GitHub pull request into its own task worktree, ready to read and check.
#
#   sh scripts/land-pr.sh <number>
#
# Fetches refs/pull/<number>/head from the remote 'github', makes the worktree pr-<number> from main
# with scripts/worktree.sh (without its warm-up build) and merges the pull request head into it with
# a merge commit, then prints the pull request's commits and changed files and stops. It builds
# nothing, never pushes and never touches main: pull request code is untrusted until it is read
# (AGENTS.md, Trust). The lead reads the diff, runs the checks in the worktree and merges pr-<number>
# into main with --no-ff (rule git-flow, Pull requests from GitHub). Only a merge that keeps the pull
# request's commits lets GitHub mark it merged once main reaches the mirror, so no squash, no rebase.

set -u

die() {
  printf 'land-pr: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf 'usage: sh scripts/land-pr.sh <pull request number>\n' >&2
  exit 2
}

[ $# -eq 1 ] || usage
number=$1
case "$number" in
  '' | 0* | *[!0-9]*) die "invalid pull request number '$number': use a positive number without leading zeros" ;;
esac

remote=github
base=main
name="pr-$number"
scripts=$(cd "$(dirname "$0")" && pwd) || die "cannot find the scripts folder"

common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || die "not inside a git repository"
main=$(dirname "$common")
git -C "$main" remote get-url "$remote" >/dev/null 2>&1 || die "no remote '$remote' (rule git-flow: the public GitHub mirror)"
git -C "$main" show-ref --verify --quiet "refs/heads/$base" || die "no branch '$base'"
git -C "$main" show-ref --verify --quiet "refs/heads/$name" && die "branch '$name' exists; finish or remove it first (sh scripts/worktree.sh done $name)"

# The head goes to a private ref first, so that a missing pull request stops before any worktree
# exists. The merge below keeps the commit reachable from pr-<number>, so the ref is dropped again.
ref="refs/land-pr/$number"
git -C "$main" fetch --no-tags "$remote" "+refs/pull/$number/head:$ref" || die "cannot fetch pull request #$number from '$remote'"
head=$(git -C "$main" rev-parse --verify --quiet "$ref^{commit}") || die "fetched no commit for pull request #$number"
git -C "$main" update-ref -d "$ref"

# WORKTREE_WARM=0: the warm-up would build the pull request's code before anyone read it.
made=$(WORKTREE_WARM=0 sh "$scripts/worktree.sh" new "$name" "$base") || die "scripts/worktree.sh new $name failed"
path=$(printf '%s\n' "$made" | tail -n 1)
[ -d "$path" ] || die "worktree.sh printed no worktree folder"

# No hooks: a hooks path that points into the checkout would run the pull request's own scripts.
if ! git -C "$path" -c core.hooksPath=/dev/null merge --no-ff --no-edit -m "Merge GitHub pull request #$number" "$head"; then
  printf 'land-pr: the merge stopped in %s (git says why above). Resolve conflicts there and commit, or abort with: git -C %s merge --abort\n' "$path" "$path" >&2
  exit 1
fi

printf '\nPull request #%s (head %s) is merged into %s in:\n  %s\n' "$number" "$head" "$name" "$path"
printf '\nIts commits:\n'
git -C "$path" log --no-merges --format='  %h %an: %s' "$base..$head"
printf '\nChanged files against %s:\n' "$base"
git -C "$path" diff --name-status "$base" HEAD | sed 's/^/  /'

# Files that run code at build, check or CI time; read these first.
risky=$(git -C "$path" diff --name-only "$base" HEAD | grep -E \
  '(^|/)build\.rs$|(^|/)Cargo\.(toml|lock)$|^\.cargo/|^rust-toolchain\.toml$|^ci/|^scripts/|^tools/|^\.github/|^\.gitlab-ci\.yml$|^\.agents/|^\.claude/|^browser-extension/|\.(sh|ps1|cmd|bat|js)$' || true)
if [ -n "$risky" ]; then
  printf '\nRead these first: they run code when something builds, checks or deploys:\n'
  printf '%s\n' "$risky" | sed 's/^/  /'
fi

printf '\nNothing was built. Next (rule git-flow, Pull requests from GitHub):\n'
printf '  1. Read the whole diff: git -C %s diff %s HEAD\n' "$path" "$base"
printf '  2. Check the CLA statement in the pull request description.\n'
printf '  3. Run the checks in the worktree, then merge %s into %s with --no-ff in the main checkout.\n' "$name" "$base"
