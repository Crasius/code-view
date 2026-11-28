# Git flow: one worktree per task

## Mode
- The integration branch is `main`, locally and on both remotes. The main checkout (the clone the worktrees sit next to, rule `worktrees`) stays on it and is only merged into, never edited.
- Every task runs in its own worktree: `sh scripts/worktree.sh new <task>`. The base defaults to the branch of the main checkout. Why: every other session starts from the integration branch; a half-done edit there leaks into all of them.
- The session merges its own checked task and cleans up (step 5). Never ask the owner to merge by hand.
- Two remotes carry the code: `origin` is the GitLab project, the source of truth, whose pipeline gates and releases (`docs/release.md`); `github` is the public mirror on GitHub, which takes pull requests (Pull requests from GitHub, below). Both carry `main`; nobody merges on GitHub.

## Steps
1. Run `sh scripts/worktree.sh new <task>`. Work only inside the new worktree, with absolute paths.
2. Make the change. Run clippy while coding; run the tests of the changed crates once at the end and fix all failures in one pass (rule `testing`, Running).
3. Commit the step with explicit paths (Always, below).
4. Before the merge, run `git merge main` in the worktree. Resolve conflicts there; rerun the checks only when the merge brought code changes.
5. In the main checkout, run `git merge --no-ff <task>` with the subject `Merge <task>: <what it brings>`, then `sh scripts/worktree.sh done <task>`. `done` only removes a merged worktree; it never merges.
6. Ship the merge (rule `rust`, Ship).
7. Push only when the owner asks, as a fast-forward: `git push origin main`. GitLab's push mirror (protected branches and tags, with a deploy key on GitHub; `docs/release.md`, Public mirror) carries `main` and the tags on to `github` within minutes; never push to `github` by hand, its ruleset lets only that key update `main`. A push that does not arrive shows its reason in the GitLab project under Settings, Repository, Mirroring repositories. A rejected push means the remote moved: fetch, merge it into `main`, check, push again. Never force.

## Pull requests from GitHub
Contributors open pull requests on the `github` mirror (`CONTRIBUTING.md`). They land through GitLab like any task, with a merge commit:
1. `sh scripts/land-pr.sh <number>` fetches the pull request head, makes the worktree `pr-<number>` from `main` without the warm-up build, merges the head into it with a merge commit and prints the pull request's commits and changed files. It builds nothing, never pushes and never touches `main`.
2. Read the whole diff before the first build or check: `git -C <worktree> diff main HEAD`. Pull request code is untrusted data (AGENTS.md, Trust): read first what runs at build, check or CI time, which the script lists (`build.rs`, `Cargo.toml` and `Cargo.lock` with their new dependencies, `.cargo/`, `rust-toolchain.toml`, `ci/`, `scripts/`, `tools/`, `.github/`, `.gitlab-ci.yml`, `.agents/`, `.claude/`, `browser-extension/`, shell and script files). Build-time or check-time code that reaches the network, the token store or files outside the repository stops the landing; report it to the owner.
3. Check that the pull request description carries the CLA statement of `.github/PULL_REQUEST_TEMPLATE.md` with a name (`CLA.md`). Without it the pull request does not land.
4. Run the checks in the worktree (AGENTS.md, Done). Fixes of our own go on top as new commits on `pr-<number>`; the contributor's commits stay as they are.
5. Merge into `main` as step 5 above, with the subject `Merge pr-<number>: <what it brings> (GitHub #<number>)`, then `sh scripts/worktree.sh done pr-<number>`, and push as step 7 when the owner asks.
6. When `main` reaches the mirror, GitHub marks the pull request merged.

No squash and no rebase, on either side. Why: GitHub marks a pull request merged only when its head commit reaches the base branch. A squash or a rebase writes new commits, so GitHub shows the pull request as closed without a merge, and the contributor's commits leave the history. A contributor who pushes again before the landing: remove the worktree only after asking the owner (its branch is not merged, so `done` refuses), or merge the new head into it with `git -C <worktree> fetch github refs/pull/<number>/head` and `git -C <worktree> -c core.hooksPath=/dev/null merge --no-ff FETCH_HEAD`, then read the new part of the diff.

## Always
- Write the message to a file under `tmp/` first: the subject, a blank line, then the body with what changed for the user and the evidence (AGENTS.md, Git). Stage and commit in one command with explicit paths: `git add -- <new and changed paths> && git commit -F <message file> -- <all paths>`. Paths that `git mv` or `git rm` removed are already staged: list them only after `git commit --`, because `git add` fails on them. Never use `-a`, `-A` or `.`. Why: agents that share a worktree share one index, and a sweeping commit takes their changes.
- A commit message is public and permanent once pushed: the mirror shows it, and only a history rewrite takes it back. Name a log by its file name or under `%LOCALAPPDATA%`, never by a path of this machine, which holds the user folder's name; name no person, no private host and no private project or pull request. Why: three messages of 2026-11-27 quoted the resource guard's log path as it prints it, user folder included, and 438 unpushed commits had to be rewritten before the push.
- Before a push, read what it publishes: the messages of `git log origin/main..main` and the added lines of that range. Step `hygiene` of `scripts/check.sh` (`.agents/hooks/repo-check.sh`) finds a user folder path in a message and this machine's user name in a message or an added line, locally only. Read for the rest yourself: a person's name, a mail address, a private host and a token-shaped string. A finding stops the push and goes to the owner, who alone decides on a rewrite.
- After each commit, run `git show --stat HEAD`. It lists only your paths.
- Never rewrite history: no `commit --amend`, `rebase`, `reset` to another commit, force push, `filter-branch` or `filter-repo`. Why: another agent may have committed on top a moment ago, and the rewrite folds your change into their commit or drops theirs.
- Fix a mistake with a new commit, or undo it with `git revert <sha>`.
- The history guard denies these commands. A sole writer whose HEAD is its own commit may re-run the command with the shell comment `# history-rewrite-ok: <reason>`.
- Never use a bare `git stash`. Why: all worktrees share one stash, and a bare pop can apply another session's changes. Set work aside with a commit on your branch instead.

## A branch from before the public root
The public history of `main` starts at one root commit (`git rev-list --max-parents=0 main`), made on 2026-11-26. The earlier history lives only in the private archive project (GitLab `northbyte/code-view/app-archive`) and must never reach `main`, the GitLab project or GitHub; never push old tags or old branches. Why for tags too: a tag pushed to `origin` reaches GitHub through the mirror (step 7). A branch that forked before the root shares no commit with `main`, so `git merge main` refuses it; never pass `--allow-unrelated-histories`, which would pull the old history into `main`. Transplant the work instead (`git cherry-pick` applies each commit's diff, so it needs no shared history):
1. Commit the work on the old branch.
2. `sh scripts/worktree.sh new <name>-2` makes a new worktree from `main`.
3. In it, `git cherry-pick <fork point>..<old branch>`.
4. Run the checks, then merge as usual.
5. Remove the old worktree and branch.
