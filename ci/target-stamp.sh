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
# Usage: target-stamp.sh save|restore   (in the repository root)
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
        echo "usage: target-stamp.sh save|restore" >&2
        exit 2
        ;;
esac
