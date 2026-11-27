#!/bin/sh
# The script of the lint job of both CIs (ci/checks.yml lint, .github/workflows/pull-request.yml
# lint): the default steps of scripts/check.sh (format, hygiene, repository rules, the clippy
# passes, the dependency bans and sources), then its CI-only step shear (unused dependencies). The
# job installs cargo-deny and cargo-shear first; LINT_BASE names repo-lint's base
# (ci/repo-lint-base.sh).
# Uploading the job's cache took about 35 s [observed 2026-11-26, pipelines 4901 to 4906]; a
# shorter clippy run keeps the cache as it is (ci/target-stamp.sh).
set -eu
seconds="$(mktemp)"
failed=0
CHECK_BUILD_SECONDS="$seconds" sh scripts/check.sh || failed=1
sh scripts/check.sh shear || failed=1
[ "$failed" = 0 ] || exit 1
sh ci/target-stamp.sh save "$(cat "$seconds")" 35
rm -f "$seconds"
