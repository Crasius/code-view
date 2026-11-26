#!/bin/sh
# The tests of a CI run (ci/checks.yml test; .github/workflows/pull-request.yml test and macos):
# builds every test of the workspace and runs those the change needs. TEST_FILTER names them as a
# nextest filterset; without it the filter comes from scripts/affected-tests.sh, compared with the
# merge request's base or, in main's child pipeline, the commit before the push
# (ci/main-scope.sh). A tag or a run by hand has neither and runs every test. The build stays the
# whole workspace, so a warm cache rebuilds only the changed crates and their dependants
# (ci/target-stamp.sh).
# review-app's walkthroughs run in one process with cargo test, as they always ran: under nextest's
# process per test, four of them at once timed out twice in three runs and took 135 to 158 s
# against 117 s [observed 2026-11-26, pipelines 4900 to 4904]. Every other test binary runs under
# nextest.
set -eu
filter="${TEST_FILTER:-}"
if [ -z "$filter" ]; then
    filter="$(sh scripts/affected-tests.sh "${CI_MERGE_REQUEST_DIFF_BASE_SHA:-${PUSH_BEFORE_SHA:-}}")"
fi
if [ "$filter" = "none()" ]; then
    echo "test: no test reads the changed files"
    exit 0
fi
# The build first: once it passed, `target` matches this tree, whatever the tests say, and the cache
# (saved also when a test fails, ci/checks.yml) stays warm for the next run. Its messages name the
# test executable of review-app's binary: the artifact whose profile has "test":true.
# Uploading its two archives took 120 to 137 s [observed 2026-11-26, pipelines 4905, 4906].
built="$(mktemp)"
start="$(date +%s)"
cargo test --locked --workspace --no-run --message-format=json-render-diagnostics > "$built"
sh ci/target-stamp.sh save "$(($(date +%s) - start))" 130
app_exe="$(grep '"name":"CodeView"' "$built" | grep '"overflow_checks":[a-z]*,"test":true}' |
    sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | head -n 1)"
rm -f "$built"
[ -x "$app_exe" ] || { echo "test: no test executable of review-app's binary" >&2; exit 1; }
app='binary_id(review-app::bin/CodeView)'
cargo nextest run --locked --workspace --profile ci --no-tests=pass -E "($filter) - $app"
if [ -n "$(cargo nextest list --locked --workspace -T oneline -E "($filter) & $app")" ]; then
    # As cargo test runs it: in the package's folder, one thread per CPU. `cargo test --bin`
    # cannot: it resolves the dev-dependency features of that target alone and rebuilt 17
    # workspace crates [observed 2026-11-26, local].
    (cd crates/review-app && "$app_exe")
else
    echo "test: the change needs no test of review-app"
fi
# nextest runs no doctests.
cargo test --locked --workspace --doc
