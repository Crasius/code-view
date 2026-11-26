#!/bin/sh
# check-repo-lint: the repository rules that clippy cannot check (tools/repo-lint, rules
# RL001.. in tools/repo-lint/README.md). repo-check.sh runs it with every other check-*.sh.
# Arguments are passed on: paths limit the run, --base <rev> sets the allowlist base.
# Exit 0 = clean, 1 = findings or the tool could not run (the reason is printed).
# POSIX sh; works from any directory inside the repository.

root=$(git rev-parse --show-toplevel 2>/dev/null) || {
  echo "check-repo-lint: not inside a git work tree"
  exit 1
}
cd "$root" || exit 1
if ! command -v cargo >/dev/null 2>&1; then
  echo "check-repo-lint: cargo not found. Install rustup; rust-toolchain.toml picks the toolchain."
  exit 1
fi
cargo run --quiet --locked -p repo-lint -- "$@"
status=$?
case $status in
  0) exit 0 ;;
  1) exit 1 ;;
  *)
    echo "check-repo-lint: repo-lint did not run (exit $status). Run cargo build -p repo-lint for the error."
    exit 1 ;;
esac
