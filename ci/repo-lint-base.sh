#!/bin/sh
# Runs repo-lint against the commit before the push or the merge request's base, so that a raised
# length allowlist fails (RL002). main's child pipeline passes the push's in PUSH_BEFORE_SHA
# (ci/main-scope.sh). A new branch, a tag or a commit outside the shallow clone falls back
# to the parent commit, as .github/workflows/macos.yml does.
set -eu
base="${CI_MERGE_REQUEST_DIFF_BASE_SHA:-${PUSH_BEFORE_SHA:-${CI_COMMIT_BEFORE_SHA:-}}}"
case "$base" in
    '' | 0000000000000000000000000000000000000000) base=HEAD~1 ;;
esac
git cat-file -e "$base^{commit}" 2> /dev/null || base=HEAD~1
cargo run --quiet --locked -p repo-lint -- --base "$base"
