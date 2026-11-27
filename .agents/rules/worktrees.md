# Worktrees

One git worktree per task or lane that builds, tests or runs the app. The script `scripts/worktree.sh` makes, lists, checks and removes them. It is POSIX sh, so the same script runs on macOS, Linux and Windows (Git Bash; from PowerShell call `sh scripts/worktree.sh ...`). Use only this script, so every worktree has the same name, place and cleanup.

## Commands

| Command | Does |
|---|---|
| `sh scripts/worktree.sh new <name> [base]` | `git worktree add ../<repo>-wt/<name> -b <name> <base>`; base defaults to the branch of the main checkout (`main`). Refuses when the branch or the folder exists. Prints the worktree path. |
| `sh scripts/worktree.sh done <name>` | Removes the worktree and deletes the branch. Refuses a branch that `new` did not create or that is its own base, a worktree with uncommitted or untracked files, and a branch not merged into its base. |
| `sh scripts/worktree.sh list` | `git worktree list`. |
| `sh scripts/worktree.sh check` | Exit 1 when a registered worktree folder is missing, or a branch with own commits merged into its base still has a worktree. Else exit 0. |
| `sh scripts/worktree.sh cache` | Points the current checkout's cargo at sccache (`.cargo/config.toml`, git-ignored). `new` does this for every new worktree. |

## Naming and place

- The name is the task or lane id: letters, digits, `.`, `_` and `-`. Branch name = folder name, so one name finds both.
- Worktrees live in a folder next to the main checkout, `../<repo>-wt/<name>` (a main checkout `<parent>/Review-App` has them in `<parent>/Review-App-wt/`), never inside the repo. Why: a worktree inside the repo shows up in searches, builds and `git status` of the main checkout. Older worktrees under `.claude/worktrees/` came from the harness; remove each once its branch is merged.
- Each worktree has its own `target/` folder. Without the build cache below its first build compiles everything; build once per lane, not per step.
- `new` records the base and start commit in git config (`branch.<name>.worktreebase`, `branch.<name>.worktreestart`). `done` and `check` use them; deleting the branch removes them.

## Build cache

- Every machine that runs lanes has [sccache](https://github.com/mozilla/sccache) on `PATH`: `winget install Mozilla.sccache` (Windows), `brew install sccache` (macOS), `cargo install --locked sccache` or the distribution package (Linux). Set its disk cache to about 60 GB (`[cache.disk] size = 64424509440` in its config file, or `SCCACHE_CACHE_SIZE=60G`); the default 10 GB does not hold debug, clippy and release builds of the workspace.
- `new` writes `<worktree>/.cargo/config.toml` with `build.rustc-wrapper` set to sccache, then warms the worktree in the background (with or without sccache): `cargo test --workspace --no-run`, then `cargo clippy --workspace --all-targets`. Log: `tmp/warm.log`; `tmp/warm.done` marks the end. `WORKTREE_WARM=0` skips the warm-up.
- A lane's cargo commands need no wait loop for the warm-up: cargo's lock on the target folder makes them wait ("Blocking waiting for file lock") and then reuse what the warm-up built. Edits wait for `tmp/warm.done` (rule `lanes`, Checks inside a lane).
- Why: lanes spent 58 % of their time waiting for cold builds of the whole workspace in fresh worktrees (15 lanes measured 2026-10-20; a one-line label change took 42 min, 40 of them building). The warm-up is the main gain: the cold build (5 to 12 min, by machine load) runs while the lane reads, and a one-line change in `review-app` afterwards rebuilds its tests in 9 s (measured 2026-10-20). sccache only helps the dependencies: it hit 80 % of their compiles, but proc-macro crates, build scripts and our own incremental crates are not cacheable, so a cold build with a full cache was not measurably faster under the same lane load.
- The config stays git-ignored and machine-local: a committed wrapper would break every machine and CI runner without sccache.
- Without sccache `new` says so and still warms the worktree; the dependencies then build without the shared cache.

## Working in a worktree

- Never edit the main checkout. Check `git branch --show-current` before the first change.
- Use absolute paths under the worktree in every tool call. Begin every shell call that needs a directory with `cd <worktree> &&` or use `git -C <worktree>`. Why: an agent's shell may return to the main checkout after each call, and a relative path then writes there.
- Keep build output and dependency folders local to each worktree. Never link them between worktrees. Why: a cleanup or rebuild through a link breaks the other checkout.
- Bring the base in with `git merge <base>` inside the worktree before a long build or a merge. Resolve conflicts there, never in the base.
- A harness-managed isolated checkout may be unable to run git outside itself, so it cannot merge into the base. Make worktrees with the script instead.

## Merge and cleanup

1. Commit in the worktree with explicit paths. Check `git status --short` is empty.
2. Merge into the base as the git flow says (rule `git-flow`): `git merge --no-ff <name>` in the base checkout, or a merge request.
3. Run `sh scripts/worktree.sh done <name>` from another checkout right after the merge.
4. Stop the processes the worktree started, by process id. Delete data folders it owned.
5. At session start run `sh scripts/worktree.sh check` and `git worktree list`. Finish or remove what is left before planning new work.

## Problems

- **Merged worktree left behind.** Mechanism: the merge happens, the cleanup is forgotten, and the next session plans on stale state. Prevention: step 3 above; `check` at session start. Fix: `sh scripts/worktree.sh done <name>`.
- **`done` refuses after a squash merge.** Mechanism: a squash merge creates a new commit; the branch tip is not an ancestor of the base, so git sees it as unmerged. Prevention: merge without squash. Fix: compare `git diff <base> <name>`; when it is empty, ask the owner before `git worktree remove` and `git branch -D` by hand.
- **`check` misses a fast-forward merge.** Mechanism: `check` counts a branch as merged only when its tip is off the base's first-parent line; this keeps a lane that only brought the base in from looking merged, but a lane merged into the base by fast-forward looks the same. Prevention: merge with `--no-ff` or a merge commit. Fix: `done <name>` still works; it checks ancestry only.
- **Missing worktree folder.** Mechanism: the folder was deleted by hand, but git still lists it. Prevention: remove worktrees only with `done`. Fix: `git worktree prune`, then `done <name>` when the branch is merged.
- **Lost uncommitted work in a stalled lane.** Mechanism: an agent stops mid-task and leaves edits uncommitted. Prevention: commit each checked step. Fix: back it up first: mark the untracked paths that `git -C <worktree> status --short` lists with `git -C <worktree> add -N -- <those paths>`, then `git -C <worktree> diff --binary > tmp/<name>.patch`. Then relaunch the lane on the same worktree.
- **Edits land in the main checkout.** Mechanism: a relative path or an earlier `cd` resolves against the main checkout. Prevention: absolute paths only. Fix: move the change with `git -C <main> diff -- <paths> | git -C <worktree> apply`, then restore the main checkout from HEAD.
