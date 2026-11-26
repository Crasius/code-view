#!/bin/sh
# Runs repo-lint in the lint job (ci/lint.sh). Only RL002 depends on a base revision: the length
# allowlist may not grow against it (tools/repo-lint/README.md). Every other rule checks the whole
# tree in every run.
# A merge request compares with its diff base. Every other pipeline (a push to main, a tag, a run by
# hand) compares with every version of the allowlist on the first-parent line of HEAD: the commit
# before the push alone would let a raise through that an earlier red or cancelled push brought in.
# The first run checks everything; the others only the allowlist. Needs the full history
# (GIT_DEPTH 0 in ci/checks.yml, lint).
set -eu
allow=tools/repo-lint/file-lengths.allow
lint() {
    cargo run --quiet --locked -p repo-lint -- "$@"
}

if [ -n "${CI_MERGE_REQUEST_DIFF_BASE_SHA:-}" ]; then
    bases="$CI_MERGE_REQUEST_DIFF_BASE_SHA"
else
    bases="$(git log --first-parent --format=%H HEAD -- "$allow")"
fi
first=true
for base in ${bases:-HEAD}; do
    # repo-lint skips RL002 for a base it cannot read, so a base missing from the clone fails here.
    git cat-file -e "$base^{commit}" || { echo "repo-lint-base: $base is not in the clone" >&2; exit 1; }
    if [ "$first" = true ]; then
        lint --base "$base"
        first=false
    else
        lint --base "$base" "$allow"
    fi
done
echo "repo-lint-base: the allowlist checked against $(printf '%s\n' "${bases:-HEAD}" | grep -c .) base(s)"
