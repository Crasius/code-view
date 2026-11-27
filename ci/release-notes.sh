#!/bin/sh
# Prints Markdown release notes for a tag: the commit subjects since the previous tag. The first tag
# of the public history has no list, because the whole history is no change anyone could read; it
# is not the first release, since the tags up to v0.1.2 stayed in the private archive
# (docs/release.md, Public mirror). Needs the full history (GIT_DEPTH 0).
# Usage: release-notes.sh <tag>
set -eu
tag="${1:?Name the release tag.}"

git config --global --add safe.directory "$(pwd)"
previous="$(git describe --tags --abbrev=0 --match 'v*' "$tag^" 2> /dev/null || true)"

printf '# Code View %s\n\n' "$tag"
if [ -z "$previous" ]; then
    printf 'This is the first release built from the public repository, so it comes without a change list.\n'
    exit 0
fi
printf 'Changes since %s:\n\n' "$previous"
git log --no-merges --format='- %s' "$previous..$tag"
