#!/bin/sh
# Prints the cargo-nextest filterset of the tests a change needs: the packages that own a changed
# file, the packages whose Rust sources name a changed file (include_bytes! across crates, paths
# read at run time), and every package that depends on one of them (rdeps). Both CIs call it
# (.gitlab-ci.yml, .github/workflows/pull-request.yml) and so can a developer:
#   cargo nextest run --workspace -E "$(sh scripts/affected-tests.sh main)"
# The build stays the whole workspace: only the run is filtered, so dependencies keep the feature
# set of `--workspace` and a warm build cache stays valid (docs/release.md, Tests).
# It prints `all()` when a shared input changed (the lockfile, the root manifest, the toolchain,
# cargo or CI configuration, cv-testkit, a build script) or the base is unknown, and `none()` when
# only files no test reads changed (docs, agent rules, marketing).
# Usage: affected-tests.sh <base commit> [<head commit, default HEAD>]   (run in the repository)
set -eu
base="${1:-}"
head="${2:-HEAD}"

all() {
    echo "affected-tests: all tests: $1" >&2
    echo "all()"
    exit 0
}

case "$base" in
    '' | 0000000000000000000000000000000000000000) all "no base commit" ;;
esac
git cat-file -e "$base^{commit}" 2> /dev/null || all "the base $base is not in the clone"
changed="$(git diff --name-only --no-renames "$base" "$head")"
[ "$(printf '%s\n' "$changed" | grep -c .)" -le 300 ] || all "more than 300 changed files"

# The workspace members as "<dir relative to the root> <package>", from cargo metadata.
root="$(git rev-parse --show-toplevel)"
members="$(cargo metadata --no-deps --format-version 1 --locked |
    grep -o '"name":"[^"]*","version"\|"manifest_path":"[^"]*"' |
    sed -e 's/\\\\/\//g' |
    awk -v root="$root/" '
        /^"name"/ { split($0, a, "\""); name = a[4]; next }
        { split($0, a, "\""); dir = a[4]; sub(/\/Cargo\.toml$/, "", dir)
          if (index(dir, root) == 1) dir = substr(dir, length(root) + 1)
          print dir, name }')"
[ -n "$members" ] || all "cargo metadata listed no workspace member"

# The package that owns <path>: the member with the longest directory that contains it.
owner() {
    printf '%s\n' "$members" | awk -v f="$1" '
        index(f, $1 "/") == 1 && length($1) > best { best = length($1); name = $2 }
        END { if (name != "") print name }'
}

# Paths may hold spaces; split lists at line ends only.
nl='
'
IFS="$nl"
packages=""
for f in $changed; do
    case "$f" in
        Cargo.lock | Cargo.toml | rust-toolchain.toml | .cargo/* | .config/* | .gitlab-ci.yml | ci/* | \
            .github/workflows/* | scripts/affected-tests.sh | crates/cv-testkit/* | */build.rs)
            all "$f is a shared input"
            ;;
        docs/* | marketing/* | .agents/rules/* | .agents/templates/* | .claude/* | .github/ISSUE_TEMPLATE/* | \
            .github/pull_request_template.md | AGENTS.md | CLAUDE.md | README.md | CONTRIBUTING.md | \
            CLA.md | SECURITY.md | LICENSE | .gitignore | .gitleaksignore | rustfmt.toml | clippy.toml | \
            .agents/hooks/*)
            # Read by no test; the lint job checks format, lints and repository rules.
            continue
            ;;
    esac
    found="$(owner "$f")"
    # Sources that name the file from elsewhere, with or without the leading crates/.
    for hit in $(git grep -l -F -e "$f" -e "${f#crates/}" "$head" -- 'crates/*.rs' 'tools/*.rs' || true); do
        found="$found$nl$(owner "${hit#"$head":}")"
    done
    [ -n "$(printf '%s' "$found" | tr -d '\n')" ] || all "$f belongs to no package and no source names it"
    packages="$packages$nl$found"
done

packages="$(printf '%s\n' "$packages" | sort -u | grep . || true)"
if [ -z "$packages" ]; then
    echo "affected-tests: no test reads the changed files" >&2
    echo "none()"
    exit 0
fi
echo "affected-tests: the tests of $(printf '%s' "$packages" | tr '\n' ' ') and their dependants" >&2
printf '%s\n' "$packages" | awk '{ printf "%srdeps(=%s)", (NR > 1 ? " | " : ""), $1 } END { print "" }'
