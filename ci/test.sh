#!/bin/sh
# The tests of a CI run (ci/checks.yml test; .github/workflows/pull-request.yml test and macos):
# builds every test of the workspace and runs those the change needs, with ci/run-tests.sh, the run
# `sh scripts/check.sh test` makes on a development machine too. TEST_FILTER names them as a
# nextest filterset; without it a merge request's filter comes from scripts/affected-tests.sh,
# compared with the request's base, and every other pipeline (a push to main, a tag, a run by hand)
# runs every test (docs/release.md, Tests). The build stays the whole workspace, so a warm cache
# rebuilds only the changed crates and their dependants (ci/target-stamp.sh).
# Uploading test's two cache archives on GitLab took 120 to 137 s
# [observed 2026-11-26, pipelines 4905, 4906]: a shorter build leaves the cache as it is.
set -eu
filter="${TEST_FILTER:-}"
if [ -z "$filter" ]; then
    if [ -n "${CI_MERGE_REQUEST_DIFF_BASE_SHA:-}" ]; then
        filter="$(sh scripts/affected-tests.sh "$CI_MERGE_REQUEST_DIFF_BASE_SHA")"
    else
        filter="all()"
    fi
fi
sh ci/run-tests.sh --profile ci --stamp 130 "$filter"
