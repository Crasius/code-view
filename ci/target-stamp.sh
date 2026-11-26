#!/bin/sh
# Makes a cached `target` folder usable after a fresh checkout (docs/release.md, Build cache).
# cargo decides by modification time whether a workspace crate is fresh, and a fresh clone stamps
# every file with the clone's time, so without this every workspace crate builds again.
#   save     after a passing build: records the tree it built (`git ls-files -s`) in target/.
#   restore  after the checkout and the cache restore: dates every tracked file and its folders
#            back to 2001, then touches every path whose blob differs from the recorded tree, and
#            its folders. cargo then rebuilds exactly the crates whose inputs changed, and what
#            depends on them. Without a record (a cold cache) it changes nothing. It removes the
#            record: a build that fails halfway leaves a `target` no recorded tree describes, and
#            a cache saved from it makes the next job date nothing back.
# Content, not commit dates, decides: a cache from another branch or an older commit is as safe as
# one from the parent commit. Files outside git (registry sources, OUT_DIR) are cargo's to judge.
#   save <build seconds> <upload seconds>  as save, and marks the cache to stay as it is when the
#            build was the cheaper of the two; drop-if-cheap then removes the marked folders.
# Usage: target-stamp.sh save [<s> <s>]|restore|drop-if-cheap   (in the repository root)
set -eu
record=target/ci-tree.txt
old=@978307200

# Every folder that holds one of the paths on stdin, each once, one per line.
folders() {
    awk -F / '{ p = ""; for (i = 1; i < NF; i++) { p = (i > 1 ? p "/" : "") $i; print p } }' | sort -u
}

# Runs touch on the paths on stdin, one per line.
touch_lines() {
    tr '\n' '\0' | xargs -0 -r touch -h -c "$@"
}

case "${1:-}" in
    save)
        mkdir -p target
        git ls-files -s > "$record"
        echo "target-stamp: recorded the tree of $(git rev-parse --short HEAD)"
        # A build shorter than the upload of its cache would take saves the next job less than the
        # upload costs this one: mark the cache to stay as it is (drop-if-cheap).
        if [ -n "${2:-}" ] && [ "$2" -lt "${3:?Name the seconds an upload costs.}" ]; then
            : > target/ci-keep-cache
            echo "target-stamp: the build took $2 s, less than the $3 s an upload costs; the cache stays"
        fi
        ;;
    drop-if-cheap)
        # In after_script: removes the cached folders of a marked job, so the runner finds nothing
        # to archive and the cache on the server stays as it was.
        if [ -f target/ci-keep-cache ]; then
            rm -rf target/debug "$record" target/ci-keep-cache
            echo "target-stamp: the cache stays as it was"
        fi
        ;;
    restore)
        if [ ! -s "$record" ]; then
            echo "target-stamp: no recorded tree, a cold build"
            exit 0
        fi
        now="$(mktemp)"
        git ls-files -s > "$now"
        git ls-files | touch_lines -d "$old"
        git ls-files | folders | touch_lines -d "$old"
        # The lines of either list that the other lacks: changed, added and removed paths.
        changed="$(sort "$record" "$now" | uniq -u | cut -f 2- | sort -u)"
        rm -f "$now" "$record"
        if [ -n "$changed" ]; then
            printf '%s\n' "$changed" | touch_lines
            printf '%s\n' "$changed" | folders | touch_lines
        fi
        echo "target-stamp: $(printf '%s' "$changed" | grep -c . || true) paths differ from the cached build"
        ;;
    *)
        echo "usage: target-stamp.sh save [<build s> <upload s>]|restore|drop-if-cheap" >&2
        exit 2
        ;;
esac
