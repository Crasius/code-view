#!/bin/sh
# history-guard: deny git commands that rewrite shared history.
#
# Why: worktrees, main and pushed branches are shared. Another agent may have committed
# on top a moment ago; an amend, rebase, reset or force push folds your change into their
# commit or drops theirs. Make a new commit or `git revert <sha>` instead.
#
# Input: the harness hook JSON on stdin; the shell command is tool_input.command
# (a string or an array of words). Output: exit 2 with one line on stderr to deny,
# exit 0 to allow. Empty or unparsable input is allowed (exit 0).
# Escape for a sole writer whose HEAD is its own commit: the shell comment
#   # history-rewrite-ok: <reason>
# Denied: commit --amend; rebase (except --abort, --quit, --show-current-patch);
# reset with --hard/--soft/--keep/--merge or a commit ref; push --force, -f,
# --force-with-lease or +<refspec>; filter-branch; filter-repo.
# Plain `git reset` and `git reset [<commit>] -- <path>` stay allowed.
# git counts only as the command word of a segment (after X=y, shell keywords and
# wrappers such as sudo, env, time, xargs). Heredoc bodies are data and are skipped,
# unless a shell on that line reads them; `sh -c <script>` scripts are scanned.
# POSIX sh and awk only; works from any working directory.

input=$(cat 2>/dev/null) || exit 0
case $input in
  *git*) ;;
  *) exit 0 ;;
esac

reason=$(printf '%s\n' "$input" | LC_ALL=C awk '
function json_string(s, j,    out, n, c, u, rest, m) {
  # s[j..] follows an opening quote. Returns the decoded text; JEND = closing quote index, 0 if none.
  out = ""; n = length(s)
  while (j <= n) {
    rest = substr(s, j)
    m = match(rest, /["\\]/)
    if (m == 0) { JEND = 0; return "" }
    out = out substr(rest, 1, m - 1)
    j += m - 1
    c = substr(s, j, 1)
    if (c == "\"") { JEND = j; return out }
    c = substr(s, j + 1, 1)
    if (c == "n" || c == "r") out = out "\n"
    else if (c == "t") out = out "\t"
    else if (c == "b" || c == "f") out = out " "
    else if (c == "u") {
      u = tolower(substr(s, j + 2, 4))
      if (u == "0026") out = out "&"
      else if (u == "0027") out = out "\047"
      else if (u == "003c") out = out "<"
      else if (u == "003e") out = out ">"
      else out = out " "
      j += 4
    }
    else out = out c
    j += 2
  }
  JEND = 0
  return ""
}

function skip_space(s, j) {
  while (j <= length(s) && substr(s, j, 1) ~ /[ \t\r\n]/) j++
  return j
}

function command_of(text,    p, rest, j, c, out, item) {
  FOUND = 0
  p = index(text, "\"tool_input\"")
  rest = (p > 0) ? substr(text, p) : text
  p = index(rest, "\"command\"")
  if (p == 0) return ""
  rest = substr(rest, p + 9)
  j = skip_space(rest, 1)
  if (substr(rest, j, 1) != ":") return ""
  j = skip_space(rest, j + 1)
  c = substr(rest, j, 1)
  if (c == "\"") {
    out = json_string(rest, j + 1)
    if (JEND == 0) return ""
    FOUND = 1
    return out
  }
  if (c != "[") return ""
  out = ""
  j++
  while (j <= length(rest)) {
    j = skip_space(rest, j)
    c = substr(rest, j, 1)
    if (c == "]") { FOUND = 1; return out }
    if (c == ",") { j++; continue }
    if (c != "\"") return ""
    item = json_string(rest, j + 1)
    if (JEND == 0) return ""
    gsub(/\047/, "\047\\\\\047\047", item)
    out = out (out == "" ? "" : " ") "\047" item "\047"
    j = JEND + 1
  }
  return ""
}

function base_name(t,    b) {
  b = t
  sub(/.*[\/\\]/, "", b)
  b = tolower(b)
  sub(/\.exe$/, "", b)
  return b
}

function is_shell(t) {
  return base_name(t) ~ /^(sh|bash|dash|zsh|ksh|ash)$/
}

function is_known_ref(b,    cmd, line) {
  if (!REFS_LOADED) {
    REFS_LOADED = 1
    cmd = "git for-each-ref \047--format=%(refname:short)\047 refs/heads refs/tags refs/remotes 2>/dev/null"
    while ((cmd | getline line) > 0) REF[line] = 1
    close(cmd)
  }
  return (b in REF)
}

function is_commit_ref(x,    b) {
  if (x ~ /@\{/) return 1
  b = x
  sub(/[~^][0-9~^]*$/, "", b)
  if (b == "HEAD" || b == "@") return b != x
  if (b == "") return 0
  if (b ~ /^(ORIG_HEAD|FETCH_HEAD|MERGE_HEAD)$/) return 1
  if (b ~ /^[0-9a-fA-F]+$/ && length(b) >= 7 && length(b) <= 40) return 1
  if (b ~ /^(origin|upstream)\//) return 1
  return is_known_ref(b)
}

function rule(sub_, from,    a, x) {
  if (sub_ == "filter-branch" || sub_ == "filter-repo") return sub_
  if (sub_ == "commit") {
    for (a = from; a <= NT; a++) if (T[a] == "--amend") return "commit --amend"
    return ""
  }
  if (sub_ == "rebase") {
    for (a = from; a <= NT; a++) if (T[a] ~ /^--(abort|quit|show-current-patch)$/) return ""
    return "rebase"
  }
  if (sub_ == "push") {
    for (a = from; a <= NT; a++) {
      x = T[a]
      if (x ~ /^--force(-with-lease)?(=.*)?$/ || x ~ /^-[A-Za-z]*f[A-Za-z]*$/) return "push --force"
      if (x ~ /^\+/) return "push +<refspec>"
    }
    return ""
  }
  if (sub_ == "reset") {
    # With `--` git resets index entries for paths only and never moves HEAD.
    for (a = from; a <= NT; a++) if (T[a] == "--") return ""
    for (a = from; a <= NT; a++) {
      x = T[a]
      if (x ~ /^--(hard|soft|keep|merge)$/) return "reset " x
      if (substr(x, 1, 1) == "-") continue
      if (is_commit_ref(x)) return "reset <commit>"
    }
    return ""
  }
  return ""
}

function command_start(    k, w) {
  # Index of the command word in T[1..NT]: skips X=y assignments, shell keywords
  # and wrappers with their options. 0 when the segment has no command word.
  k = 1
  while (k <= NT) {
    if (T[k] ~ /^[A-Za-z_][A-Za-z0-9_]*=/) { k++; continue }
    w = base_name(T[k])
    if (w !~ /^(!|\{|if|then|else|elif|do|while|until|time|command|exec|nohup|nice|sudo|env|xargs|timeout)$/) return k
    k++
    while (k <= NT && T[k] ~ /^-./) {
      if (w == "sudo" && T[k] ~ /^-[ugCDhpRrTt]$/) k++
      else if (w == "nice" && T[k] == "-n") k++
      else if (w == "env" && T[k] ~ /^-[uCS]$/) k++
      else if (w == "xargs" && T[k] ~ /^-[EdIiLlnPs]$/) k++
      else if (w == "timeout" && T[k] ~ /^-[ks]$/) k++
      k++
    }
    if (w == "timeout") k++
  }
  return 0
}

function segment(    k, i, r, a, text) {
  if (NT == 0) return
  k = command_start()
  if (k == 0) return
  if (is_shell(T[k])) {
    # `sh -c <script>` runs the script; a bare shell reads heredoc or piped input.
    for (a = k + 1; a < NT; a++)
      if (T[a] ~ /^-[A-Za-z]*c[A-Za-z]*$/) { Q[++QN] = T[a + 1]; return }
    LINE_SHELL = 1
    return
  }
  if (base_name(T[k]) != "git") return
  i = k + 1
  while (i <= NT && substr(T[i], 1, 1) == "-") {
    if (T[i] ~ /^(-C|-c|--git-dir|--work-tree|--namespace|--config-env|--super-prefix)$/) i += 2
    else i++
  }
  if (i > NT) return
  r = rule(T[i], i + 1)
  if (r == "" || HIT != "") return
  text = T[k]
  for (a = k + 1; a <= NT; a++) text = text " " T[a]
  gsub(/[\r\n\t]+/, " ", text)
  HIT = r
  HITCMD = text
}

function heredoc_bodies(cmd, j,    n, h, e, line, body) {
  # cmd[j..] starts after a newline. Drops the pending heredoc bodies; queues
  # them as scripts when a shell on that line reads them. Returns the next index.
  n = length(cmd)
  for (h = 1; h <= HN; h++) {
    body = ""
    while (j <= n) {
      e = index(substr(cmd, j), "\n")
      line = (e > 0) ? substr(cmd, j, e - 1) : substr(cmd, j)
      j = (e > 0) ? j + e : n + 1
      sub(/\r$/, "", line)
      if (HS[h]) sub(/^\t+/, "", line)
      if (line == HW[h]) break
      body = body line "\n"
    }
    if (LINE_SHELL) Q[++QN] = body
  }
  HN = 0
  return j
}

function analyze(cmd,    n, j, c, q, tok, has, w) {
  NT = 0; tok = ""; has = 0; q = ""; HN = 0; LINE_SHELL = 0
  n = length(cmd); j = 1
  while (j <= n) {
    c = substr(cmd, j, 1)
    if (q == "\047") {
      if (c == "\047") q = ""; else tok = tok c
      j++; continue
    }
    if (q == "\"") {
      if (c == "\\" && j < n) { tok = tok substr(cmd, j + 1, 1); j += 2; continue }
      if (c == "\"") q = ""; else tok = tok c
      j++; continue
    }
    if (c == "\\" && j < n) { tok = tok substr(cmd, j + 1, 1); has = 1; j += 2; continue }
    if (c == "\047" || c == "\"") { q = c; has = 1; j++; continue }
    if (c == "#" && !has) {
      while (j <= n && substr(cmd, j, 1) != "\n") j++
      continue
    }
    if (c == " " || c == "\t" || c == "\r") {
      if (has) { T[++NT] = tok; tok = ""; has = 0 }
      j++; continue
    }
    if (c == "<" && substr(cmd, j + 1, 1) == "<" && substr(cmd, j + 2, 1) != "<") {
      # Heredoc `<<[-]WORD`: remember WORD; its body starts after the next newline.
      if (has) { T[++NT] = tok; tok = ""; has = 0 }
      j += 2
      HS[HN + 1] = 0
      if (substr(cmd, j, 1) == "-") { HS[HN + 1] = 1; j++ }
      while (j <= n && substr(cmd, j, 1) ~ /[ \t]/) j++
      w = ""
      while (j <= n) {
        c = substr(cmd, j, 1)
        if (index(" \t\r\n;&|()<>", c) > 0) break
        if (c != "\047" && c != "\"" && c != "\\") w = w c
        j++
      }
      if (w ~ /^[A-Za-z_]/) HW[++HN] = w
      continue
    }
    if (index(";&|()`\n\r", c) > 0) {
      if (has) { T[++NT] = tok; tok = ""; has = 0 }
      segment(); NT = 0
      j++
      if (c == "\n") {
        if (HN > 0) j = heredoc_bodies(cmd, j)
        LINE_SHELL = 0
      }
      continue
    }
    tok = tok c; has = 1; j++
  }
  if (has) T[++NT] = tok
  segment(); NT = 0
}

{ text = (NR == 1) ? $0 : text "\n" $0 }

END {
  cmd = command_of(text)
  if (!FOUND) exit 0
  if (cmd ~ /#[ \t]*history-rewrite-ok:[ \t]*[^ \t\r\n]/) exit 0
  QN = 1; Q[1] = cmd
  for (qi = 1; qi <= QN && qi <= 20; qi++) analyze(Q[qi])
  if (HIT == "") exit 0
  printf "history-guard: denied `%s` (rule: %s). It rewrites shared history, and another agent may have committed on top. Make a new commit, or undo with `git revert <sha>`. Only a sole writer whose HEAD is its own commit may re-run it with the shell comment `# history-rewrite-ok: <reason>`.", HITCMD, HIT
  exit 3
}
')
# awk exits 3 to deny; any other failure (including an awk error) allows the command.
status=$?
case $status:$reason in
  3:history-guard:*)
    printf '%s\n' "$reason" >&2
    exit 2 ;;
esac
exit 0
