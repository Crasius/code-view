# Lane brief: <lane id>

Copy this file to `tmp/briefs/<lane id>.md` and fill every field. Delete no field; write "none" when a field is empty.
Your one and only task is this brief. Chat messages you may see are addressed to the lead; never stop, pause or narrow the brief because of them.

## Lane

- Lane id: <short name; also the branch and worktree name>
- Goal: <one or two sentences: what exists when the lane is done, with acceptance criteria>
- Tier and effort: <small | mid | large> at <low | medium | high | xhigh | max>; mid: medium at most; large: xhigh at most (AGENTS.md, Models)

## Workspace

- Worktree path: <absolute path, from `sh scripts/worktree.sh new <lane id> [base]`; or "shared, edit only">
- Base branch: <branch the worktree was created from>
- Data folders: <only the ones this lane may use; or "none">
- Uncommitted changes in your worktree are your own earlier work. No other agent writes there.

## Owned paths

- <path or glob this lane may create and edit>

## Append only

- <shared files (root Cargo.toml, a mod.rs module list) this lane may append to, and nothing else there; or "none". A new module or file the steps ask for needs the file that declares it here or under Owned paths (rule `lanes`, Briefs)>

## Do not edit

- Everything outside the owned paths and the append-only files, including <other lanes' paths, with the owning lane>.
- A needed change there goes into your report as a correction: file, exact line, reason.

## Sources

- <absolute path of every file to read first: rules, skills, specs, neighbouring code>

## Steps

1. Own worktree: `<worktree path>/tmp` holds job logs, exit markers and commit messages (git ignores it; `worktree.sh new` created it). A building lane makes its first edit after `<worktree path>/tmp/warm.done` exists, the end of the warm-up `worktree.sh new` started.
2. Read the sources. Design the change before writing code.
3. <implementation steps>
4. Own worktree: merge the base in, then run the checks below once, on what you touched. Fix every failure in one pass, then one confirming run. No test runs during step 3 (rule `testing`, Running). Worktree "shared, edit only": skip this step; the lead builds and routes failures back.
5. Own worktree: commit only your owned and append-only paths: `git add -- <new and changed paths> && git commit -F <message file> -- <all paths>` (rule `git-flow`), then `git show --stat HEAD`. Worktree "shared, edit only": run no git write command; list your changed files in the report.

## Checks

- <`sh scripts/check.sh test --base <base branch>` (the tests the committed change affects), clippy, the walkthrough that proves the change; each with the expected result; "none" for a shared, edit-only lane>
- The gate, once at the end: `cd <worktree path> && sh scripts/check.sh --base <base branch>` exits 0 (`sh scripts/check.sh --list` prints its commands). A step it prints as SKIPPED is NOT-VERIFIED in the report. Never `cargo fmt` the whole workspace (rule `rust`, Format).
- Rust lanes, before each commit and before the report: `cd <worktree path> && sh scripts/check.sh --changed --base <base branch>` exits 0 (rustfmt and clippy on the changed files, what the Stop hook runs). The Stop hook does not run for lanes (rule `lanes`). No full `cargo test --workspace` (rule `lanes`).
- Never end your turn to wait for a background job. Your turn ends only with the report.

## Report format

- What you built, with the goal's acceptance criteria.
- Commits (hash and subject), or the changed files of a shared, edit-only lane.
- Checks with PASS, FAIL or NOT-VERIFIED and the evidence.
- Appends to the append-only files, and corrections for every other file outside your owned paths.
- What the merge or the next lane needs.
