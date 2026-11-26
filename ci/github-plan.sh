#!/bin/sh
# The plan of .github/workflows/pull-request.yml: decides from the changed paths which jobs a pull
# request needs and writes them to $GITHUB_OUTPUT. A pull request's checkout is GitHub's merge
# commit, so HEAD^1 is the base branch and the diff is exactly the pull request's change.
#   rust   lint and lint-apple: a cargo input changed (the list of .rust-inputs in ci/checks.yml)
#   tests  the nextest filterset of test (scripts/affected-tests.sh); test: whether it selects any
#   macos  macos: the selection reaches a crate with macOS code. scripts/affected-tests.sh selects
#          every test when a script the job runs changes (ci/, the workflow, the lockfile, ...), and
#          the crates that depend on a changed one, dev-dependencies included.
#   mac-tests  the part of tests that macos runs: the crates with macOS code
# Every other event (the cache warm-up, a manual run) gets everything.
# Usage: github-plan.sh <event name>
set -eu
out="${GITHUB_OUTPUT:?Run it in GitHub Actions.}"

# The crates with code under cfg(target_os = "macos") or macOS-only dependencies.
mac_crates="cv-base cv-editor cv-media cv-settings cv-shell review-app review-core review-store"
mac_filter="$(printf '%s\n' "$mac_crates" | tr ' ' '\n' |
    awk '{ printf "%spackage(=%s)", (NR > 1 ? " | " : ""), $1 }')"

if [ "${1:-}" != pull_request ]; then
    rust=true tests="all()" macos=true
else
    rust=false
    # Split at line ends only and expand no glob: a path may hold spaces, *, ? or [.
    nl='
'
    IFS="$nl"
    set -f
    for f in $(git -c core.quotePath=false diff --name-only --no-renames HEAD^1 HEAD); do
        case "$f" in
            crates/* | tools/* | browser-extension/* | .cargo/* | Cargo.toml | Cargo.lock | \
                rust-toolchain.toml | clippy.toml | rustfmt.toml | .gitattributes | .github/workflows/* | \
                ci/* | .agents/hooks/* | .config/* | scripts/affected-tests.sh)
                rust=true
                ;;
        esac
    done
    set +f
    unset IFS
    tests="$(sh scripts/affected-tests.sh "$(git rev-parse HEAD^1)")"
    [ "$rust" = true ] || tests="none()"
    macos=false
    case "$tests" in
        "all()") macos=true ;;
        *)
            for crate in $mac_crates; do
                case "$tests" in *"package(=$crate)"*) macos=true ;; esac
            done
            ;;
    esac
fi

{
    echo "rust=$rust"
    echo "tests=$tests"
    if [ "$tests" = "none()" ]; then echo "test=false"; else echo "test=true"; fi
    echo "macos=$macos"
    echo "mac-tests=($tests) & ($mac_filter)"
} | tee -a "$out"
