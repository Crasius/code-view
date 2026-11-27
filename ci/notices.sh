#!/bin/sh
# Writes the license notices of the crates that link on the given targets (docs/release.md,
# Third-party notices). cargo-about reads `cargo metadata` and compiles nothing; a crate under a
# license outside `accepted` in ci/about.toml stops it. ci/build-release.sh calls it for the notices
# of a build, and the `licenses` step of scripts/check.sh for all six shipped targets.
# Usage: notices.sh <out file> --target <triple> [--target <triple>]...
set -eu
out="${1:?Name the file to write.}"
shift
mkdir -p "$(dirname "$out")"
cargo-about generate --locked --config ci/about.toml --manifest-path crates/review-app/Cargo.toml "$@" \
    -o "$out" ci/about.hbs
