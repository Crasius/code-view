#!/bin/sh
# check.sh: the one entry point for the repository's checks, for contributors, lanes and both CIs.
#
#   sh scripts/check.sh [--base <rev>]            the static gate: fmt hygiene repo-lint clippy deps
#   sh scripts/check.sh [--base <rev>] <step>...  only the named steps; also advisories,
#                                                 licenses, shear, test
#   sh scripts/check.sh --changed [--base <rev>]  rustfmt on the changed .rs files, clippy on their crates
#   sh scripts/check.sh --list                    every step with its commands
#
# Every named step runs, also after one failed; the summary names each step. Exit 1 = a step
# failed, 2 = a usage error. --base is repo-lint's allowlist base and the fork point `test` and
# --changed compare with; without it repo-lint and `test` take main when it resolves, else HEAD,
# and --changed looks at the uncommitted files only.
# A missing tool (cargo-deny, cargo-about, cargo-shear): without CI set the step prints SKIPPED
# with the install command and the summary lists it as NOT-VERIFIED, the exit code stays; with CI
# set the step fails. The versions come from ci/install-build-deps.sh, the one place that pins them.
# shear is not in the default list: cargo-shear is not installed on the machine whose local checks
# are the gate, so CI runs it by name (`sh scripts/check.sh shear`).
# CARGO_BUILD_JOBS limits cargo's jobs; the script passes no -j. CHECK_BUILD_SECONDS=<file>
# receives the seconds the clippy step took (ci/target-stamp.sh).
# --changed is what .agents/hooks/rust-stop-gate.sh (the Stop hook) and a lane's turn end run:
# exit 0 = clean (one line on stdout names what it checked), 2 = findings on stderr, 1 = clippy did
# not run (stderr says why).
# Why sh and not cargo aliases or an xtask crate: an alias is one cargo command, and a crate takes
# the build directory lock and must compile before the format check can start.
# POSIX sh; macOS, Linux and Windows (Git Bash).

set -u
nl='
'

usage_line='usage: sh scripts/check.sh [--base <rev>] [fmt|hygiene|repo-lint|clippy|deps|advisories|licenses|shear|test]... | --changed [--base <rev>] | --list'
usage() {
  printf 'check: %s\n%s\n' "$1" "$usage_line" >&2
  exit 2
}

default_steps="fmt hygiene repo-lint clippy deps"
mode=steps
base=""
steps=""
while [ $# -gt 0 ]; do
  case $1 in
    --base)
      [ $# -ge 2 ] || usage "--base needs a revision"
      base=$2
      shift 2 ;;
    --list) mode=list; shift ;;
    --changed) mode=changed; shift ;;
    -h | --help) printf '%s\n' "$usage_line"; exit 0 ;;
    fmt | hygiene | repo-lint | clippy | deps | advisories | licenses | shear | test) steps="$steps $1"; shift ;;
    *) usage "unknown step or option $1" ;;
  esac
done
[ "$mode" = steps ] || [ -z "$steps" ] || usage "--$mode takes no steps"

# CHECK_BUILD_SECONDS is written after the cd below, so a relative path is made absolute first.
case ${CHECK_BUILD_SECONDS:-} in
  '' | /* | [A-Za-z]:[/\\]*) ;;
  *) CHECK_BUILD_SECONDS=$PWD/$CHECK_BUILD_SECONDS ;;
esac
root=$(git rev-parse --show-toplevel 2>/dev/null) || usage "not inside a git work tree"
cd "$root" || exit 2

# The revision for repo-lint and `test`: --base, else main when it resolves, else HEAD. Printed as
# its name when that is plain, else as the commit id, so the eval of a command stays safe.
base_rev() {
  if [ -n "$base" ]; then
    id=$(git rev-parse --verify --quiet "$base^{commit}") || {
      printf 'check: --base %s is not a commit\n' "$base" >&2
      return 1
    }
    case $base in
      *[!A-Za-z0-9._/-]*) printf '%s\n' "$id" ;;
      *) printf '%s\n' "$base" ;;
    esac
  elif git rev-parse --verify --quiet 'main^{commit}' >/dev/null; then
    echo main
  else
    echo HEAD
  fi
}

# The tool a step needs beyond the toolchain; empty for none.
step_tool() {
  case $1 in
    deps | advisories) echo cargo-deny ;;
    licenses) echo cargo-about ;;
    shear) echo cargo-shear ;;
  esac
}

# The version= line of <tool>'s entry in ci/install-build-deps.sh.
tool_version() {
  awk -v entry="$1)" '
    $1 == entry { inside = 1; next }
    inside && $1 ~ /^version=/ { sub(/^version=/, "", $1); print $1; exit }
    inside && $1 == ";;" { exit }' ci/install-build-deps.sh
}

# The commands of a step, one per line.
step_commands() {
  case $1 in
    fmt) echo 'cargo fmt --all --check' ;;
    hygiene) echo 'sh .agents/hooks/repo-check.sh' ;;
    repo-lint)
      if [ -n "${CI:-}" ]; then
        echo 'sh ci/repo-lint-base.sh'
      else
        rev=$(base_rev) || return 1
        echo "cargo run -q --locked -p repo-lint -- --base $rev"
      fi ;;
    clippy)
      # --all-targets lints the libraries with test-support on; the second pass lints the shipped
      # variant, the only one that sees cfg(not(any(test, feature = "test-support"))). The third
      # pass is review-app's measurement group (ci/measurements.sh), whose code compiles only with
      # the feature; with --workspace the other crates keep the features of the first pass, so only
      # review-app is checked again.
      echo 'cargo clippy --locked --workspace --all-targets -- -D warnings'
      echo 'cargo clippy --locked --workspace -- -D warnings'
      echo 'cargo clippy --locked --workspace --all-targets --features review-app/measurements -- -D warnings' ;;
    # Without the inclusion graphs: the duplicate-version warnings alone print 12,000 lines of them
    # on a passing run. `cargo deny check bans` by hand shows who pulls a crate in.
    deps) echo 'cargo deny --locked check --hide-inclusion-graph bans sources' ;;
    advisories) echo 'cargo deny --locked check --hide-inclusion-graph advisories' ;;
    # The release build's notices for all six shipped targets at once, so that a licence outside
    # ci/about.toml fails before the release tag; the file itself is scratch.
    licenses) echo 'sh ci/notices.sh target/check-notices.txt --target x86_64-pc-windows-msvc --target aarch64-pc-windows-msvc --target x86_64-unknown-linux-gnu --target aarch64-unknown-linux-gnu --target aarch64-apple-darwin --target x86_64-apple-darwin' ;;
    shear) echo 'cargo shear' ;;
    test)
      rev=$(base_rev) || return 1
      fork=$(git merge-base "$rev" HEAD) || {
        printf 'check: no common ancestor of %s and HEAD\n' "$rev" >&2
        return 1
      }
      filter=$(sh scripts/affected-tests.sh "$fork" HEAD) || return 1
      case $filter in
        'none()') ;;
        'all()') echo 'cargo test --locked --no-fail-fast --workspace' ;;
        *)
          printf 'cargo test --locked --no-fail-fast'
          printf '%s\n' "$filter" | tr '|' '\n' | sed -n 's/.*package(=\([A-Za-z0-9_-]*\)).*/ -p \1/p' | tr -d '\n'
          echo ;;
      esac ;;
  esac
}

results=""
failed=0
record() {
  results="$results$(printf '%-11s %s' "$1" "$2")$nl"
}

run_step() {
  step=$1
  printf '\n== check: %s\n' "$step"
  tool=$(step_tool "$step")
  if [ -n "$tool" ] && ! command -v "$tool" >/dev/null 2>&1; then
    hint="cargo install --locked $tool@$(tool_version "$tool")"
    if [ -n "${CI:-}" ]; then
      printf 'check: %s is not installed; CI installs it with sh ci/install-build-deps.sh %s\n' "$tool" "$tool" >&2
      record "$step" "FAIL (not installed: $tool)"
      failed=1
    else
      printf 'SKIPPED (not installed: %s)\n' "$hint"
      record "$step" "NOT-VERIFIED (SKIPPED, not installed: $hint)"
    fi
    return
  fi
  cmds=$(step_commands "$step") || {
    record "$step" FAIL
    failed=1
    return
  }
  if [ -z "$cmds" ]; then
    echo "nothing to run: no test reads the changed files"
    record "$step" "PASS (nothing to run)"
    return
  fi
  started=$(date +%s)
  status=PASS
  set -f
  old_ifs=$IFS
  IFS=$nl
  for cmd in $cmds; do
    IFS=$old_ifs
    printf '$ %s\n' "$cmd"
    eval "$cmd" </dev/null || { status=FAIL; break; }
  done
  IFS=$old_ifs
  set +f
  if [ "$step" = clippy ] && [ -n "${CHECK_BUILD_SECONDS:-}" ]; then
    echo "$(($(date +%s) - started))" >"$CHECK_BUILD_SECONDS"
  fi
  [ "$status" = PASS ] || failed=1
  record "$step" "$status"
}

list() {
  printf 'Default steps (sh scripts/check.sh [--base <rev>]): %s\n' "$default_steps"
  for step in $default_steps advisories licenses shear; do
    case $step in
      advisories) printf '\nadvisories (by name; fetches the advisory database)\n' ;;
      licenses) printf '\nlicenses (by name; the licences of the six shipped targets)\n' ;;
      shear) printf '\nshear (CI only: not in the default list; run it by name)\n' ;;
      *) printf '\n%s\n' "$step" ;;
    esac
    tool=$(step_tool "$step")
    [ -z "$tool" ] || printf '  needs %s %s (sh ci/install-build-deps.sh %s)\n' "$tool" "$(tool_version "$tool")" "$tool"
    step_commands "$step" | sed 's/^/  /'
    if [ "$step" = repo-lint ] && [ -z "${CI:-}" ]; then
      printf '  (with CI set: sh ci/repo-lint-base.sh)\n'
    fi
  done
  printf '\ntest (by name)\n'
  printf '  cargo test --locked --no-fail-fast -p <package>... for the filterset of sh scripts/affected-tests.sh <fork point of --base> HEAD\n'
  printf '  (all() runs --workspace, none() runs nothing)\n'
  printf '\n--changed [--base <rev>]\n'
  printf '  rustfmt --check -l <changed .rs files outside vendor/>\n'
  printf '  cargo clippy -p <crate of each changed file>... --all-targets --message-format=short\n'
  printf '  (stopped, exit 1, when another cargo holds the build directory lock)\n'
}

# rustfmt on the changed .rs files and clippy on their crates, at the levels of [workspace.lints]
# (no -D warnings, so findings stay warnings and keep their file). Warnings count only in the
# changed files, because other files may still carry warnings another lane fixes; errors count in
# every file. Changed: differs from HEAD (or from the fork point of --base) in the work tree or the
# index, or untracked and not ignored; outside vendor/ folders (vendored code keeps its upstream
# style, rule rust, Dependencies).
changed() {
  since=HEAD
  if [ -n "$base" ]; then
    since=$(git merge-base "$base" HEAD 2>/dev/null) || {
      printf 'check --changed: no common ancestor of %s and HEAD; pass the base branch\n' "$base" >&2
      exit 1
    }
  fi

  files=$( {
    git -c core.quotepath=off diff --name-only "$since" -- '*.rs'
    git -c core.quotepath=off ls-files --others --exclude-standard -- '*.rs'
  } 2>/dev/null | sort -u | while IFS= read -r f; do
    case /$f in */vendor/*) continue ;; esac
    [ -f "$f" ] && printf '%s\n' "$f"
  done)
  if [ -z "$files" ]; then
    echo 'check --changed: clean, no changed .rs file'
    exit 0
  fi

  # The package name of the nearest Cargo.toml above each file; the virtual root has none.
  crates=$(printf '%s\n' "$files" | while IFS= read -r f; do
    dir=${f%/*}
    [ "$dir" = "$f" ] && dir=.
    while [ "$dir" != "." ] && [ ! -f "$dir/Cargo.toml" ]; do
      case $dir in */*) dir=${dir%/*} ;; *) dir=. ;; esac
    done
    awk '/^\[package\]/ { p = 1; next } /^\[/ { p = 0 }
         p && /^name[ \t]*=/ { sub(/^[^"]*"/, ""); sub(/".*/, ""); print; exit }' "$dir/Cargo.toml"
  done | sort -u)

  report=""
  skipped=""

  # rustfmt prints absolute paths (verbatim \\?\ ones on Windows); show them repo-relative.
  fmt=$(printf '%s\n' "$files" | tr '\n' '\0' | xargs -0 rustfmt --check -l 2>&1 |
    sed -e 's|^\\\\?\\||' -e 's|\\|/|g' -e "s|^$root/||")
  if [ -n "$fmt" ]; then
    report="rustfmt: these files are not formatted; run rustfmt --edition 2024 on them:
$fmt
"
  fi

  if [ -n "$crates" ]; then
    packages=$(printf '%s\n' "$crates" | sed 's/^/-p /' | tr '\n' ' ')
    log=$(mktemp 2>/dev/null) || log="${TMPDIR:-/tmp}/check-changed.$$"
    # One reader instead of a poll: awk copies cargo's output into the log and stops at cargo's
    # "Blocking waiting for file lock" line (hence no --quiet). The subshell starts cargo in the
    # background and ends at once, so the pipeline waits for awk alone, which ends with cargo's
    # output or at that line; nothing starts a process per round (rule windows-host). A cargo
    # that waits for a lock writes nothing more and never sees a broken pipe, so it is killed,
    # never waited for.
    # shellcheck disable=SC2086 # $packages is a list of -p flags
    (cargo clippy $packages --all-targets --message-format=short 2>&1 & echo $! >"$log.pid") |
      awk -v out="$log" '/Blocking waiting for file lock/ { print "locked" > (out ".lock"); exit }
                         { print > out }'
    if [ -f "$log.lock" ]; then
      read -r cargo_pid <"$log.pid" && kill "$cargo_pid" 2>/dev/null
      skipped="clippy did not run: another cargo job holds the build directory lock. Run cargo clippy $packages--all-targets -- -D warnings once it has finished."
    else
      relevant=$(CHANGED=$files awk '
        BEGIN { n = split(ENVIRON["CHANGED"], list, "\n"); for (i = 1; i <= n; i++) keep[list[i]] = 1 }
        {
          line = $0; gsub(/\\/, "/", line)
          path = line; sub(/:[0-9]+:[0-9]+: .*/, "", path)
          if (line ~ /^[^ ]+:[0-9]+:[0-9]+: error/) print line
          else if (path in keep && line ~ /^[^ ]+:[0-9]+:[0-9]+: warning/) print line
          else if (line ~ /^error(\[|:)/ && line !~ /could not compile/ && line !~ /aborting due to/) print line
        }' "$log" 2>/dev/null)
      if [ -n "$relevant" ]; then
        report="${report}clippy on $(printf '%s' "$crates" | tr '\n' ' '):
$relevant
"
      fi
    fi
    rm -f "$log" "$log.pid" "$log.lock"
  fi

  if [ -n "$report" ]; then
    printf 'check --changed: fix these before you finish (rule rust, Lints and Format):\n%s' "$report" >&2
    [ -n "$skipped" ] && printf '%s\n' "$skipped" >&2
    exit 2
  fi
  if [ -n "$skipped" ]; then
    printf 'check --changed: %s\n' "$skipped" >&2
    exit 1
  fi
  printf 'check --changed: clean, rustfmt on %s changed file(s), clippy on: %s\n' \
    "$(printf '%s\n' "$files" | wc -l | tr -d ' ')" "$(printf '%s' "${crates:-no crate}" | tr '\n' ' ')"
  exit 0
}

case $mode in
  list) list; exit 0 ;;
  changed) changed ;;
esac

for step in ${steps:-$default_steps}; do
  run_step "$step"
done
printf '\n== check: summary\n%s' "$results"
exit "$failed"
