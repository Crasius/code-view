#!/bin/sh
# repo-check: repository hygiene gate. Prints one line per finding.
# Exit 0 = clean, 1 = findings.
#   1. Untracked, not-ignored junk files outside tmp/ and runtime/ (screenshots, logs, merge leftovers).
#      Why: they get swept into a commit or hide in the tree. Keep captures in tmp/.
#   2. Tracked files with secret-like names. Why: a committed secret stays in history.
#   3. Every .agents/hooks/check-*.sh (0 = ok, 1 = finding).
# POSIX sh; works from any directory inside the repository.

root=$(git rev-parse --show-toplevel 2>/dev/null) || {
  echo "repo-check: not inside a git work tree"
  exit 1
}
cd "$root" || exit 1
found=0

junk=$(git -c core.quotepath=off ls-files --others --exclude-standard | while IFS= read -r f; do
  case $f in
    tmp/*|runtime/*) continue ;;
  esac
  case ${f##*/} in
    *.png|*.jpg|*.webm|*.stackdump|*.log|*.orig|*.rej|nul)
      printf 'repo-check: untracked junk file %s. Move it to tmp/, delete it, or git add it when it belongs to the project.\n' "$f" ;;
  esac
done)
if [ -n "$junk" ]; then
  printf '%s\n' "$junk"
  found=1
fi

secrets=$(git -c core.quotepath=off ls-files | while IFS= read -r f; do
  hit=0
  case $f in
    .secrets/*|*/.secrets/*) hit=1 ;;
    *)
      case ${f##*/} in
        *.example|*.example.*) ;;
        .env|.env.*|*.key|*.pfx|*.p12|*-key.txt|credentials*.json|token.json|*-token.json|*_token.json) hit=1 ;;
      esac ;;
  esac
  if [ "$hit" -eq 1 ]; then
    printf 'repo-check: tracked secret-like file %s. Run git rm --cached on it, keep it ignored, and tell the owner to rotate the secret.\n' "$f"
  fi
done)
if [ -n "$secrets" ]; then
  printf '%s\n' "$secrets"
  found=1
fi

for check in .agents/hooks/check-*.sh; do
  [ -f "$check" ] || continue
  if ! sh "$check"; then
    echo "repo-check: $check reported findings"
    found=1
  fi
done

exit "$found"
