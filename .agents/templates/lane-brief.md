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

- <shared files (root Cargo.toml, a mod.rs module list) this lane may append to, and nothing else there; or "none">

## Do not edit

- Everything outside the owned paths and the append-only files, including <other lanes' paths, with the owning lane>.
- A needed change there goes into your report as a correction: file, exact line, reason.

## Sources

- <absolute path of every file to read first: rules, skills, specs, neighbouring code>

## Steps

1. Read the sources. Design the change before writing code.
2. <implementation steps>
3. Own worktree: merge the base in, then run the checks below once, on what you touched. Fix every failure in one pass, then one confirming run. No test runs during step 2 (rule `testing`, Running). Worktree "shared, edit only": skip this step; the lead builds and routes failures back.
4. Own worktree: commit only your owned and append-only paths: `git add -- <new and changed paths> && git commit -F <message file> -- <all paths>` (rule `git-flow`), then `git show --stat HEAD`. Worktree "shared, edit only": run no git write command; list your changed files in the report.

## Checks

- <`cargo test -p <crate> [filter]` for the changed crates, clippy, the walkthrough that proves the change; each with the expected result; "none" for a shared, edit-only lane>
- `rustfmt --edition 2024 --check <changed files>` and `cargo run -q -p repo-lint -- --base <base branch> <owned paths>` are clean; never `cargo fmt` the whole workspace (rule `rust`, Format).
- Rust lanes, before each commit and before the report: `cd <worktree path> && sh .agents/hooks/rust-stop-gate.sh --base <base branch> </dev/null` exits 0. The Stop hook does not run for lanes (rule `lanes`). No full `cargo test --workspace` (rule `lanes`).
- Never end your turn to wait for a background job. Your turn ends only with the report.

## Report format

- What you built, with the goal's acceptance criteria.
- Commits (hash and subject), or the changed files of a shared, edit-only lane.
- Checks with PASS, FAIL or NOT-VERIFIED and the evidence.
- Appends to the append-only files, and corrections for every other file outside your owned paths.
- What the merge or the next lane needs.
