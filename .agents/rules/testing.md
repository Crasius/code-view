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
- Walkthroughs wait through `action_e2e::wait_until`/`wait_for`: a wait fails after 60 s with nothing in flight in the Background, or after `LIMIT` (180 s) in total. Never add a fixed wall-clock limit of your own. Budgets in simulated time count only idle frames. Wait for the real completion signal (a link's cursor, an enabled menu item, a store record), never a fixed number of frames. Why: under the full suite's load real work lags far behind kittest's simulated clock.
- Whole-app walkthroughs in `crates/review-app` click through `app::clicks::Clicks` (`click_at`, `secondary_click_at`, `click_point`) or the `press` helpers built on it, never through kittest's `Node::click` or `click_secondary`. Why: `Node::click` runs as three frames (move, press, release) and egui hit-tests the press against the move frame, which the test never read; background work taken in by that frame moves the widget, and the click lands on something else (`clicks.rs`; GitLab jobs 25000 and 25120).
- Never start git or another process inside a per-frame wait condition. On Windows std starts processes one at a time for the whole test binary, so at most `PROCESS_SLOTS` (8, `cv-testkit`, per test binary: each crate's tests are one binary) tests start processes at once, and fixture repositories come from cached templates instead of fresh git commands.
- Memory walkthroughs measure only the app: the wiremock fixture forgets matched requests before each measurement (`Fixture::forget_requests`), because wiremock keeps a copy of every matched request even with request recording disabled.
- Tests never read the machine's reduced-motion setting: a harness pins it with `motion::set_reduced(ctx, false)` (or `true` for a reduced-motion case) or leaves it unset, which animates. Only the binary's `main` calls `motion::install` (repo-lint RL016). Why: GitHub's macOS runners have Reduce Motion on, and every test that asserted a fade, a pulse or a fling failed only there.

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
- What to run: `cargo test -p <crate> [filter]` for the changed crates; the filter is the module path inside that crate. After a change in a low crate, also test the crates above it that use what changed (the layer table, rule `rust`, Workspace).
- Measurements are their own group: tests that are measurements by nature (heap curves over a long session or thousands of frames, loads until the heap settles) compile only with the `review-app` feature `measurements`, so the ordinary `cargo test` leaves them out. They run on `main` and when memory-relevant code changes (caches, grammars, pages kept or dropped): `cargo test -p review-app --bin CodeView --features measurements -- app::memory_walkthrough::memory_`. A new measurement joins that group (module doc of `memory_walkthrough`); hand-run reproductions and probes stay `#[ignore]`, so neither command runs the other group.
- The full `cargo test --workspace` runs only after a merge where conflicts were resolved by hand or `review-core` changed. The lead merges ready lanes in a batch, then runs checks and one targeted test run over the crates in `git diff --stat <last tested commit> HEAD`, then ships once.

## Live checks
- Live checks are `#[ignore]`d tests that reach real hosts; only the owner runs them. The tracked source never names a real organisation, repository, login or private host, not even as a default: tests and walkthroughs use invented names (`contoso`, `fabrikam`, `acme`, `octo`, `example.com`), and a live check reads its real targets at run time.
- The live write walkthroughs (`crates/review-app/src/app/live_writes.rs`) read their repositories from `live-writes.json` in the owner's config folder, beside `connections.json`, outside every checkout. A run without the file, the platform's entry or one of its fields fails with the file's path and the expected shape (`hosts::TARGETS_FORMAT`). The read-only public checks (`live_check.rs`) take public repositories as defaults and `REVIEW_LIVE_*` variables to point elsewhere.
