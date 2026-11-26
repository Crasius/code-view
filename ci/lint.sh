#!/bin/sh
# The script of the lint job (ci/checks.yml): format, repository rules and both clippy passes.
# They differ: --all-targets lints the libraries with test-support on, the second pass lints the
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
sh ci/target-stamp.sh save "$(($(date +%s) - start))" 35
