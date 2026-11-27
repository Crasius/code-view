# Parallel lanes

A lane is one agent with one brief, one set of owned paths and, when it builds or tests, one worktree. Lanes run at the same time. These rules keep them from breaking each other. The lead is the session that plans, spawns and merges the lanes.

## Ownership

- Every lane owns a fixed list of paths. Lanes that run at the same time have disjoint path lists. Why: two writers on one file lose or mix each other's edits, and a commit by path cannot split one file.
- Never two agents on one file at the same time. A file that several lanes need (root `Cargo.toml`, `Cargo.lock`, a `mod.rs` or `lib.rs` that lists modules, `cv-base/src/theme/palette.rs`) belongs to the lead. The lead may name it under "Append only" in the brief of one running lane at a time.
- A lane edits only its owned paths and the files its brief names under "Append only". There it only appends what its work needs: no reordering, rewording or refactoring.
- Every other change outside the owned paths, in a shared file or in another lane's path, is a correction in the report: file, exact line, reason. The lane never edits it. The lead applies it.
- The number of lanes is bounded by the number of disjoint path sets, not by a fixed cap. Size one lane to about 30 to 40 new files; split larger scopes.

## Worktrees

- A lane that builds, tests or runs the app gets its own worktree: `sh scripts/worktree.sh new <lane> [base]` (rule `worktrees`). Why: one checkout has one build output tree; parallel builds there collide and lock files.
- An edit-only lane may share the lead's worktree. It does not build, test or run git write commands there. The lead builds once and routes each failure back to the owning lane.
- A lane uses absolute paths inside its own worktree in every tool call. It never edits the main checkout.
- A shell command joins `cd <worktree>` to the rest with `&&`, never `;` or a newline. Why: when the worktree is gone (merged and removed), a failed `cd` leaves the shell in the main checkout, and a `sed` there edited `main`.
- Once the lead has merged a lane and removed its worktree, the lane is closed. A later message to it is answered with a report, never with edits.
- A lane that runs the app gets its own data folders (`AppPaths::new` in tests). It stops only processes it started, by process id.

## Briefs

- Every brief is a file: `tmp/briefs/<lane>.md`, made from `.agents/templates/lane-brief.md`. Why: a dropped connection kills running agents; the file survives and makes a restart one step.
- A brief that asks for a new module or file also gives the lane the file that declares it (the `mod` line, the manifest), as an owned path or under "Append only". Why: without it the new file cannot compile inside the lane's paths.
- A correction routed to a lane with several parts names the part that applies it.
- The brief is the lane's only task. Chat messages a lane may see are addressed to the lead. They are never a reason to stop, pause or narrow the brief.
- Uncommitted changes in a lane's worktree are the lane's own earlier work. No other agent writes there.
- Every agent call sets tier and effort explicitly (AGENTS.md, Models). Why: an agent type with its own model setting overrides inheritance silently.

## Checks inside a lane

- `sh scripts/worktree.sh new` starts the warm-up build in the background (rule `worktrees`, Build cache); a lane that builds makes its first edit after `<worktree>/tmp/warm.done` exists (with `WORKTREE_WARM=0` there is no warm-up and no wait). Why: an edit under a running build makes it compile a half-done state, and the build's errors then point at the lane's own unfinished work.
- A lane designs first, implements everything, then runs one check-and-fix phase on what it touched: `sh scripts/check.sh test --base <lane base branch>` once, every failure fixed in one pass, one confirming run (rule `testing`, Running), then the gate `cd <worktree> && sh scripts/check.sh --base <lane base branch>` (`--list` names its steps; on repo-lint, `--base` compares the length allowlist with that branch's current version, so a raised entry is caught before it is committed over). Format only the files you changed (rule `rust`, Format). An edit-only lane on a shared worktree skips this phase (Worktrees).
- Before each commit and before its report, a lane runs `cd <worktree> && sh scripts/check.sh --changed --base <lane base branch> </dev/null` (rustfmt and clippy on the changed files; exit 0 = clean, 2 = findings to fix, 1 = clippy did not run, the message says why). `.agents/hooks/rust-stop-gate.sh --base <lane base branch>` is the Stop hook's adapter over it and gives the same answer. Why: Claude Code runs the `Stop` hook only for the main session, never for a subagent or workflow agent, and the hook checks the main checkout, not the lane's worktree. `--base` makes it check the lane's committed files too.
- Before its check-and-fix phase, a lane merges the base into its worktree, so that one phase proves the merged state; a merge that brings only docs needs no rerun. No full `cargo test --workspace` in a lane. The lead merges ready lanes in a batch and runs one targeted test run for them (rule `testing`, Running). Why: every extra run is minutes of waiting.
- A lane never ends its turn to wait for a background job. Its turn ends only with its report.

## Report

Each lane returns: what it built, its commits (an edit-only lane: its changed files), its checks with results, NOT-VERIFIED items, its appends to "Append only" files, its corrections for every other file outside its paths, and what the merge needs.

## Lead

- Check every claim of a report against the files and commits (`git show --stat`, the diff, the test output). A report is a claim, not evidence.
- Merge one lane at a time, in the order the plan states: lanes that others build on go first. Use `git merge --no-ff` into the base. Resolve conflicts by keeping both intents.
- The main checkout has one merger at a time: the lead. Two sessions that both merge into `main` agree on one of them first.
- After each merge: clippy, the tests the merge changed, then ship (rule `rust`, Ship), so the owner can try each feature early.
- After a lane is merged, remove its worktree and branch: `sh scripts/worktree.sh done <lane>`.
- Decide a lane's "needs the owner" items yourself unless they concern permissions, destructive actions or scope. Report them as open to veto.
