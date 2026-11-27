#!/bin/sh
# The tests of a CI run (ci/checks.yml test; .github/workflows/pull-request.yml test and macos):
# builds every test of the workspace and runs those the change needs. TEST_FILTER names them as a
# nextest filterset; without it a merge request's filter comes from scripts/affected-tests.sh,
# compared with the request's base, and every other pipeline (a push to main, a tag, a run by hand)
# runs every test (docs/release.md, Tests). The build stays the whole workspace, so a warm cache
# rebuilds only the changed crates and their dependants (ci/target-stamp.sh).
# review-app's walkthroughs run in one process with cargo test, as they always ran: under nextest's
# process per test, four of them at once timed out twice in three runs and took 135 to 158 s
# against 117 s [observed 2026-11-26, pipelines 4900 to 4904]. Every other test binary runs under
# nextest.
set -eu
filter="${TEST_FILTER:-}"
if [ -z "$filter" ]; then
    if [ -n "${CI_MERGE_REQUEST_DIFF_BASE_SHA:-}" ]; then
        filter="$(sh scripts/affected-tests.sh "$CI_MERGE_REQUEST_DIFF_BASE_SHA")"
    else
        filter="all()"
    fi
fi
if [ "$filter" = "none()" ]; then
    echo "test: no test reads the changed files"
    exit 0
fi
# The build first: once it passed, `target` matches this tree, whatever the tests say, and the cache
# (saved also when a test fails, ci/checks.yml) stays warm for the next run.
# Uploading its two archives took 120 to 137 s [observed 2026-11-26, pipelines 4905, 4906].
start="$(date +%s)"
cargo test --locked --workspace --no-run
sh ci/target-stamp.sh save "$(($(date +%s) - start))" 130
# The test executable of review-app's binary from nextest's list of the binaries, which builds
# nothing more after the build above. Each entry is a flat object under its binary id.
app_exe="$(cargo nextest list --locked --workspace --list-type binaries-only --message-format json |
    sed -n 's/.*"review-app::bin\/CodeView":{[^}]*"binary-path":"\([^"]*\)".*/\1/p')"
[ -x "$app_exe" ] || { echo "test: no test executable of review-app's binary" >&2; exit 1; }
app='binary_id(review-app::bin/CodeView)'
# Every group runs also after another failed, so that one run lists every failure; the job fails at
# the end.
failed=""
cargo nextest run --locked --workspace --profile ci --no-tests=pass -E "($filter) - $app" ||
    failed="$failed nextest"
# Assigned first, so that a failing listing stops the job (set -e) instead of skipping the tests.
app_tests="$(cargo nextest list --locked --workspace -T oneline -E "($filter) & $app")"
if [ -n "$app_tests" ]; then
    # As cargo test runs it: in the package's folder, one thread per CPU. `cargo test --bin`
    # cannot: it resolves the dev-dependency features of that target alone and rebuilt 17
    # workspace crates [observed 2026-11-26, local].
    (cd crates/review-app && "$app_exe") || failed="$failed review-app"
else
    echo "test: the change needs no test of review-app"
fi
[ -z "$failed" ] || { echo "test: failed:$failed" >&2; exit 1; }
