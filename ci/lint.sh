#!/bin/sh
# The script of the lint job (ci/checks.yml): format, repository rules, both clippy passes and the
# lints of the measurements. The two passes differ: --all-targets lints the libraries with test-support on, the second pass lints the
# shipped variant, the only one that sees cfg(not(any(test, feature = "test-support"))).
# Uploading the job's cache took about 35 s [observed 2026-11-26, pipelines 4901 to 4906]; a
# shorter clippy run keeps the cache as it is (ci/target-stamp.sh).
set -eu
cargo fmt --all --check
sh .agents/hooks/repo-check.sh
sh ci/repo-lint-base.sh
start="$(date +%s)"
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo clippy --locked --workspace -- -D warnings
# The measurement group of review-app (ci/measurements.sh): its code compiles only with the feature.
# With --workspace the other crates keep the features of the first pass, so only review-app is
# checked again.
cargo clippy --locked --workspace --all-targets --features review-app/measurements -- -D warnings
sh ci/target-stamp.sh save "$(($(date +%s) - start))" 35
