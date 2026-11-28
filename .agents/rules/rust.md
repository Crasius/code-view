---
paths:
  - "crates/**"
  - "tools/**"
  - "Cargo.toml"
  - "Cargo.lock"
  - "clippy.toml"
  - "rustfmt.toml"
  - "rust-toolchain.toml"
---

# Rust: workspace, format, lints, rules, performance, shipping

Rules carry stable ids (`RS-<group>-<nn>`). An id never changes meaning and is never reused; a retired rule keeps its id and names its replacement. Each rule says Rule, Why and Enforced by: a lint, a script, or review. Tests and the unit-test matrix are in rule `testing`.

## Workspace
The layer table. A crate depends only on its domain crates and on `cv-*` crates of a strictly lower layer (normal, optional, dev or build dependency). `DIRECTION` in `tools/repo-lint/src/manifest.rs` holds the same table; change both together. Domain short names: core, git, store, agent and grammars are the `review-*` crates.

| Layer | Crate | Holds | Domain crates |
|---|---|---|---|
| domain | `review-core` | Model types, the `Provider` trait, the HTTP client and its TLS choice, errors | none |
| domain | `review-git` | Git mirrors, changed files, file diffs, whitespace handling; runs the `git` binary | core |
| domain | `review-store` | Data folders (`AppPaths`), settings, connections, token vault, retention | core |
| domain | `review-agent` | The pull request assistant: drives the signed-in `claude` and `codex` CLIs in their machine modes | core |
| domain | `review-grammars` | The syntax grammars: bat's set (two-face) with ours (C#, PowerShell), merged and dumped at build time | none |
| domain | `provider-github`, `provider-gitlab`, `provider-azure` | One provider each: DTOs, mapping to core types, API calls | core |
| 0 | `cv-base` | Platform FFI, theme, frame metrics, pointer, window activity; the `#[global_allocator]` | store |
| 1 | `cv-widgets` | The atom widgets, `progress_log`, `numbers` | core, git, store |
| 1 | `cv-testkit` | Test support (`Watchdog`, `assert_hermetic`, loopback `http_client`, `PROCESS_SLOTS`, `poll_until`, `samples`, `provider::Script`, `frame_bench`) | core, store |
| 1 | `cv-providers` | `create_provider`, the provider factory | core, the providers |
| 2 | `cv-runtime` | Background runtime, load, refresh, cache, toasts, route, history, trail, link parsing, intents | core, git, store |
| 3 | `cv-media` | Images, image clipboard, video, Mermaid, `Picture`, image state | core, git, agent |
| 3 | `cv-syntax` | Grammars, highlighting, code colors | grammars |
| 4 | `cv-markdown` | The Markdown renderer | core, store |
| 4 | `cv-services` | `Services`: connections, writes, checkouts, mirrors, agents, self review | core, git, store, agent, provider-github |
| 5 | `cv-editor` | `markdown_editor`, `rich_editor` | core, git, store |
| 5 | `cv-code` | Diff rows, line source, contents, code jobs, diff and find paint | core, git |
| 5 | `cv-shell` | The window chrome without the title bar, input, links, single instance, browser host and registration | core, store |
| 6 | `cv-pr-session` | Pull request session, actions, lifecycle, merge choice and paint, assistant link | core, git, store |
| 6 | `cv-pr-tree` | The Files-tab tree | core, git, store |
| 7 | `cv-pr-threads` | Comment threads | core, git, store, provider-github |
| 7 | `cv-pr-assistant` | The assistant panel | core, git, store, agent |
| 7 | `cv-pr-pipelines` | Pipelines, logs, reruns | core, store |
| 7 | `cv-lists` | Pull request list, repositories | core, git, store |
| 7 | `cv-settings` | Settings, onboarding | core, git, store, agent, the providers |
| 8 | `cv-pr-diff` | Diff view, find, overview strip, patch colors | core, git, store |
| 9 | `cv-pr-rendered` | Markdown diff, preview, file header | core, git, store |
| 10 | `cv-pr-summary` | Folder summary | core, git, store |
| 11 | `cv-pr-files` | The Files tab | core, git, store |
| 12 | `cv-pr-page` | The pull request page: band, header, overview, live updates, checkout, merge button | core, git, store |
| 13 | `review-app` | The `CodeView` binary: `main`, `app` with the walkthrough tests, `title_bar` | all |
| tool | `tools/repo-lint` | The repository rules clippy cannot check (Analyzer, below) | none |

- Keep this direction. A provider never depends on another provider, on the store or on the app. Shared types move to `review-core`; shared UI code moves down a layer. Provider-specific behaviour stays in its provider crate; the app speaks to providers only through `review-core`. repo-lint RL005/RL006 check it (RS-DO-03).
- `cv-base` never uses `cv-testkit`, not even as a dev-dependency: `cv-testkit` depends on `cv-base`, so the test binary of `cv-base` would link a second `cv-base` with a second `#[global_allocator]`. `cv-widgets` cannot use it either, because both sit in layer 1.
- Test helpers that another crate's tests need sit under `#[cfg(any(test, feature = "test-support"))]`. The feature adds test items, and it may swap a system seam (process start, dialog, clipboard, registry, HTTP client, agent CLI) for its fake: one `#[cfg]` pair (or one `if cfg!`) in the module that owns the seam. It never removes an item that shipped code calls. Only `[dev-dependencies]` and `[features]` turn it on (RL015, rule `testing`).

## Toolchain
- `rust-toolchain.toml` pins the toolchain with clippy and rustfmt. rustup picks it in every checkout and installs it on the first cargo command (`RUSTUP_AUTO_INSTALL`, default on); `rustup toolchain install` inside the repository does it ahead of time. CI reads the channel from the same file. The CI image `rust:<version>-bookworm` is pinned by digest in `ci/checks.yml` (`RUST_IMAGE`) and `.github/workflows/pull-request.yml` and is bumped with the pin. `rust-version` in `Cargo.toml` follows the pin.
- Bump the pin only in a change of its own that also fixes the new clippy findings and moves `rust-version` and the two image pins with it (RS-DO-02).

## Dependencies
- Declare every external dependency once in `[workspace.dependencies]` of the root `Cargo.toml`, with `default-features = false` where the crate allows it and only the features the app uses. Crates use `<dep>.workspace = true` and may add features. repo-lint RL011 checks it.
- Add a dependency with `cargo add -p <crate> <dep>` after it is in the workspace table. Commit `Cargo.lock` with the change.
- Write one comment above a dependency when its reason or its feature choice is not obvious, as the existing entries do.
- A new dependency must build on Windows, macOS and Linux without system `-dev` packages (no OpenSSL, no libdbus). Why: `docs/linux.md` promises a build with only a C compiler.
- Vendor a crate only when no release does what the app needs and the fix is not released upstream (syntect 5.3.0 pins fancy-regex 0.16). The copy lives in a `vendor/<crate>` folder of the crate that needs it, wired in through `[patch.crates-io]` in the root `Cargo.toml`. Vendored code keeps its upstream style: repo-lint and the stop gate skip every `vendor/` folder, and the only edits are the local patches. Its `README.md` names the upstream, the base version, each local patch and when to drop the copy.
- `deny.toml` holds the dependency graph's bans, sources and advisories: the `deps` step of `sh scripts/check.sh` checks bans and sources on every change, and the CI job `supply-chain` (`ci/checks.yml`) checks the RustSec advisories and the licences when the dependency set changes and on a schedule.
- Footprint is a feature (about 70 MB idle). Weigh binary size and startup cost before you add a crate; prefer a few lines of our own over a large crate for a small need (RS-AGENT-07).

## Format
- The workspace is formatted with rustfmt (`rustfmt.toml`: edition and style edition 2024, `use_small_heuristics = "Max"`). `cargo fmt --all --check` runs in CI and `.agents/hooks/rust-stop-gate.sh` checks the changed files.
- Format what you touch: `rustfmt --edition 2024 <changed files>` before you commit. Parallel lanes format only their own files, never the whole workspace.

## Lints
The contract lives in two files; never loosen either without the owner (RS-AGENT-03).
- Root `Cargo.toml`, `[workspace.lints.rust]` and `[workspace.lints.clippy]`; every crate sets `lints.workspace = true`. Levels stay at warn; `-D warnings` on the command line (CI, ship) makes them errors (RS-LINT-01, RS-DONT-07). The stop gate runs clippy without it, so it can tell the warnings in the changed files from those in other files.
- `clippy.toml` in the root: thresholds, the test exemptions and `disallowed-methods`. Clippy reads only the nearest `clippy.toml`, so a crate-local one would replace the root file; there is none.
- Cargo refuses a crate `[lints]` table next to `workspace = true`, so crate-level differences are crate-root attributes: `#![forbid(unsafe_code)]` in `review-core`, `review-git`, the providers, `repo-lint` and every `cv-*` crate except `cv-base` and `cv-shell`, whose `platform/` modules carry `#![expect(unsafe_code, reason)]`; `#![expect(clippy::cast_*, reason)]` in a UI crate root for egui pixel math, listing only the cast lints that crate's shipped code fires (lints that fire only in tests go into `#![cfg_attr(test, expect(...))]`).
- rustc's `unreachable_pub` keeps `pub` for what the crate root exports, so the visibility shows the real API; everything else is `pub(crate)` or narrower, which in a binary crate is every item. An item that only other crates' tests reach is `pub(crate)` with a gated alias at the export, `#[cfg(any(test, feature = "test-support"))] pub const X: T = inner::X;`; an item no const can alias keeps `pub` behind its gated `pub use` and carries `#[cfg_attr(not(any(test, feature = "test-support")), expect(unreachable_pub, reason))]` (RS-LINT-04).
- Check: the `clippy` step of `sh scripts/check.sh` runs `cargo clippy --workspace --all-targets -- -D warnings`, then `cargo clippy --workspace -- -D warnings` (and a third pass with the `review-app` feature `measurements`). The first unifies the dev-dependency features, so it lints the libraries with `test-support` on; the second lints the shipped variant and is the only one that sees code under `cfg(not(any(test, feature = "test-support")))`. A lint that is new to the contract lands together with the fixes or `#[expect]` entries for all its findings, so the check stays green.

**RS-LINT-01** Declare lint levels once in `[workspace.lints]`, group entries at `priority = -1` so single lints override them; thresholds go in `clippy.toml`.
- Why: one place makes the contract visible and diffable instead of living in command-line flags.
- Enforced by: Cargo `[lints]`, the CI and ship command `-D warnings`.

**RS-LINT-02** Keep clippy's default groups (correctness deny; suspicious, style, complexity, perf warn) and add `pedantic` as a warn group. A pedantic lint the team rejects on principle is allowed once in `[workspace.lints.clippy]`, with its reason and its finding count in the comment above it; that table is the list. Never enable `restriction` or `nursery` as groups.
- Why: pedantic carries most readability lints; restriction and nursery hold contradictory and unstable lints that must be picked one by one.
- Enforced by: `[workspace.lints.clippy]`.

**RS-LINT-03** Take restriction and nursery lints one by one, each in `[workspace.lints.clippy]` under a comment that names the RS rule it serves; that table is the list, and the rules below name the lint that enforces them. `expect_used` stays off: an `expect` that names its invariant is allowed. Tests may unwrap, panic, print, `dbg!` and index (`allow-*-in-tests` in `clippy.toml`).
- Why: each blocks a pattern agents produce or a rule here states.
- Enforced by: `[workspace.lints.clippy]`, `clippy.toml`.

**RS-LINT-04** Suppress a lint only with `#[expect(<lint>, reason = "...")]` on the narrowest item, the reason naming the real cause. Never `#[allow]`.
- Why: `#[expect]` warns when the lint stops firing (`unfulfilled_lint_expectations`), so suppression debt can only shrink, and the reason is checked instead of being a free comment.
- Enforced by: `clippy::allow_attributes`, `clippy::allow_attributes_without_reason`.

## Rules for agents

**RS-AGENT-01** Never report done after `cargo check` or `cargo build`. Done is: `sh scripts/check.sh` clean (`--list` names its steps), the tests of the changed crates pass, and the release build was shipped and started (Ship). Mark what you could not check (Linux, macOS) NOT-VERIFIED.
- Why: LLM-written Rust has the most compile and runtime failures of four measured languages (arXiv 2608.00661); runtime failures survive compilation.
- Enforced by: `.agents/hooks/rust-stop-gate.sh` (the Stop hook of the main session, over `sh scripts/check.sh --changed`, which lanes run by hand, rule `lanes`), CI, AGENTS.md (Done).

**RS-AGENT-02** Never end a borrow or lifetime error with `.clone()`, `Rc<RefCell<_>>`, `Arc<Mutex<_>>`, a `'static` bound, `Box::leak`, `mem::forget` or `ManuallyDrop`. Restructure: borrow the field instead of `self`, return owned data from the background task, end the borrow before the mutation. When a clone is the right answer, say why in the commit message.
- Why: these compile but move the bug to runtime (RefCell panics), leak memory or allocate every frame.
- Enforced by: `clippy::mem_forget`, `clippy::redundant_clone`, RS-DONT-01/02, review of new `Rc`/`RefCell`/`Arc<Mutex>` imports.

**RS-AGENT-03** Never silence to get green: no `#[allow]`, no `#[expect]` without a reason naming the cause, no `#[ignore]` on a failing test, no weakened assertion, no deleted failing test, no loosened level in `Cargo.toml`, `clippy.toml`, `rustfmt.toml` or `tools/repo-lint/file-lengths.allow` unless the owner asked.
- Why: an agent that optimizes for a passing gate lowers the gate; every loosening must be a visible owner decision.
- Enforced by: `clippy::allow_attributes(_without_reason)`, `clippy::ignore_without_reason`, repo-lint RL002, review of any diff to those files.

**RS-AGENT-04** Format only what you touch, with rustfmt (Format). No PostToolUse formatter hook and no workspace-wide `cargo fmt` in a lane.
- Why: a hook that rewrites files behind the agent makes its file state stale; a workspace run in a lane touches other lanes' files.
- Enforced by: `cargo fmt --all --check` in CI, `.agents/hooks/rust-stop-gate.sh` on changed files, review.

**RS-AGENT-05** Never rely on memory of the egui or eframe API: the workspace pins egui 0.37, and the API changes between minor versions. Read docs.rs for the pinned version or the source in `~/.cargo/registry` before you use a method. Run clippy on the touched crate after each logical edit, not only at the end.
- Why: a short edit-compile loop catches invented methods early and keeps diffs small.
- Enforced by: `.agents/hooks/rust-stop-gate.sh` (rustfmt on the changed files, clippy on their crates): the Stop hook of the main session; lanes run `sh scripts/check.sh --changed` by hand (rule `lanes`).

**RS-AGENT-06** Leave no `todo!()`, `unimplemented!()`, `dbg!()`, `println!`/`eprintln!`, `TODO`/`FIXME` comment, commented-out code or parallel old and new code path in a commit.
- Why: these are agent-loop leftovers; in a GUI app they ship as panics or console noise.
- Enforced by: `clippy::todo`, `unimplemented`, `dbg_macro`, `print_stdout`, `print_stderr`, repo-lint RL012, review for dead code.

**RS-AGENT-07** Never add a crate, a feature flag or async to solve a small problem. A new dependency follows Dependencies. Pure logic (diff mapping, parsing) stays synchronous, without tokio types in its signatures.
- Why: footprint is a feature, and synchronous pure functions test without a runtime.
- Enforced by: Dependencies, repo-lint RL011, review.

**RS-AGENT-08** Never write tests to show coverage, and never accept a snapshot change (kittest or insta) without reading its diff. The unit-test matrix in rule `testing` decides what gets a test.
- Why: tests of trivial code are pure maintenance cost and fail on harmless refactors.
- Enforced by: review against rule `testing`.

## Don'ts

**RS-DONT-01** Never clone to make a borrow error go away. A `.clone()` of a `String`, `Vec` or state struct in a loop or on the `update` path is a review trigger; `Rc`/`Arc` clones and `Copy` types are exempt.
- Why: it hides the ownership design and allocates every frame.
- Enforced by: `clippy::redundant_clone`, `implicit_clone`, `unnecessary_to_owned`, `clone_on_copy`, review.

**RS-DONT-02** Never use `RefCell` or `Rc<RefCell<T>>` as the default for shared app state. Pass `&mut State` or the needed `&mut` field down the widget calls. Where interior mutability is unavoidable, keep each borrow inside one statement, never across a callback or an egui closure, and use `try_borrow_mut` where re-entrancy is possible.
- Why: RefCell moves the borrow rules to runtime; with `panic = "abort"` in release the app dies and loses unsent review state.
- Enforced by: review; `clippy::await_holding_refcell_ref` and `await_holding_lock` for the async case.

**RS-DONT-03** Write `unsafe` only in `platform/` modules. There each block carries a `// SAFETY:` comment with its preconditions, holds one unsafe operation and sits in a safe function; the module carries `#![expect(unsafe_code, reason = "...")]`.
- Why: a small, auditable blast radius; egui needs no unsafe, so UI code never does.
- Enforced by: `unsafe_code` (forbid in the crate roots without FFI, warn plus `-D warnings` elsewhere), `clippy::undocumented_unsafe_blocks`, `multiple_unsafe_ops_per_block`, repo-lint RL007. Edition 2024 adds `unsafe_op_in_unsafe_fn`.

**RS-DONT-04** Never panic on data from outside the process. Provider JSON, git output, diff offsets and settings files use `get()`, checked arithmetic and `?`, not indexing, `unwrap` or `as` narrowing.
- Why: release builds abort on panic, so one malformed response crashes the app.
- Enforced by: `clippy::unwrap_used`, `panic`, `get_unwrap`, `unwrap_in_result`; every module that parses or maps external text adds `#![warn(clippy::string_slice, clippy::indexing_slicing)]` (crate-wide these are noise in egui pixel code); in a UI crate whose root expects the cast lints for pixel math, it also adds `clippy::cast_possible_truncation`, which pedantic already enables in the other crates. `indexing_slicing` does not cover `&str` ranges; `string_slice` does.

**RS-DONT-05** Never drop a `Result` or `#[must_use]` value with `let _ =` or a bare call. Handle it, log it, or bind it to a named `_reason` with a comment that says why it may fail silently.
- Why: a dropped error in a background task is a silent dead end; every error needs a visible action.
- Enforced by: rustc `unused_must_use`, `clippy::let_underscore_must_use`.

**RS-DONT-06** Never block the egui update path: no `block_on`, `std::thread::sleep`, `Command::output`/`status`, unbounded synchronous file reads, or a `Mutex::lock` that a background task holds for long. Network, git and disk work runs on `cv_runtime::background::Background` (a small tokio runtime); a finished task wakes the UI with one repaint, nothing polls.
- Why: `update` runs on the UI thread every repainted frame; a block freezes input and rendering.
- Enforced by: `disallowed-methods` in `clippy.toml` (`#[expect(clippy::disallowed_methods, reason)]` where a worker thread, a test driver or a CLI tool owns the thread), repo-lint RL013 for `block_on` (clippy cannot list `Runtime::block_on`: every `#[tokio::test]` expands to it).

**RS-DONT-07** Never put `#![deny(warnings)]` in source. Warnings become errors only through `-D warnings` on the command line.
- Why: in-source deny turns every new compiler or clippy release into a build break.
- Enforced by: repo-lint RL009.

**RS-DONT-08** Never name a type `*Manager`, `*Service`, `*Helper` or `*Handler`, or a module `utils`, `util`, `helpers`, `helper`, `common` or `misc` (RL004). Name the work (`DiffLoader`, `TokenVault`, `ThreadAnchor`). The `cv-services` crate (the app's connections, writes, checkouts and agents) is a named topic.
- Why: a role suffix names no responsibility, so the type grows without a boundary.
- Enforced by: repo-lint RL003, RL004.

## Dos

**RS-DO-01** Name per the Rust API Guidelines: no `get_` prefix on getters; `as_` (free view), `to_` (expensive or borrowed to owned), `into_` (consumes self). The guidelines are defaults with a written exception, not law.
- Why: the name tells the reader cost and ownership at the call site.
- Enforced by: `clippy::wrong_self_convention` (with `avoid-breaking-exported-api = false`, also on pub items); the `get_` prefix is review.

**RS-DO-02** Pin the toolchain (Toolchain).
- Why: each release adds and moves clippy lints; an unpinned toolchain turns `-D warnings` into a random break.
- Enforced by: `rust-toolchain.toml`, which rustup and CI read.

**RS-DO-03** Keep the crate dependency direction (Workspace).
- Why: a domain crate that pulls in egui links UI code into the domain and makes its tests need a GPU.
- Enforced by: repo-lint RL005, RL006.

**RS-DO-04** Wrap access tokens in a type whose `Debug` and `Display` print only presence, and never derive `Debug` or `Serialize` on a struct that holds the raw string. Tokens live only in the OS store (rule `desktop`).
- Why: a derived `Debug` in a log line or panic message prints the token.
- Enforced by: review.

## Errors and platform code
- Libraries return typed errors (`thiserror`, `review_core::ProviderError`) or `io::Error` with context. `expect` only for true invariants, naming the invariant ("tokio runtime must start").
- Platform code lives in `platform/` modules behind `cfg(target_os = ...)`, one file per system. Shared code never branches on the OS inline.
- After a change to platform code or dependencies, also check the other systems: the Linux clippy pass in the container of `docs/linux.md` (Tests in a container); a `--target x86_64-unknown-linux-gnu` run on Windows or macOS would also need a C cross compiler for aws-lc-sys; and both clippy passes with `--target aarch64-apple-darwin` (needs only `rustup target add aarch64-apple-darwin`; clippy links nothing, so it runs on Windows and Linux too). Where a target is not installed, say NOT-VERIFIED for that system. Why: code behind `cfg(target_os = "macos")` is compiled by no other check on this host, so a finding there stayed unseen until the macOS workflow ran.
- Comments say why, not what. Module docs (`//!`) explain the design and its trade-offs; keep them true when you change the behaviour.

## Readability

**RS-READ-01** Guard clauses with `let ... else` and `?` instead of `if let` or `match` pyramids; let-chains (edition 2024) merge nested `if let` with conditions.
- Why: the success path stays at one indentation level, each condition next to its exit.
- Enforced by: `clippy::manual_let_else`, `question_mark`, `collapsible_if`, `collapsible_match`.

**RS-READ-02** Option and Result combinators (`map`, `and_then`, `ok_or_else`, `unwrap_or_default`) for one-step transforms; `match` when two or more arms carry logic. No chain of more than about four adapters with nested closures: name the intermediate with a `let`.
- Why: combinators are compact for simple transforms; nested `map_or_else` reads worse than a match.
- Enforced by: `clippy::manual_map`, `manual_unwrap_or`, `map_unwrap_or`, `manual_ok_or`; chain length is review. `option_if_let_else` stays off.

**RS-READ-03** At most four levels of control flow inside a method and about 80 lines per function. A growing egui panel function splits into widget functions that take only the data they draw (`fn file_header(ui: &mut egui::Ui, file: &FileDiff) -> Response`).
- Why: immediate-mode code nests by nature; small widget functions are the boundary that keeps a reader oriented.
- Enforced by: `clippy::too_many_lines` (80) and `clippy::excessive_nesting` (threshold 6: clippy counts the impl, the fn body and every closure as blocks). An existing offender without a natural seam carries `#[expect(clippy::too_many_lines, reason)]`.

**RS-READ-04** File length in shipped lines: no file over 700 unless it has an entry in the allowlist, and an allowlisted file never grows past its entry, whatever its length. Above 400, split by topic; split an allowlisted file when you change it. Large test modules move to a sibling `*_tests.rs`, which does not count.
- Why: a file is the module boundary a reader scans; a long one hides its parts.
- Enforced by: repo-lint RL001/RL002 with the shrink-only allowlist `tools/repo-lint/file-lengths.allow`; the split above 400 and of a changed allowlisted file is review (`repo-lint --lengths` lists the files over 400).

**RS-READ-05** Bind a compound condition to a named bool before an `if` that produces a value; no nested if-else in value position; a two-variant enum instead of a bool parameter.
- Why: the name says what the test means; `render(ui, true, false)` is unreadable at the call site.
- Enforced by: `clippy::nonminimal_bool`, `overly_complex_bool_expr`, `fn_params_excessive_bools`, `struct_excessive_bools`; naming is review.

**RS-READ-06** One inherent `impl` block per type, in the module that defines it. Split a large type by extracting sub-types, not by spreading `impl App` across files. Trait impls are exempt.
- Why: scattered inherent impls are a partial class: one blob of private state without a boundary between topics.
- Enforced by: `clippy::multiple_inherent_impl`.

**RS-READ-07** At most seven parameters; group the rest into a struct and name complex types with an alias or newtype.
- Why: long parameter lists and nested generics are the main noise in signatures.
- Enforced by: `clippy::too_many_arguments` (7), `type_complexity`.

**RS-READ-08** Explicit imports; no glob imports except crate preludes and `use super::*` in test modules.
- Why: reader and agent see where each name comes from without an IDE.
- Enforced by: `clippy::wildcard_imports`, `enum_glob_use`.

**RS-READ-09** A module folder holds at most 12 shipped files; above that it splits into sub-modules or carries a `.folder-budget` whose first line gives the reason.
- Why: a flat folder of many files hides its sub-topics the way a long file hides its parts.
- Enforced by: repo-lint RL008.

## Performance

**RS-PERF-01** Treat `update()` as a hot loop: nothing per frame proportional to file or repo size except drawing the visible rows. Cache derived data (highlighting, `LayoutJob`s and galleys, word diffs, markdown and mermaid renders) keyed by a content id or generation, with a size limit and an eviction rule stated in the module doc.
- Why: egui reruns the UI code on every repaint; idle costs nothing only while each repaint is cheap.
- Enforced by: review, profiling (RS-PERF-04).

**RS-PERF-02** Virtualize long lists (diff lines, file trees, threads) with `ScrollArea::show_rows` or `show_viewport`.
- Why: laying out a 50,000-line diff each frame costs milliseconds for rows nobody sees.
- Enforced by: review; frame times from `cv_shell::frame_stats` on a large fixture.

**RS-PERF-03** Never call `ctx.request_repaint()` unconditionally in `update()`. Use `request_repaint_after` for timers and animations; background tasks wake the UI once when data arrives.
- Why: an unconditional repaint turns zero idle CPU into a 60 to 120 Hz busy loop.
- Enforced by: review; idle check of `dist/CodeView.exe` (frame stats show no frames while idle).

**RS-PERF-04** Measure before optimizing: build with `--profile profiling` (release with line tables, not stripped) and profile with samply, Windows Performance Analyzer or the `frame_pacing` example. A perf change lands with a before and after number or a named per-frame cost.
- Why: release strips symbols; without a profile the effort goes to guessed hotspots.
- Enforced by: `[profile.profiling]` in the root `Cargo.toml`, review.

**RS-PERF-05** All `[profile.*]` settings live in the root `Cargo.toml`. `[profile.dev.package."*"] opt-level = 2` keeps dependencies fast while workspace crates rebuild quickly. Change release `lto` or `codegen-units` only with measured build time and binary size, as the existing comment does.
- Why: Cargo ignores profiles in member manifests.
- Enforced by: repo-lint RL010, root `Cargo.toml` review.

**RS-PERF-06** Share immutable text and buffers with `Arc<str>`, `Arc<[T]>` or ids into a store instead of cloning `String` or `Vec` into per-frame widget calls. Take `&str` and `&[T]`, return `Cow` when a value is mostly borrowed, and don't `format!` in per-row render code when a cached string exists.
- Why: per-frame and per-row allocation is the common hidden cost of immediate-mode UIs.
- Enforced by: clippy perf group, `needless_pass_by_value`, `inefficient_to_string`, `format_collect`, `format_push_string`, `large_types_passed_by_value`, `redundant_clone`.

**RS-PERF-07** Run expensive algorithms (`similar` diffs of huge files, whole-file syntect highlighting, mermaid layout) on the background runtime with a size or time budget, and show a placeholder until the result arrives.
- Why: frame time stays bounded whatever a pull request contains.
- Enforced by: review; RS-DONT-06 bans the blocking forms.

## Analyzer
`tools/repo-lint` checks the rules above that clippy cannot: file length (RL001/RL002), names (RL003/RL004), crate direction and UI crates (RL005/RL006), unsafe placement (RL007), folder budgets (RL008), `deny(warnings)` (RL009), member profiles (RL010), workspace dependencies (RL011), TODO comments (RL012), `block_on` in app code (RL013), its own waivers (RL014), the `test-support` feature outside tests (RL015) and `motion::install` outside the app's start-up (RL016). Each check, its waiver comment and its registry files are documented in `tools/repo-lint/README.md`.
- Run: `cargo run -q -p repo-lint [-- [--base <rev>] <paths>]`; the repo-lint step of `sh scripts/check.sh` and CI run it on the whole repository. On a branch, pass `--base main` (or the branch it forked from): the default `HEAD` misses an allowlist raise once it is committed.

## Ship
After every merge of a feature into `main`:
1. `sh scripts/check.sh` and the tests (rule `testing`).
2. `cargo build --release -p review-app`. Only `-p review-app`: a `--workspace --all-targets` build unifies `test-support` into the binary, and the release build refuses it (`cv_base::TEST_SUPPORT`).
3. `pwsh -NoProfile -File scripts/ship.ps1` (allowed without asking): it stops every running `CodeView.exe` and waits until each is gone, copies `target/release/CodeView.exe` to `dist/CodeView.exe` and starts it through `scripts/start-background.ps1`, which never takes the owner's focus (`stole_focus=False`; not `Start-Process`, which activates the window). Why: a copy right after a kill fails while the file is still locked.
4. Tell the owner what is new and where to find it, so they can give feedback on each change. The owner checks the UI; agents never take or read screenshots (rule `ui`).
- macOS bundles come from `scripts/package-macos.sh` (`docs/macos.md`); not runnable on this host.
