#!/bin/sh
# The script of the test job (ci/checks.yml): builds every test of the workspace and runs those the
# change needs (scripts/affected-tests.sh), compared with the merge request's base or, in main's
# child pipeline, the commit before the push (ci/main-scope.sh). A tag or a run by hand has
# neither and runs every test. The build stays the whole workspace, so a warm cache rebuilds only
# the changed crates and their dependants (ci/target-stamp.sh).
set -eu
base="${CI_MERGE_REQUEST_DIFF_BASE_SHA:-${PUSH_BEFORE_SHA:-}}"
filter="$(sh scripts/affected-tests.sh "$base")"
if [ "$filter" = "none()" ]; then
    echo "test: no test reads the changed files"
    exit 0
fi
cargo nextest run --locked --workspace --profile ci --no-tests=pass -E "$filter"
# nextest runs no doctests.
cargo test --locked --workspace --doc
sh ci/target-stamp.sh save
