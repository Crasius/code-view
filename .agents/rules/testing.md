---
paths:
  - "crates/**"
  - "tools/**"
---

# Testing: what gets a test, where tests live, how to run them

A test earns its place by catching a bug that matters at a maintenance cost below that value. Decide each test with the matrix below, not by coverage. The rule ids `RS-TEST-*` follow the id rules of rule `rust`.

## Unit-test matrix
| Function kind | Example here | Verdict | Cheaper alternative or how |
|---|---|---|---|
| Pure domain logic with branching | which favorites a role hides; which data folders retention purges | **must-test** | Encode impossible states in enums; test the remaining rules through the public fn. |
| Diff algorithms, line and anchor mapping | map a comment's old line to the new revision; whitespace-only changes; word diff | **must-test** | Table-driven cases; property tests only if the dependency is accepted. Highest bug density in a review app. |
| Parsers of external text | `@@ -a,b +c,d @@` hunks, `git diff --numstat`, provider JSON quirks, URLs, workflow `needs` | **must-test** | Fixtures of real outputs plus edge cases (empty, binary, rename, CRLF, non-UTF-8). |
| Provider DTO to core mapping | GitHub review comments JSON to `review_core::Thread` | **must-test** | JSON fixtures in `crates/<crate>/tests/fixtures/` served by wiremock; assert the mapped core values, not the DTO fields. |
| Persistence formats and settings migration | load a `settings.json` of the previous version; token vault round trip | **must-test** | One fixture per schema version that must still load, plus a round trip. |
| State machines and async coordination | a stale result for a switched pull request is dropped; a load error surfaces once | **must-test** | Test the transition as a pure `(state, event) -> state` fn; one walkthrough covers the tokio wiring. |
| UI interaction logic extracted from widgets | next-unreviewed-file navigation; j/k across hunks | **must-test** | Plain functions on a model struct, tested without an egui `Context`. |
| Bug regressions | a comment jumped lines after a CRLF-only change | **must-test** | At the lowest level that reproduces the bug, named after the behaviour (RS-TEST-03). |
| API client paging and error mapping | follow Link headers; 401 to `ProviderError::Unauthorized` with a sign-in action | **test-if-cheap** | A few wiremock tests per provider for paging, auth failure, rate limit. Never mock reqwest. |
| Platform FFI wrappers | DPAPI encrypt and decrypt; window placement | **test-if-cheap** | One round trip per OS where it runs without UI; otherwise SAFETY comments and a manual check, NOT-VERIFIED elsewhere. |
| Error-to-action mapping | a missing token yields a hint with a "Sign in" action | **test-if-cheap** | Assert the action variant, not the wording. |
| Cache invalidation of perf paths | highlight cache, galley cache | **test-if-cheap** | Test the invalidation logic; timings belong in examples (`frame_pacing`) or `cv_shell::frame_stats`, never in assertions. |
| UI layout and render code | `file_header` draws icons, path and counts; theme colour tables | **skip** | Move decisions into pure functions and test those; cover critical flows with a few walkthroughs that assert geometry and state. |
| Icon, asset and image generation | rasterize each toolbar SVG at 1x, 2x, 3x | **delete** | At most one aggregate test that every embedded asset decodes or rasterizes (RS-TEST-07). |
| Getters, newtypes, one-line `From`, derives | `impl From<GhUser> for Author`; `#[derive(Default)]` | **skip** | The type system covers them; mapping and domain tests use them. Delete such tests when you touch them. |
| Defaults and constants | `Settings::default()` font size is 14 | **skip** | A test that restates a constant doubles the edit. An invariant between constants is a `const` assertion. |
| Glue and wiring | `main`, eframe setup, service construction, font loading | **skip** | Every walkthrough and the start of `dist/CodeView.exe` after a ship exercise it. |

## Walkthroughs
- Whole-app walkthroughs (`*_walkthrough.rs`, `*_e2e.rs`, `*_check.rs`) drive the app through `egui_kittest`. Keep the ones that drive a critical flow: open a pull request, review a file, comment, switch file, pipelines, setup. Merge overlapping ones.
- They live next to the feature or, when they drive the whole app, in `crates/review-app/src/app/`, and build services through `cv_testkit`, so they stay hermetic: loopback hosts only, no system token store, a watchdog against hangs.
- A test helper that another crate's tests need sits under `#[cfg(any(test, feature = "test-support"))]` and is `pub` along its path; a helper only the own crate's tests use stays `#[cfg(test)]`. Each crate's `[dev-dependencies]` turns the feature on for the `cv-*` crates it uses (rule `rust`, Workspace).
- No screenshot tests. A walkthrough asserts state (accessibility tree, geometry, focus); it never renders offscreen or writes an image. Why: the owner judges the look in the shipped build, and offscreen rendering serialized the suite into many minutes.
- Walkthroughs wait through the kit in `crates/review-app/src/app/walkthrough.rs` (its module doc states the waits): `walkthrough::wait_until`, `wait_until_unsettled` and `wait_for`. `wait_until` runs four more frames after its condition held, so what that frame started has landed; `wait_until_unsettled` returns on that frame, for a test that reads a state the next frames change. A wait fails after 60 s with nothing in flight in the Background, or after `LIMIT` (180 s) in total. Never add a fixed wall-clock limit of your own. Budgets in simulated time count only idle frames. Wait for the real completion signal (a link's cursor, an enabled menu item, a store record), never a fixed number of frames. Why: under the full suite's load real work lags far behind kittest's simulated clock.
- Every kittest test, whole-app walkthrough or component test, clicks through `cv_testkit::Clicks` (`harness.click_at(node.rect())`, `secondary_click_at`, `click_point`) or the helpers built on it (`walkthrough::press`, `secondary_press`); a hover-only widget gets `Node::hover` and a step first. kittest's `Node::click`, `click_secondary`, `click_button`, `click_modifiers` and `click_button_modifiers` fail clippy's `disallowed-methods` (root `clippy.toml`). Why: `Node::click` runs as three frames (move, press, release) and egui hit-tests the press against the move frame, which the test never read; background work taken in by that frame moves the widget, and the click lands on something else (`clicks.rs`; GitLab jobs 25000 and 25120).
- Never start git or another process inside a per-frame wait condition. On Windows std starts processes one at a time for the whole test binary, so at most `PROCESS_SLOTS` (8, `cv-testkit`, per test binary: each crate's tests are one binary) tests start processes at once, and fixture repositories come from cached templates instead of fresh git commands.
- Memory walkthroughs measure only the app: the wiremock fixture forgets matched requests before each measurement (`Fixture::forget_requests`), because wiremock keeps a copy of every matched request even with request recording disabled.
- Tests never read the machine's reduced-motion setting: a harness pins it with `motion::set_reduced(ctx, false)` (or `true` for a reduced-motion case) or leaves it unset, which animates. Only the binary's `main` calls `motion::install` (repo-lint RL016). Why: GitHub's macOS runners have Reduce Motion on, and every test that asserted a fade, a pulse or a fling failed only there.

### Writing a walkthrough
- The scenario comes from `GitHubHost` (`github_mock.rs`) when the test asserts requests, from a `cv_testkit::provider::Script` (made a provider by `Scripted`) when it is not about HTTP, and from `mock_host::MockHost` for a host of the scenario's own (GitLab, Azure DevOps). The app starts through `walkthrough::app` (real providers on mock hosts) or `walkthrough::app_with` (the test's own providers). Waits, clicks and queries come from `walkthrough.rs`; a helper two walkthroughs need moves there or into a scenario module, never into another walkthrough file. Unit tests and crate-level scenes wait through `cv_testkit::poll_until`/`poll_for`.
- A wait names its signal: `wait_until`, `wait_until_unsettled` or `wait_for` for a state, `wait_until_still` for a place at rest, `hover_until` for a tooltip. Why `hover_until`: egui shows no tooltip while the last click is newer than the pointer's last move, so a hover after a click needs the pointer move it sends. A fixed frame count stays only where the count is what the test measures, and then it is a named constant; after pure UI input on data already loaded (a key, a click on a widget already laid out) `harness.run_steps(n)` lets that input land.
- A request count taken after a settle on a page with a periodic request (the live probe, the list's refresh) puts the window in the background first (`walkthrough::set_focus(harness, false)`). Why: a settle steps simulated time for as long as wall-clock work is open, and a slow machine steps past the period.
- A count on the wire cannot see a read that joined a GET in flight (`review_core::http`), so a hand-over is proved by where the data came from (`PageCache::stored_at`), not by the count.
- The HTTP write epoch is one per process, and `cargo test` runs a binary's tests as threads of it: a test that counts reads while another test of the same binary may write opens a `walkthrough::SharingWindow` and asserts through its `NeighbourWrites`.

## Rules

**RS-TEST-01** Decide each test with the matrix. Unit-test complex, domain-significant logic; cover glue, controllers and UI flows with a few walkthroughs or wiremock integration tests; trivial code gets no test.
- Why: unit tests pay off for domain logic and algorithms; the cost here is over-testing trivial code.
- Enforced by: review (RS-AGENT-08).

**RS-TEST-02** Assert observable behaviour through the module's public functions (inputs to outputs, state before and after), not private helpers, call order or internal fields. A test that fails on a behaviour-preserving refactor is rewritten against behaviour or deleted.
- Why: resistance to refactoring keeps tests from becoming a burden.
- Enforced by: review.

**RS-TEST-03** Every fixed bug gets one regression test at the lowest level that reproduces it, named after the behaviour (`keeps_comment_on_line_after_whitespace_only_change`). A test whose name or doc names a fixed bug stays.
- Why: a real bug is the strongest evidence that a path is critical and error-prone.
- Enforced by: review; the fix commit names the test (AGENTS.md, Done).

**RS-TEST-04** No coverage targets. If coverage is measured at all (`cargo llvm-cov`), read it per crate: high for `review-core`, `review-git` and provider mapping, low for widgets and platform code.
- Why: coverage inflates with tests of trivial code and says nothing about their quality.
- Enforced by: nothing mechanical, by design.

**RS-TEST-05** Snapshot tests (insta) only for large structured text output that is tedious to assert by hand. Run, read the diff, then accept. Small values use `assert_eq!`. Never image snapshots (see Walkthroughs).
- Why: a snapshot nobody reviewed is a change detector, not a test.
- Enforced by: review of `.snap` diffs.

**RS-TEST-06** Tests are hermetic and fast: no sleeps where a channel or an injected clock works, loopback-only HTTP through wiremock, temp folders through `AppPaths::new`, live-host checks `#[ignore]` with `allow_real_network` and read-only.
- Why: a slow or flaky suite gets skipped, and a skipped suite protects nothing.
- Enforced by: `clippy::disallowed_methods` on `std::thread::sleep` (a poll loop that must wait carries `#[expect(clippy::disallowed_methods, reason)]`), `cv_testkit::assert_hermetic`, `clippy::ignore_without_reason`.

**RS-TEST-07** No per-icon or per-asset generation tests. At most one test decodes or rasterizes every embedded asset.
- Why: asset generation has low complexity and low domain value; one aggregate check catches a corrupt asset.
- Enforced by: review.

## Placement and naming
- Unit tests live next to the code in `#[cfg(test)] mod tests`, or in a sibling `*_tests.rs` when the module is large (it does not count toward RS-READ-04). Provider responses are JSON fixtures in `crates/<crate>/tests/fixtures/`.
- A test name states the behaviour as a snake_case sentence; the module or file names the subject (`mod line_mapping { fn keeps_anchor_when_lines_inserted_above() }`).

## Running
- Test once, at the end: implement the whole change first (`cargo check` and clippy while coding are fine), then run the tests of what changed once, collect every failure, fix them all in one pass and make one confirming run. No test run after each small step, no run-and-wait loops. Why: the owner wants fast turnaround; walkthroughs take minutes per run and the waiting dominated the work.
- What to run: `sh scripts/check.sh test --base <base branch>`. It compares the work tree (committed, uncommitted and untracked files) with the fork point of the base, maps the changed files to their packages and every package that depends on them (`scripts/affected-tests.sh`), and runs their tests as both CIs do (`ci/run-tests.sh`): cargo-nextest, one process per test, for every test binary but `review-app`'s, and `review-app`'s binary in one process; a changed shared input (the lockfile, the root manifest, CI configuration, cv-testkit, a build script) runs the whole workspace. Without `--base` it compares with `main`. It needs the pinned cargo-nextest (`sh ci/install-build-deps.sh cargo-nextest`) and fails without it. Why: `cargo test` runs the tests of a binary as threads of one process, so tests coupled through process-wide state fail or pass by their neighbours; two such couplings showed only under the local `cargo test` and never in CI's nextest run [observed 2026-11-27], and the gate's result has to be the one CI reports. For one module inside a crate: `cargo test -p <crate> <module path> --no-fail-fast`. A run that is meant to show every failure carries `--no-fail-fast`, since cargo stops at the first failing test binary otherwise.
- Measurements are their own group: tests that are measurements by nature (heap curves over a long session or thousands of frames, loads until the heap settles) compile only with the `review-app` feature `measurements`, so the ordinary `cargo test` leaves them out. They run on `main` and when memory-relevant code changes (caches, grammars, pages kept or dropped): `cargo test -p review-app --bin CodeView --features measurements -- app::memory_walkthrough::memory_`. A new measurement joins that group (module doc of `memory_walkthrough`); hand-run reproductions and probes stay `#[ignore]`, so neither command runs the other group.
- The full run, `sh ci/run-tests.sh 'all()'` (every test, with the gate's runner), runs only after a merge where conflicts were resolved by hand or `review-core` changed. The lead merges ready lanes in a batch, then runs checks and one targeted test run over the crates in `git diff --stat <last tested commit> HEAD`, then ships once.

## Live checks
- Live checks are `#[ignore]`d tests that reach real hosts; only the owner runs them. The tracked source never names a real organisation, repository, login or private host; the only defaults that name real ones are the public repositories of the read-only public checks (below). Tests and walkthroughs use invented names (`contoso`, `fabrikam`, `acme`, `octo`, `example.com`), and a live check reads its real targets at run time.
- The live write walkthroughs (`crates/review-app/src/app/live_writes.rs`) read their repositories from `live-writes.json` in the owner's config folder, beside `connections.json`, outside every checkout. A run without the file, the platform's entry or one of its fields fails with the file's path and the expected shape (`hosts::TARGETS_FORMAT`). The read-only public checks (`live_check.rs`) take public repositories as defaults and `REVIEW_LIVE_*` variables to point elsewhere.
