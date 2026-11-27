#!/bin/sh
# Prints the cargo-nextest filterset of the tests a change needs: the packages that own a changed
# file, the packages whose Rust sources name a changed file (include_bytes! across crates, paths
# read at run time), and every package that depends on one of them through any kind of dependency,
# dev-dependencies included. Its callers: both CIs (ci/test.sh, ci/github-plan.sh), which build
# `--workspace` and filter only the run, so dependencies keep the feature set of `--workspace` and
# a warm build cache stays valid (docs/release.md, Tests); and the step `test` of scripts/check.sh,
# which turns the filterset into `cargo test -p` flags (rule testing, Running). A developer runs
# `sh scripts/check.sh test [--base <rev>]`.
# It prints `all()` when a shared input changed (the lockfile, the root manifest, the toolchain,
# cargo, git or CI configuration, cv-testkit, a build script) or the base is unknown, and `none()`
# when only files no test reads changed (docs, agent rules, marketing). Otherwise the filterset is
# `package(=a) | package(=b) ...`, one term per package, so a caller can intersect it with a list of
# packages (ci/github-plan.sh).
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
# Paths as they are (no octal quoting); lists split at line ends only, and no glob expands a path
# that holds *, ? or [.
changed="$(git -c core.quotePath=false diff --name-only --no-renames "$base" "$head")"
[ "$(printf '%s\n' "$changed" | grep -c .)" -le 300 ] || all "more than 300 changed files"
nl='
'
IFS="$nl"
set -f

# cargo metadata of the workspace members, one token per line: a package's name, the path
# dependencies it lists (any kind), its manifest.
root="$(git rev-parse --show-toplevel)"
tokens="$(cargo metadata --no-deps --format-version 1 --locked |
    grep -o '"name":"[^"]*","version"\|"name":"[^"]*","source":null\|"manifest_path":"[^"]*"' |
    sed -e 's/\\\\/\//g')"
# "<dir relative to the root> <package>" per member.
members="$(printf '%s\n' "$tokens" | awk -v root="$root/" '
    /","version"$/ { split($0, a, "\""); name = a[4]; next }
    /^"manifest_path"/ { split($0, a, "\""); dir = a[4]; sub(/\/Cargo\.toml$/, "", dir)
      if (index(dir, root) == 1) dir = substr(dir, length(root) + 1)
      print dir, name }')"
[ -n "$members" ] || all "cargo metadata listed no workspace member"
# "<dependency> <dependant>" per path dependency.
edges="$(printf '%s\n' "$tokens" | awk '
    /","version"$/ { split($0, a, "\""); name = a[4]; next }
    /","source":null$/ { split($0, a, "\""); print a[4], name }')"

# The package that owns <path>: the member with the longest directory that contains it.
owner() {
    printf '%s\n' "$members" | awk -v f="$1" '
        index(f, $1 "/") == 1 && length($1) > best { best = length($1); name = $2 }
        END { if (name != "") print name }'
}

packages=""
for f in $changed; do
    case "$f" in
        Cargo.lock | Cargo.toml | rust-toolchain.toml | .cargo/* | .config/* | .gitattributes | \
            .gitlab-ci.yml | ci/* | .github/workflows/* | scripts/affected-tests.sh | crates/cv-testkit/* | \
            */build.rs)
            all "$f is a shared input"
            ;;
        docs/* | marketing/* | .agents/rules/* | .agents/templates/* | .claude/* | .github/ISSUE_TEMPLATE/* | \
            .github/PULL_REQUEST_TEMPLATE.md | AGENTS.md | CLAUDE.md | README.md | CONTRIBUTING.md | \
            CLA.md | SECURITY.md | LICENSE | .gitignore | .gitleaksignore | rustfmt.toml | clippy.toml | \
            deny.toml | .agents/hooks/* | scripts/check.sh | scripts/worktree.sh | scripts/land-pr.sh | \
            scripts/start-background.ps1 | scripts/ship.ps1 | scripts/package-macos.sh | scripts/bundle-macos.sh | \
            browser-extension/README.md)
            # Read by no test; the lint job checks format, lints and repository rules.
            continue
            ;;
    esac
    found="$(owner "$f")"
    # Sources that name the file from elsewhere, with or without the leading crates/.
    for hit in $(git -c core.quotePath=false grep -l -F -e "$f" -e "${f#crates/}" "$head" -- 'crates/*.rs' 'tools/*.rs' || true); do
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
# The packages and everything that depends on them, to the fixed point.
{
    printf '%s\n' "$packages" | sed 's/^/+ /'
    printf '%s\n' "$edges"
} | awk '
    $1 == "+" { hit[$2] = 1; next }
    { dep[NR] = $1; user[NR] = $2 }
    END {
        # `in` tests membership without creating the element, as hit[x] would.
        do {
            grew = 0
            for (i in dep) if ((dep[i] in hit) && !(user[i] in hit)) { hit[user[i]] = 1; grew = 1 }
        } while (grew)
        for (p in hit) print p
    }' | sort | awk '{ printf "%spackage(=%s)", (NR > 1 ? " | " : ""), $1 } END { print "" }'
