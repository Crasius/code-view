#!/bin/sh
# Decides what a push to main checks and writes the configuration of its child pipeline
# (.gitlab-ci.yml, main-checks; docs/release.md, Pipelines). The checks run again unless the
# pushed commit is a merge GitLab made from a merge request ("See merge request <project>!<iid>"
# in its message) whose tree is the tree of its second parent, that request's head. GitLab merges
# a request only after the pipeline of its head passed (project setting "Pipelines must succeed"),
# so that tree is checked and the child runs one short job that names the request. In every other
# case, main moved between that pipeline and the merge, a local merge or another push, or any git
# error, the child runs the checks of ci/checks.yml. A rule cannot compare trees, so this job
# decides at run time and the trigger job starts the child from the file it writes.
# Usage: main-scope.sh <output yml>   (needs the commit and its parents: GIT_DEPTH 2)
set -eu
out="${1:?Name the output file.}"
head="$(git rev-parse HEAD)"

checked_by=""
if second="$(git rev-parse -q --verify 'HEAD^2^{commit}')" &&
    [ "$(git rev-parse 'HEAD^{tree}')" = "$(git rev-parse 'HEAD^2^{tree}')" ]; then
    # The request GitLab names in the merge commit, if its head ref is the second parent;
    # refs/merge-requests/<iid>/head stays on that commit after the merge.
    iid="$(git log -1 --format=%B HEAD |
        sed -n "s|^See merge request ${CI_PROJECT_PATH:?}!\([0-9][0-9]*\)\$|\1|p" | tail -n 1)"
    if [ -n "$iid" ] && refs="$(git ls-remote origin "refs/merge-requests/$iid/head" 2> /dev/null)" &&
        [ "$(printf '%s\n' "$refs" | awk '{ print $1; exit }')" = "$second" ]; then
        checked_by="!$iid"
    fi
fi

# Either configuration runs only as main's child pipeline.
cat > "$out" << EOF
# ci/main-scope.sh for $head.
workflow:
  rules:
    - if: \$CI_PIPELINE_SOURCE == "parent_pipeline"
stages: [test]
EOF
if [ -n "$checked_by" ]; then
    echo "main-scope: $head has the tree of $second, the head of merge request $checked_by, whose pipeline passed; no check runs again"
    cat >> "$out" << EOF
# The tree of merge request $checked_by's head $second, which its pipeline checked.
merge-request-checked:
  stage: test
  image: $ZIGBUILD_IMAGE
  tags: [build]
  interruptible: true
  variables:
    GIT_STRATEGY: none
  script:
    - echo "$head has the tree of $second, the head of merge request $checked_by; its pipeline checked it"
EOF
else
    echo "main-scope: $head has a tree no merge request pipeline passed; the child runs the checks"
    cat >> "$out" << EOF
# A tree no merge request pipeline passed: the checks.
include:
  - local: ci/checks.yml
variables:
  # The commit before the push, which ci/repo-lint-base.sh compares with; a child pipeline has its own.
  PUSH_BEFORE_SHA: "${CI_COMMIT_BEFORE_SHA:-}"
EOF
fi
cat "$out"
