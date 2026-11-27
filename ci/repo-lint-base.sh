#!/bin/sh
# Runs repo-lint in the lint job of both CIs (the repo-lint step of scripts/check.sh with CI set).
# Only RL002 depends on a base revision: the length allowlist may not grow against it
# (tools/repo-lint/README.md). Every other rule checks the whole tree in every run.
# A merge request or pull request compares with its base: LINT_BASE (GitHub's pull request base),
# else GitLab's merge request diff base. Every other pipeline (a push to main, a tag, a run by hand)
# compares with every version of the allowlist on the first-parent line of HEAD: the commit before
# the push alone would let a raise through that an earlier red or cancelled push brought in.
# The first run checks everything; the others only the allowlist. repo-lint exits 2 on a base it
# cannot read. Needs the full history (GIT_DEPTH 0 in ci/checks.yml, lint; fetch-depth 0 on GitHub).
set -eu
allow=tools/repo-lint/file-lengths.allow
lint() {
    cargo run --quiet --locked -p repo-lint -- "$@"
}

base_rev="${LINT_BASE:-${CI_MERGE_REQUEST_DIFF_BASE_SHA:-}}"
if [ -n "$base_rev" ]; then
    bases="$base_rev"
else
    bases="$(git log --first-parent --format=%H HEAD -- "$allow")"
fi
first=true
for base in ${bases:-HEAD}; do
    if [ "$first" = true ]; then
        lint --base "$base"
        first=false
    else
        lint --base "$base" "$allow"
    fi
done
echo "repo-lint-base: the allowlist checked against $(printf '%s\n' "${bases:-HEAD}" | grep -c .) base(s)"
