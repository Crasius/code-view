# macOS pain points: first run on a real Mac

Companion to [macos.md](macos.md). First hands-on run on a Mac, 2026-10-19.

Machine: Apple silicon (arm64), macOS 26 (Darwin 25.6), Xcode installed, Rust 1.99.1 via rustup.

## What worked

- `cargo build -p review-app --bin CodeView` builds without errors (about 2m20s cold, 20 s
  incremental).
- `./target/debug/CodeView` starts, stays up and writes nothing to stderr. The window has not
  been clicked through by hand yet.

## Run 1, commit 2c1ca80: 850 passed, 5 failed, 11 ignored

`cargo test --workspace` finished in about a minute. Five failures, all in `review-app`:

1. **Emoji in semibold text panics** (2 tests)
   - `widgets::markdown::render_tests::markdown_sample_renders_in_light_and_dark`
   - `widgets::markdown::render_tests::scale_grows_the_rendered_height`
   - `epaint-0.37.0/src/text/fonts.rs:806`: `No glyph for "🚀" (U+1F680) in Name("semibold")`.
     Installed fonts: `ui-semibold`, `ui`, `Ubuntu-Light`, `egui-icons`, `phosphor-bold`, `symbol`.
   - The semibold family seems to have no fallback to egui's bundled emoji font on macOS. If so,
     the real app can crash on a PR description with emoji in bold text. Likely fix: add the
     emoji font to the semibold family's fallbacks, or use `MissingGlyphPolicy::Tofu`.
   - **Status:** in progress on `feature/linux-platform`, which reworks the font families and
     adds the emoji and symbol fallbacks to every family chain.

2. **Log viewer copy and select-all do nothing** (2 tests)
   - `features::pull_request::pipelines::log_viewer::copy_tests::copy_log_puts_plain_text_on_the_clipboard`:
     no node labelled `Next match (Enter)` (`copy_tests.rs:111`).
   - `features::pull_request::pipelines::log_viewer::selection_tests::ctrl_a_selects_the_whole_log_once_it_has_focus`:
     "the log never reached the clipboard" (`copy_tests.rs:92`).
   - Suspected cause: the tests press Ctrl, but on macOS `Modifiers::COMMAND` is ⌘. The tests
     should send `Modifiers::COMMAND`.
   - **Status:** fixed on `feature/macos-painpoints-fixes`. The tests already sent
     `Modifiers::COMMAND`; the causes were elsewhere. The copy test looked for the label
     "Next match (Enter)", which macOS spells "Next match (↩)"; it now uses the app's own
     constant. The select-all test lost a fast copy: `key_press_modifiers` queues four events,
     one frame each, and a copy finished within those frames (a fast SSD) landed in a frame whose
     output nobody read. The tests now keep every pass's copied text. The other hard-coded
     shortcut labels in tests and two in the app ("Shift+Enter on a focused row", "(Enter)" on
     the file cards) go through `shortcut!` too.

3. **Merge options popup leaves the window** (1 test)
   - `app::clip_audit::clip_audit_every_screen_keeps_its_text_and_controls_inside`
   - `06-merge-options 960@2x: popup frame [493,515 400×64] … merge_button … menu leaves the
     window [0,0 960×540]`.
   - **Status:** fixed on `feature/macos-painpoints-fixes`. The audit itself measured the wrong
     sizes: in the frame where the scale changes, egui takes the window from the frame before
     (1920 × 1080 at 1x became 960 × 540 at 2x) and ignores the size the harness sends, on every
     system. It now changes the scale in a frame of its own, checks that each state is inspected
     at the size it names, and adds a short window (960 × 540 at 2x) on purpose. The reported
     rect (400 wide, inside the menu's margin) is most likely a strategy row inside the menu's
     scroll area, which the check counted past the window although the scroll area clips it;
     the check now flags only what the window's edge cuts, and compares overlapping text row by
     row (a link wrapped after other text starts mid-line). The menu now picks its side itself:
     below the button with 240 points or more of room there (or more room than above), else
     above it, in both cases no taller than that room and narrower in a narrow window, so it
     never covers the button and never leaves the window. NOT-VERIFIED on a Mac.

## Run 2, commit 9fb7762: the review-app tests hang

After pulling the Favorites lane, title bar search sizing and comment hand-over merges, the
`review-app` unit test binary (470 tests) did not finish. It was stopped after 15 minutes: 322
passed, 1 failed, the rest stuck. In run 1 the same binary finished in 59 s. Every other test
binary in the workspace passed: 415 passed, 0 failed, 1 ignored. The app itself still builds and
starts.

Tests still running after 60 s and stuck there:

- `app::clip_audit::clip_audit_every_screen_keeps_its_text_and_controls_inside`
- `app::comment_origin_walkthrough::comment_origin_shows_the_version_an_outdated_thread_was_written_on`
- `app::frame_rate_walkthrough::frame_rate_limit_defaults_to_45_applies_live_and_is_saved`
- `app::live_updates_e2e::live_push_while_viewing_applies_quietly_and_keeps_the_file`
- `app::live_updates_e2e::live_reply_while_writing_waits_for_reload_and_keeps_the_draft`
- `app::merge_button_walkthrough::merge_button_is_disabled_with_conflicts_and_warns_when_blocked`
- `app::merge_button_walkthrough::merge_button_loads_no_policy_while_the_window_is_inactive`
- `app::merge_button_walkthrough::merge_button_screenshots`
- `app::overview_walkthrough::overview_collapsed_group_is_remembered_and_loads_nothing_until_expanded`
- `app::overview_walkthrough::overview_files_button_opens_the_files_tab_and_not_the_row`
- `app::overview_walkthrough::overview_files_button_screenshots`
- `app::overview_walkthrough::overview_header_is_one_row_at_wide_and_narrow_widths`

Failed:

- `app::comment_origin_walkthrough::comment_origin_falls_back_to_the_comments_lines_when_the_commit_is_gone`
  (no message, since the run was stopped before the summary printed)
  **Status:** see below; most likely the shared Keychain items, not the test.

All the stuck tests drive the whole app. The new Favorites lane on the overview is the main
suspect, for example a load that waits on a provider the tests do not mock. This has not been
investigated. The macOS CI workflow would show the same hang: its only run so far was cancelled.

**Status:** fixed on `feature/macos-painpoints-fixes` (found by reading the code; NOT-VERIFIED on
a Mac). The Favorites lane is not the cause. Every whole-app test saves its connections through
`ConnectionStore`, which kept tokens in the platform store, and only the store's own tests used a
test service name: the app's tests wrote, read and deleted real login Keychain items (service
"Code View", accounts such as `gh`, `gl-1`, `clip-audit`), the same items the installed app uses.
Run 1 created them. Run 2's rebuilt test binary has another code signature, so macOS asked
whether it may read those items, and `SecItemCopyMatching` waits until someone answers that
dialog. Twelve test threads blocked there or behind the rendering lock one of them held (12 is
the thread count; the remaining tests never started). Tests running side by side also shared
these items: one deleting `gh` while another read it explains the failed
`comment_origin_falls_back_to_the_comments_lines_when_the_commit_is_gone` (its connections did
not load). The test itself has no platform-dependent part (git runs with fixed config, every
fetch error leads to the fallback). Now `AppPaths::new` (all tests) keeps tokens in its own
connections file and never calls the Keychain; `AppPaths::from_environment` (the app) still uses
it. Services built in a test check that, their HTTP client resolves no host but `localhost`
(mock servers listen on 127.0.0.1; any other host fails at once and names itself), and a
watchdog stops the test binary with a message once a test runs five minutes. The Keychain's own
round trip is `--ignored`. Clean-up after runs 1 and 2: Keychain Access still lists the test
items under "Code View" (accounts such as `gh`, `ado`, `gitlab`, `clip-audit…`, `local-…`);
those that are no connection of yours can go. Some tests used the ids the setup wizard gives
(`gh-1`, `gl-1`), so a token of Code View's own connection with such an id may have been
overwritten or deleted: if a connection asks for its token again or answers 401, enter it again
in Settings.

## Run 3, GitHub run 45853777892 (commit cc50530): the first complete CI run

The first macOS workflow run that got past the lints (`macos-latest`, arm64). Format, repo-lint,
both clippy passes and the traffic-light example passed. The test step compiled for about four
minutes, then ran the crates in order up to `cv-pr-pipelines`: every binary before it passed,
and that one failed 1 of 86 (85 passed, 2 ignored). Without `--no-fail-fast` cargo stopped
there, so `cv-pr-rendered` and every crate after it (`cv-pr-summary`, `cv-pr-threads`,
`cv-pr-tree`, `cv-settings`, `review-app` and more) have not run on a Mac in CI yet.

- **The stage graph did not coast after a fling**:
  `stage_pan_check::a_release_mid_drag_flings_then_stops_repainting`, "the graph coasts on after
  the release" (`stage_pan_check.rs:172`). It passes on Windows and on Linux.
  - Found by reading the code: the fling's velocity is set only without reduced motion
    (`graph/pan_canvas.rs:398`), and the test's harness called `theme::motion::install`, which
    reads the machine's Reduce Motion setting (`cv-base` `theme/motion.rs:20`,
    `platform/macos.rs:112`). Every other input on that path is deterministic: kittest sets only
    `predicted_dt` and no wall-clock time, so egui's pointer time, velocity and `stable_dt` are
    the same on every host. The same press, moves and release already pan 1:1 in
    `a_drag_on_empty_canvas_pans`, which passed on the Mac. The runner's Reduce Motion being on
    is inferred, not seen: NOT-VERIFIED. The workflow now prints it (step "Host motion
    setting"), and the assertion prints both positions and the motion setting.
  - **Status:** fixed on `macos-tests`: the harnesses of `cv-pr-pipelines` set motion on
    themselves (`motion::set_reduced(ctx, false)`) instead of reading the OS. NOT-VERIFIED on a
    Mac.
  - Other harnesses still call `motion::install` (in `cv-pr-diff`, `cv-pr-files`, `cv-pr-page`,
    `cv-pr-rendered`, `cv-pr-summary`, `cv-pr-threads`, `cv-pr-tree`, `cv-settings` and the whole
    app). None of them asserts motion, so they should pass either way; the first three passed on
    the Mac.
- **The workflow**: the test step now runs with `--no-fail-fast`, so one run lists every failing
  test, and the job stops after 45 minutes (our own budget, from the 20 minutes this run took up
  to the failing crate), so a hang like run 2's ends the job instead of running for hours.

## Other friction

- **Nothing to install from.** There is no released `.app`, so testing means installing Rust
  and building from source.
  **Status:** documented: macos.md, "Trying it without building", downloads the CI bundle
  (`gh run download … --name CodeView-macos-universal`). A release is still open.
- **Ad-hoc signing.** An ad-hoc signed CI bundle that is downloaded on another Mac is blocked by
  Gatekeeper until its quarantine flag is cleared (see macos.md).
  **Status:** documented: `xattr -dr com.apple.quarantine "Code View.app"` in the same section.
  Developer ID signing and notarization stay open.
- **Git over SSH with two GitHub accounts.** The repository was private then. With `gh` set to SSH and the
  SSH key bound to another GitHub account, `gh repo clone` fails. Cloning over HTTPS with the
  right account's token works.
  **Status:** documented: macos.md, "Getting the source" (`gh auth switch`, `gh auth setup-git`,
  clone over HTTPS).
