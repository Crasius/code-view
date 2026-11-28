#!/bin/sh
# The script of the measurements job (ci/checks.yml): review-app's heap measurements, which compile
# only with its feature `measurements` (rule `testing`, Running; module doc of
# crates/review-app/src/app/memory_walkthrough.rs). The build is test's (`--workspace`) plus that
# feature, which adds no dependency, so on test's cache only review-app compiles again; `-p
# review-app` would resolve the features of that package alone and rebuild dependencies. The
# executable runs as ci/run-tests.sh runs it: in the package's folder. lint checks the group's lints
# (ci/lint.sh, the clippy step of scripts/check.sh).
# nextest's list of the binaries builds them and names the executable under its binary id, in a
# flat object (as ci/run-tests.sh picks it).
set -eu
app_exe="$(cargo nextest list --locked --workspace --features review-app/measurements \
    --list-type binaries-only --message-format json |
    sed -n 's/.*"review-app::bin\/CodeView":{[^}]*"binary-path":"\([^"]*\)".*/\1/p')"
[ -x "$app_exe" ] || { echo "measurements: no test executable of review-app's binary" >&2; exit 1; }
cd crates/review-app
"$app_exe" app::memory_walkthrough::memory_ --nocapture
