#!/bin/sh
# repo-check: repository hygiene gate. Prints one line per finding.
# Exit 0 = clean, 1 = findings.
#   1. Untracked, not-ignored junk files outside tmp/ and runtime/ (screenshots, logs, merge leftovers).
#      Why: they get swept into a commit or hide in the tree. Keep captures in tmp/.
#   2. Tracked files with secret-like names. Why: a committed secret stays in history.
#   3. What a push would publish: the commits on HEAD that are not on origin/main.
#      - A message that names a user folder (C:\Users\<name>, C:/Users/<name>, /c/Users/<name>,
#        /Users/<name>, /home/<name>, also behind file:/// and WSL's /mnt/c and //wsl$/<distro>)
#        whose name is no placeholder. Placeholders: a name that starts with < $ % { * ~ . or an
#        ellipsis, a name of digits only (API paths like /users/123), and you, me, user,
#        username, name, runner, runneradmin (GitHub's Windows runners), public, default,
#        shared, all (any case). Messages only: tracked files hold invented paths
#        (/home/ada/...) on purpose, a message never needs one.
#      - A message or an added line that holds this machine's user folder name or login name,
#        as a whole word in any case. The names come from USERPROFILE, HOME, USERNAME, USER and
#        LOGNAME at run time; names shorter than four characters and generic ones (user, admin,
#        root, runner, ...) are skipped. The finding never prints the name.
#      Local only: with CI set, or without origin/main, it does nothing and prints nothing. Why: a
#      pushed message cannot change without a history rewrite, and a contributor's pull request
#      must not fail on its messages. Why it exists: three unpushed messages quoted a log path
#      with the user folder, and 438 commits had to be rewritten before a push (rule git-flow).
#      Two processes (git log and awk), however long the range.
# repo-lint runs on its own, as the step repo-lint of scripts/check.sh.
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

# Check 3. The names are folded to one list here without starting a process; awk filters them.
if [ -z "${CI:-}" ]; then
  profile=${USERPROFILE:-}
  profile=${profile##*\\}
  profile=${profile##*/}
  home=${HOME:-}
  home=${home%/}
  home=${home##*/}
  own_names="$profile
$home
${USERNAME:-}
${USER:-}
${LOGNAME:-}"
  # git log -p shows no diff for a merge, so a merge contributes its message alone; the merged
  # commits are in the range themselves. Without origin/main git prints nothing (stderr dropped).
  published=$(git -c core.quotepath=off log -p -U0 --no-color --no-ext-diff --no-renames \
    --format='%x01%h%n%B%n%x02' refs/remotes/origin/main..HEAD -- 2>/dev/null |
    OWN_NAMES=$own_names LC_ALL=C awk '
      BEGIN {
        n = split(ENVIRON["OWN_NAMES"], raw, "\n")
        generic = " user users admin administrator root runner default public guest owner home build test vscode ubuntu "
        for (i = 1; i <= n; i++) {
          w = tolower(raw[i])
          if (length(w) < 4 || index(generic, " " w " ") > 0 || (w in seen)) continue
          seen[w] = 1
          names[++nn] = w
        }
        placeholder = " you me user username name runner runneradmin public default shared all "
        todo = "Reword the commit before it is merged: a sole writer whose HEAD is that commit may amend it with the shell comment # history-rewrite-ok: <reason> (rule git-flow, Always); otherwise stop and tell the owner."
      }
      function word(c) { return c ~ /[a-z0-9_]/ }
      function own_name(line,    l, i, p, from, at, w) {
        l = tolower(line)
        for (i = 1; i <= nn; i++) {
          w = names[i]
          from = 1
          while ((p = index(substr(l, from), w)) > 0) {
            at = from + p - 1
            # substr(l, 0, 1) is the first character in gawk and mawk, so test at == 1 first.
            if ((at == 1 || !word(substr(l, at - 1, 1))) && !word(substr(l, at + length(w), 1))) return 1
            from = at + 1
          }
        }
        return 0
      }
      # The first user folder path with a real-looking name, as "<prefix><name>" masked; "" for none.
      # A drive path may follow a slash (file:///C:/Users/...). A Unix path counts at the line
      # start, after a non-path character, or after file://, /mnt/<drive> (WSL) or
      # //wsl$/<distro>, so URL paths like example.com/home/x stay out.
      function user_folder(line,    l, from, at, rest, before, lead, prefix, name) {
        l = tolower(line)
        from = 1
        while (match(substr(l, from), /[a-z]:[\\\/]+users[\\\/]+|\/[a-z]\/users\/|\/users\/|\/home\//)) {
          at = from + RSTART - 1
          prefix = substr(line, at, RLENGTH)
          # One step on, not past the match: file:///Users/x first matches "e:///Users/".
          from = at + 1
          if (at > 1) {
            before = substr(l, at - 1, 1)
            lead = substr(l, 1, at - 1)
            if (prefix ~ /^[a-zA-Z]:/) {
              if (before ~ /[a-z0-9_]/) continue
            } else if (before ~ /[a-z0-9_.\/-]/ && lead !~ /(file:\/\/|\/mnt(\/[a-z])?|\/\/wsl(\$|\.localhost)\/[^\/ \t]+)$/) continue
          }
          rest = substr(line, at + length(prefix))
          if (match(rest, /^[^\\\/ \t"'"'"'`)>,;:]+/) == 0) continue
          name = substr(rest, 1, RLENGTH)
          sub(/\.+$/, "", name)
          if (name == "" || name ~ /^[<$%{*~.]/ || substr(name, 1, 3) == "\342\200\246" || name ~ /^[0-9]+$/ || index(placeholder, " " tolower(name) " ") > 0) continue
          return prefix "<name>"
        }
        return ""
      }
      function report(place, what) {
        key = hash SUBSEP place SUBSEP what
        if (key in done) return
        done[key] = 1
        printf "repo-check: commit %s, %s: %s. %s\n", hash, place, what, todo
      }
      /^\001/ { hash = substr($0, 2); inmsg = 1; next }
      inmsg && /^\002/ { inmsg = 0; next }
      inmsg {
        folder = user_folder($0)
        if (folder != "") report("message", "names the user folder of a machine (" folder ")")
        if (own_name($0)) report("message", "holds the user folder or login name of this machine")
        next
      }
      # "+++ " names the file only in a file header (from "diff " to the first "@@"); in a hunk
      # it is an added line that starts with "++ ".
      /^diff / { inhdr = 1; next }
      inhdr && /^\+\+\+ / { file = substr($0, 5); sub(/^b\//, "", file); next }
      /^@@ / { inhdr = 0; split($3, h, ","); lineno = substr(h[1], 2) + 0; next }
      inhdr { next }
      /^\+/ {
        if (own_name(substr($0, 2))) report(file ":" lineno, "adds a line with the user folder or login name of this machine")
        lineno++
      }')
  if [ -n "$published" ]; then
    printf '%s\n' "$published"
    found=1
  fi
fi

exit "$found"
