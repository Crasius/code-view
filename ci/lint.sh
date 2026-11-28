#!/bin/sh
# The script of the lint job of both CIs (ci/checks.yml lint, .github/workflows/pull-request.yml
# lint): the default steps of scripts/check.sh (`--list` names them), shear among them, the same
# command a developer runs. The job installs cargo-deny and cargo-shear first; LINT_BASE names
# repo-lint's base (ci/repo-lint-base.sh).
# On GitLab, uploading the job's cache took about 35 s [observed 2026-11-26, pipelines 4901 to
# 4906]; a shorter clippy run leaves GitLab's cache as it is (rustdoc runs in every job:
# target/doc is not cached; drop-if-cheap in ci/checks.yml's after_script, ci/target-stamp.sh).
# GitHub's manual run saves the cache every time.
set -eu
seconds="$(mktemp)"
CHECK_BUILD_SECONDS="$seconds" sh scripts/check.sh
sh ci/target-stamp.sh save "$(cat "$seconds")" 35
rm -f "$seconds"
