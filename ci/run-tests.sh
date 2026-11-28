#!/bin/sh
# Builds every test of the workspace and runs those a nextest filterset selects, the one
# implementation of the test run for both CIs (ci/test.sh) and the developer's gate
# (`sh scripts/check.sh test`); `sh ci/run-tests.sh 'all()'` runs every test of the workspace.
# Usage: run-tests.sh [--profile <nextest profile>] [--stamp <upload seconds>] <filterset>
#   --profile  the nextest profile (.config/nextest.toml); CI passes `ci`
#   --stamp    once the build passed, records it for the build cache with the seconds it took and
#              the seconds an upload costs (ci/target-stamp.sh save), before any test runs: then
#              `target` matches the tree whatever the tests say, and the cache stays warm
# The build stays the whole workspace, so dependencies keep the feature set of `--workspace` and a
# warm build cache rebuilds only the changed crates and their dependants (docs/release.md, Tests).
# review-app's walkthroughs run in one process as cargo test runs them: under nextest's process per
# test, four of them at once timed out twice in three runs and took 135 to 158 s against 117 s
# [observed 2026-11-26, pipelines 4900 to 4904]. Every other test binary runs under nextest, one
# process per test, so tests that share process-wide state cannot hide a coupling.
# Without cargo-nextest the run fails; it never falls back to another runner. Exit 1 = a build or a
# group failed (every group runs also after another failed), 2 = a usage error.
set -eu
profile=default
stamp=""
while [ "$#" -gt 1 ]; do
    case "$1" in
        --profile) profile="$2"; shift 2 ;;
        --stamp) stamp="$2"; shift 2 ;;
        *) break ;;
    esac
done
[ "$#" -eq 1 ] || {
    echo "usage: run-tests.sh [--profile <name>] [--stamp <upload seconds>] <filterset>" >&2
    exit 2
}
filter="$1"
if [ "$filter" = "none()" ]; then
    echo "test: no test reads the changed files"
    exit 0
fi
# The pinned cargo-nextest first on PATH: on a runner the one there, on a development machine the
# copy in the shared tools folder (ci/install-build-deps.sh), so a direct call works as well.
nextest="$(sh ci/install-build-deps.sh --path cargo-nextest)" || {
    echo "test: $nextest; install it with sh ci/install-build-deps.sh cargo-nextest" >&2
    exit 1
}
PATH="${nextest%/*}:$PATH"
start="$(date +%s)"
cargo test --locked --workspace --no-run
[ -z "$stamp" ] || sh ci/target-stamp.sh save "$(($(date +%s) - start))" "$stamp"
# The test executable of review-app's binary from nextest's list of the binaries, which builds
# nothing more after the build above. Each entry is a flat object under its binary id; a Windows
# path comes JSON-escaped and is turned into a forward-slash one.
app_exe="$(cargo nextest list --locked --workspace --list-type binaries-only --message-format json |
    sed -n 's/.*"review-app::bin\/CodeView":{[^}]*"binary-path":"\([^"]*\)".*/\1/p' |
    sed 's/\\\\/\//g')"
[ -x "$app_exe" ] || { echo "test: no test executable of review-app's binary" >&2; exit 1; }
app='binary_id(review-app::bin/CodeView)'
failed=""
cargo nextest run --locked --workspace --profile "$profile" --no-fail-fast --no-tests=pass \
    -E "($filter) - $app" || failed="$failed nextest"
# Assigned first, so that a failing listing stops the run (set -e) instead of skipping the tests.
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
