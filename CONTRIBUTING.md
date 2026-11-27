# Contributing to Code View

Thank you for helping. This page says how to set up, what to check before a pull request, how a pull request gets merged and what a reviewer looks for.

## Before you start

- Code View is source-available under the [Functional Source License 1.1](LICENSE) (`FSL-1.1-ALv2`), not open source. Every contribution needs the contributor license agreement in [CLA.md](CLA.md): you keep your copyright and give Northbyte a license that lets it ship, relicense and sell your contribution. The pull request template holds the statement you leave in the pull request to agree.
- Issues and pull requests go to the public repository, <https://github.com/Crasius/code-view>.
- For anything larger than a fix, open an issue first and describe the change, so that nobody builds something that will not be merged.
- Security problems go through [SECURITY.md](SECURITY.md), never into a public issue or pull request.

## How to read AGENTS.md

[AGENTS.md](AGENTS.md) and the rules in `.agents/rules/` are the project's rulebook, written for coding agents and the maintainer. As a human contributor, follow its Done, Git and Commands sections and the rules for the files you change; skip what is about agents, models, lanes and the maintainer's chat.

## Set up

1. Clone the repository from GitHub and install git and Rust through [rustup](https://rustup.rs). `rust-toolchain.toml` pins the toolchain; rustup installs it on the first `cargo` command.
2. Install what your system needs: [docs/windows.md](docs/windows.md), [docs/macos.md](docs/macos.md) or [docs/linux.md](docs/linux.md).
3. Run the app: `cargo run -p review-app --bin CodeView`.

The tests never reach the network or your token store (AGENTS.md, Invariants), so you can run them without an account. To try the app against a host, use your own account and a repository of your own.

## Before you open a pull request

- Run `sh scripts/check.sh` and fix every finding (`sh scripts/check.sh --list` names its steps), then commit and run `sh scripts/check.sh test` (the tests your committed change affects) ([AGENTS.md](AGENTS.md#commands), Commands). Clippy and rustdoc run with `-D warnings`, so a warning or a broken link in a doc comment fails the check. The dependency bans need cargo-deny; without it the step says SKIPPED with the install command.
- Add tests as [.agents/rules/testing.md](.agents/rules/testing.md) says: a regression test for every bug you fix. UI changes are proven by kittest tests that assert state, not by screenshots ([.agents/rules/ui.md](.agents/rules/ui.md)).
- Write commit subjects in the imperative mood, in English, and put what you checked in the body (AGENTS.md, Git).
- Keep one topic per pull request.
- Never commit tokens, `.env` files or keys.

## How a pull request gets merged

The project's source of truth is a GitLab repository; this GitHub repository is its public mirror, and it takes pull requests.

1. Open the pull request on GitHub against `main` and fill in the template, including the CLA statement.
2. The pull request workflow (`.github/workflows/pull-request.yml`) checks it: format, repository rules and lints, the tests your change affects on Linux, and on a Mac those of the crates with macOS code when your change reaches one, directly or through a crate it depends on. Its check `required` sums them up. A maintainer may have to approve the run of a first-time contributor.
3. A maintainer reads the whole change, then lands it through GitLab with a merge commit that keeps your commits as they are. Nobody merges on GitHub, and nobody squashes or rebases your commits.
4. When GitLab's `main` reaches the mirror, GitHub shows your pull request as merged.

Push more commits to your branch to answer review comments. Until it is landed you may also rewrite your branch; once it is landed, your commits are part of `main`.

## What a reviewer expects

- The checks above pass, and the pull request says what you ran.
- The change does one thing and fits the structure: the right crate for its layer ([.agents/rules/rust.md](.agents/rules/rust.md), Workspace), no duplicated code, names that say what a thing is.
- No leftover `TODO`, no old and new code path side by side, no special case that hides a broken path (AGENTS.md, Done).
- Documentation that changes with the code is updated in its one place in `docs/`, linked rather than copied.
- User-facing text in the app is English.

The maintainer may ask for changes, push small fixes on top of your commits, or decline a change that does not fit the product. Pull request content is reviewed before anything of it is built.
