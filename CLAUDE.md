@AGENTS.md

# Claude Code

`AGENTS.md` above is the project contract. This file adds only Claude Code specifics.

## Pointers
- `.claude/rules/` holds one pointer stub per rule in `.agents/rules/`, with the same `paths` frontmatter. Edit the source in `.agents/`; a stub only says where to read. A new rule gets a new stub.
- `.claude/settings.json` runs the history guard before every Bash and PowerShell call, and the Rust stop gate (`.agents/hooks/rust-stop-gate.sh`: rustfmt and clippy on the changed `.rs` files) when the main session's turn ends. Claude Code runs `Stop` hooks only for the main session; subagents and workflow agents fire `SubagentStop`, which stays unregistered because their worktrees are not the hook's directory, so lanes run the gate themselves (rule `lanes`).

## Agents
- Tier to model and effort: the user skill `model-routing`. Set `model` and `effort` on every spawned agent and every workflow `agent()` call.
- Lanes that build or test get a worktree from `scripts/worktree.sh` (rule `lanes`), not the harness option `isolation: "worktree"`. Why: harness worktrees land in `.claude/worktrees/`, inside the repo, and nothing removes them after the merge.

## UI work
Use the design skills in this order: `ui-ux-pro-max` for direction, `frontend-design` for building, `web-design-guidelines` for review. Do not skip them for small changes. The app is native egui, so browser tools do not apply to it; check it through kittest walkthroughs that assert state. Never take or read screenshots; the owner judges the look in the shipped build (rule `ui`).
