#!/bin/sh
# The script of the measurements job (ci/checks.yml): review-app's heap measurements, which compile
# only with its feature `measurements` (rule `testing`, Running; module doc of
# crates/review-app/src/app/memory_walkthrough.rs). The build is test's (`--workspace`) plus that
# feature, which adds no dependency, so on test's cache only review-app compiles again; `-p
# review-app` would resolve the features of that package alone and rebuild dependencies. The
# executable runs as ci/test.sh runs it: in the package's folder. lint checks the group's lints
# (ci/lint.sh).
set -eu
built="$(mktemp)"
cargo test --locked --workspace --features review-app/measurements --no-run \
    --message-format=json-render-diagnostics > "$built"
app_exe="$(grep '"name":"CodeView"' "$built" | grep '"overflow_checks":[a-z]*,"test":true}' |
    sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | head -n 1)"
rm -f "$built"
[ -x "$app_exe" ] || { echo "measurements: no test executable of review-app's binary" >&2; exit 1; }
cd crates/review-app
"$app_exe" app::memory_walkthrough::memory_ --nocapture
