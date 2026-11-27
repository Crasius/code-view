//! Focus mode end to end: the real app against the mocked GitHub host, its title bar drawn as on
//! a Windows window. The diff of a long file and a job's log each take over the window and give
//! it back by F11, Esc, the toolbar and strip buttons and the Ctrl+K search; the title bar, the
//! navigation rail, the pull request header and the tabs leave, the strip stays within 28 points
//! with reachable caption buttons and a draggable empty area, nothing in it moves from frame to
//! frame, and the line at the top of the view stays exactly where it was. The log's strip takes
//! the attempt switcher, a refused rerun floats under it with its ways out, and a comment's
//! original version says in the strip what its banner says (Esc leaves it before focus mode).

use std::fmt::Write as _;

use egui::{Key, Modifiers, Rect, Role, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::ReviewApp;
use super::github_mock::{ExtraFile, FILE, GitHubHost, ReplacedHead, open_pull_request};
use super::walkthrough::{self, wait_until};
use cv_base::platform::{self, hit_test, shortcut};
use cv_pr_rendered::file_header::NEXT_CHANGE_HINT;
use cv_shell::focus_mode::{self, ENTER_HINT, EXIT_HINT, Surface, strip};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const LONG_FILE: &str = "src/long.rs";
const JOB: &str = "Build and test";
const LOG_LINES: usize = 3000;
/// The refused rerun's message, in the run header's place and in focus mode under the strip.
const RERUN_REFUSED: &str = "did not start";
const SWITCH_ATTEMPT: &str = "Attempt 2 of 2. Switch attempt";
const SHOW_ORIGIN: &str = "Show original version";
const BACK_LABEL: &str = "Back to latest";
const MARK_VIEWED: &str = concat!("Mark as viewed (", shortcut!(alt "V"), ")");
const UNMARK_VIEWED: &str = concat!("Viewed: click or ", shortcut!(alt "V"), " to unmark");
const NEXT_MATCH: &str = concat!("Next match (", shortcut!(Enter), ")");
const COPY_LOG: &str = concat!("Copy log (", shortcut!(command shift "C"), ")");

/// 600 lines, every eighth one changed: 75 changes to step through.
fn long_file() -> ExtraFile {
    let line = |index: usize, changed: bool| {
        if changed {
            format!("    let value_{index} = compute({index}) + 1; // changed\n")
        } else {
            format!("    let value_{index} = compute({index});\n")
        }
    };
    let base: String = (0..600).map(|index| line(index, false)).collect();
    let head: String = (0..600).map(|index| line(index, index % 8 == 4)).collect();
    ExtraFile { path: LONG_FILE, base: base.into_bytes(), head: head.into_bytes() }
}

fn long_log() -> String {
    (0..LOG_LINES).fold(String::new(), |mut text, index| {
        writeln!(
            text,
            "2026-10-01T10:{:02}:{:02}.0000000Z step {index}: compiling crate_{index}",
            index / 60 % 60,
            index % 60
        )
        .unwrap();
        text
    })
}

fn app(name: &str) -> (ScratchDir, GitHubHost, Harness<'static, ReviewApp>) {
    let scratch = ScratchDir::new(name);
    let host = GitHubHost::with_pull_request_files(scratch.path(), &[long_file()]);
    host.add_workflow_job(JOB, &long_log());
    let mut harness = walkthrough::app(&scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().title_bar.integrate_for_test();
    // Alt+V keeps the open file, so its place can be checked after it.
    harness.state_mut().services.settings.open_next_unviewed = false;
    open_pull_request(&mut harness);
    (scratch, host, harness)
}

/// The system's focus mode key: F11, ⌃⌘F on macOS.
fn toggle(harness: &mut Harness<'_, ReviewApp>) {
    let key = platform::FOCUS_MODE_KEY;
    harness.key_press_modifiers(key.modifiers, key.logical_key);
}

fn rect_of(harness: &Harness<'_, ReviewApp>, role: Role, label: &str) -> Rect {
    harness.get_by_role_and_label(role, label).rect()
}

/// The change the strip's counter ("18 of 75") names.
fn change_in_focus(harness: &Harness<'_, ReviewApp>) -> usize {
    let counter = harness
        .query_all_by_role(Role::Label)
        .find_map(|node| node.accesskit_node().value().filter(|text| text.ends_with(" of 75")))
        .expect("the change counter");
    counter.trim_end_matches(" of 75").parse().expect("a change number")
}

fn top_line(harness: &Harness<'_, ReviewApp>, surface: Surface) -> (usize, f32) {
    focus_mode::top_line(&harness.ctx, surface).expect("the view noted its top line")
}

/// The chrome of a pull request page: breadcrumbs in the title bar, the rail, the tabs, the header.
fn chrome_visible(harness: &Harness<'_, ReviewApp>) -> [bool; 4] {
    [
        harness.query_by_role_and_label(Role::Link, review_core::APP_NAME).is_some(),
        harness.query_by_role_and_label(Role::Button, "Settings and connections").is_some(),
        harness.query_by_role_and_label(Role::Tab, "Overview").is_some(),
        harness.query_all_by_label_contains(super::github_mock::TITLE).next().is_some(),
    ]
}

/// The strip is at the top, spans the window, keeps its height budget and its caption buttons
/// answer the window's hit test; its empty area drags the window, its buttons stay clickable.
fn assert_strip(harness: &Harness<'_, ReviewApp>, width: f32, exit_label: &str) {
    let bar = rect_of(harness, Role::Toolbar, strip::LABEL);
    assert_eq!(
        (bar.top(), bar.left(), bar.width()),
        (0.0, 0.0, width),
        "the strip spans the top of the window"
    );
    assert!(bar.height() <= strip::HEIGHT, "the strip is {} points tall", bar.height());
    let regions = platform::published_caption().expect("the strip published the caption");
    let frame =
        hit_test::WindowFrame { is_maximized: false, resize_border: 8, width: width as i32 };
    let at =
        |point: egui::Pos2| hit_test::hit_test(point.x as i32, point.y as i32, &regions, frame);
    for (label, button) in [
        ("Minimize", hit_test::CaptionButton::Minimize),
        ("Maximize", hit_test::CaptionButton::Maximize),
        ("Close", hit_test::CaptionButton::Close),
    ] {
        let rect = rect_of(harness, Role::Button, label);
        assert!(bar.contains_rect(rect), "{label} sits in the strip");
        assert_eq!(
            at(rect.center()),
            hit_test::Hit::Button(button),
            "{label} answers as the caption button"
        );
    }
    assert_eq!(
        rect_of(harness, Role::Button, "Close").right(),
        width,
        "close reaches the window's corner"
    );
    let exit = rect_of(harness, Role::Button, exit_label);
    assert_eq!(
        at(pos2(exit.center().x, 20.0)),
        hit_test::Hit::Client,
        "the strip's buttons stay clickable"
    );
    let empty = pos2(bar.center().x, 20.0);
    assert_eq!(at(empty), hit_test::Hit::Caption, "the empty middle drags the window");
}

/// The strip's controls keep their rects over consecutive frames.
fn assert_stable(harness: &mut Harness<'_, ReviewApp>, labels: &[&str]) {
    let rects = |harness: &Harness<'_, ReviewApp>| {
        labels.iter().map(|label| harness.get_by_label(label).rect()).collect::<Vec<_>>()
    };
    let first = rects(harness);
    for _ in 0..3 {
        harness.step();
        assert_eq!(rects(harness), first, "nothing in the strip moves between frames");
    }
}

#[test]
fn focus_mode_gives_the_diff_the_window_and_keeps_its_place() {
    let (_scratch, _host, mut harness) = app("focus-files");
    walkthrough::files_tab(&mut harness);
    walkthrough::open_in_tree(&mut harness, LONG_FILE);
    walkthrough::wait_for_diff(&mut harness, LONG_FILE);
    // N steps through the changes: the diff scrolls well into the file.
    for _ in 0..20 {
        harness.key_press(Key::N);
        harness.step();
    }
    harness.run_steps(10);
    assert_eq!(chrome_visible(&harness), [true; 4], "the page shows its chrome");
    let before = top_line(&harness, Surface::Files);
    assert!(before.0 > 50, "the diff scrolled into the file: {before:?}");

    // F11: the diff takes the window; the line at the top stays.
    toggle(&mut harness);
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [false; 4], "title bar, rail, tabs and header leave");
    assert_eq!(top_line(&harness, Surface::Files), before, "the top line stays exactly in place");
    assert_strip(&harness, 1440.0, EXIT_HINT);
    assert!(
        harness.query_by_role_and_label(Role::TreeItem, LONG_FILE).is_none(),
        "the file tree folds away"
    );
    // The header had no room for the change counter beside the tree; the strip has. N, one of
    // the diff's keys, names the change it steps to there.
    harness.key_press(Key::N);
    harness.run_steps(3);
    let change = change_in_focus(&harness);
    assert!(change > 10, "the counter names the change in focus: {change}");
    let position = format!("{change} of 75");
    assert_stable(&mut harness, &[EXIT_HINT, NEXT_CHANGE_HINT, MARK_VIEWED, "Close", &position]);

    // The diff's keys keep working.
    harness.key_press(Key::N);
    harness.run_steps(3);
    assert!(
        walkthrough::shows(&harness, &format!("{} of 75", change + 1)),
        "N steps to the next change"
    );
    let in_focus = top_line(&harness, Surface::Files);
    harness.key_press_modifiers(Modifiers::ALT, Key::V);
    harness.run_steps(3);
    assert!(walkthrough::shows(&harness, UNMARK_VIEWED), "Alt+V marks the file viewed");

    // Ctrl+B shows the file tree and remembers it; the diff narrows without losing its line.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::B);
    harness.run_steps(3);
    assert!(
        harness.query_by_role_and_label(Role::TreeItem, LONG_FILE).is_some(),
        "Ctrl+B shows the tree"
    );
    assert!(walkthrough::saved_settings(&harness).focus_file_tree, "the choice is saved");
    assert_eq!(top_line(&harness, Surface::Files), in_focus);
    let tree_row = harness.get_by_role_and_label(Role::TreeItem, LONG_FILE).rect();
    // On the left by default; tree_side_walkthrough moves it right.
    assert!(tree_row.right() < 720.0, "the tree sits left of the diff: {tree_row:?}");
    let switch = harness.get_by_label_contains("Hide the file tree").rect();
    assert!(switch.right() < 360.0, "its switch sits at the strip's left end: {switch:?}");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::B);
    harness.run_steps(3);
    assert!(!walkthrough::saved_settings(&harness).focus_file_tree);

    // Esc gives the window back, with the line still on top.
    harness.key_press(Key::Escape);
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [true; 4], "Esc brings the chrome back");
    assert_eq!(top_line(&harness, Surface::Files), in_focus, "and the top line stays");

    // The toolbar's focus button and the strip's exit button.
    harness.click_at(harness.get_by_label(ENTER_HINT).rect());
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [false; 4], "the toolbar button enters focus mode");
    assert_eq!(top_line(&harness, Surface::Files), in_focus);
    harness.click_at(harness.get_by_label(EXIT_HINT).rect());
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [true; 4], "the strip's button leaves it");

    // The Ctrl+K search offers the command; Ctrl+K in focus mode brings the search back.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::K);
    harness.event(egui::Event::Text("focus".to_owned()));
    harness.run_steps(3);
    assert!(walkthrough::shows(&harness, "Toggle focus mode"), "the search offers the command");
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [false; 4], "the command enters focus mode");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::K);
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [true; 4], "Ctrl+K leaves focus mode");
    assert!(harness.ctx.text_edit_focused(), "with the search box taking the keys");
    harness.key_press(Key::Escape);
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [true; 4], "Esc closes the search, nothing else");
}

#[test]
#[expect(clippy::too_many_lines, reason = "one walkthrough scenario, told step by step")]
fn focus_mode_gives_a_log_the_window_and_keeps_its_place() {
    let (_scratch, _host, mut harness) = app("focus-log");
    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Pipelines").rect());
    wait_until(&mut harness, "the job's row", |harness| {
        harness
            .query_by(|node| {
                node.role() == Role::Row && node.label().is_some_and(|label| label.starts_with(JOB))
            })
            .is_some()
    });
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Row && node.label().is_some_and(|label| label.starts_with(JOB))
            })
            .rect(),
    );
    wait_until(&mut harness, "the log", |harness| walkthrough::shows(harness, "Copy log"));
    // A search scrolls well into the log.
    let search = harness
        .query_all_by_role(Role::TextInput)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("the log's search box");
    harness.click_at(search.rect());
    harness.event(egui::Event::Text("step 1800:".to_owned()));
    harness.key_press(Key::Enter);
    wait_until(&mut harness, "the match", |harness| walkthrough::shows(harness, "1 / 1"));
    harness.run_steps(5);
    // A rerun the host refuses: its failure shows above the log with its ways out.
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Button
                    && node.label().is_some_and(|label| label.starts_with("Re-run"))
            })
            .rect(),
    );
    wait_until(&mut harness, "the refused rerun", |harness| {
        harness.query_by_label_contains(RERUN_REFUSED).is_some()
    });
    harness.run_steps(5);
    let before = top_line(&harness, Surface::Log);
    assert!(before.0 > 1000, "the log scrolled to the match: {before:?}");
    assert!(
        harness.query_by_label_contains(SWITCH_ATTEMPT).is_some(),
        "the job failed in two attempts"
    );

    // F11 while the search box has the keys: the log takes the window, the search stays.
    toggle(&mut harness);
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [false; 4], "title bar, rail, tabs and header leave");
    assert!(
        harness.query_by_label("Stages").is_none(),
        "the run header and the job list leave too"
    );
    assert_eq!(top_line(&harness, Surface::Log), before, "the top line stays exactly in place");
    assert_strip(&harness, 1440.0, EXIT_HINT);
    let field = harness.get_by_role(Role::TextInput).rect();
    assert!(
        rect_of(&harness, Role::Toolbar, strip::LABEL).contains_rect(field),
        "the search box fits the strip: {field:?}"
    );
    assert!(
        walkthrough::shows(&harness, "1 / 1") && walkthrough::shows(&harness, COPY_LOG),
        "search and copy moved into the strip"
    );
    assert!(
        walkthrough::shows(&harness, JOB) && walkthrough::shows(&harness, "Failed"),
        "the strip names the job and its state"
    );
    let bar = rect_of(&harness, Role::Toolbar, strip::LABEL);
    let switcher = harness.get_by_label_contains(SWITCH_ATTEMPT).rect();
    assert!(bar.contains_rect(switcher), "the attempt switcher moved into the strip: {switcher:?}");
    // The refused rerun stays, all its ways out with it, floating under the strip's right end.
    let failure = harness.get_by_label_contains(RERUN_REFUSED).rect();
    let retry = rect_of(&harness, Role::Button, "Retry");
    let dismiss = rect_of(&harness, Role::Button, "Dismiss");
    assert!(
        failure.top() >= bar.bottom() && retry.top() >= bar.bottom(),
        "the failure sits under the strip: {failure:?}"
    );
    assert!(dismiss.right() <= 1440.0 && failure.left() > 720.0, "at the right end: {failure:?}");
    assert_stable(&mut harness, &[EXIT_HINT, NEXT_MATCH, COPY_LOG, "Close", SWITCH_ATTEMPT]);
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Dismiss").rect());
    harness.run_steps(3);
    assert!(harness.query_by_label_contains(RERUN_REFUSED).is_none(), "Dismiss closes it");
    assert_eq!(top_line(&harness, Surface::Log), before, "the log did not move under it");
    harness.click_at(harness.get_by_role(Role::TextInput).rect());
    harness.run_steps(2);

    // The first Esc only takes the keys from the search box; the next one leaves focus mode.
    harness.key_press(Key::Escape);
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [false; 4], "Esc in the search box stays in focus mode");
    harness.key_press(Key::Escape);
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [true; 4], "the next Esc gives the window back");
    assert!(walkthrough::shows(&harness, "Stages"), "the log page is back as it was");
    assert_eq!(top_line(&harness, Surface::Log), before, "and the top line stays");

    // The log toolbar's focus button.
    harness.click_at(harness.get_by_label(ENTER_HINT).rect());
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [false; 4]);
    assert_eq!(top_line(&harness, Surface::Log), before);
    harness.click_at(harness.get_by_label(EXIT_HINT).rect());
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [true; 4]);
}

#[test]
fn focus_mode_folds_the_original_versions_banner_into_its_strip() {
    let scratch = ScratchDir::new("focus-origin");
    let mut host = GitHubHost::with_pull_request(scratch.path());
    let documented = (1..=60).fold(String::new(), |mut text, line| {
        writeln!(text, "/// Line {line} of the greeting's documentation.").unwrap();
        text
    });
    let replaced = host.force_push_outdating_the_thread(
        &format!("{documented}pub fn greeting() -> &'static str {{\n    \"Hello, world\"\n}}\n"),
        ReplacedHead::Kept,
    );
    let banner = format!("Showing the version from {}", &replaced[..7]);
    let mut harness = walkthrough::app(&scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().title_bar.integrate_for_test();
    open_pull_request(&mut harness);
    walkthrough::files_tab(&mut harness);
    walkthrough::open_in_tree(&mut harness, FILE);
    open_origin(&mut harness, &banner);

    // F11: the banner's words, Back to latest and Open on GitHub move into the strip.
    toggle(&mut harness);
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [false; 4], "title bar, rail, tabs and header leave");
    assert_strip(&harness, 1440.0, EXIT_HINT);
    let bar = rect_of(&harness, Role::Toolbar, strip::LABEL);
    let said: Vec<Rect> =
        harness.query_all_by_label_contains(&banner).map(|node| node.rect()).collect();
    assert!(
        said.len() == 1 && bar.contains_rect(said[0]),
        "the strip says what the banner said, once: {said:?}"
    );
    assert!(
        bar.contains_rect(rect_of(&harness, Role::Button, BACK_LABEL)),
        "Back to latest is in the strip"
    );
    assert!(
        bar.contains_rect(rect_of(&harness, Role::Button, "Open on GitHub")),
        "and the comment on GitHub"
    );
    assert_stable(&mut harness, &[EXIT_HINT, BACK_LABEL, "Close"]);

    // Esc goes back to the latest version first; focus mode stays.
    harness.key_press(Key::Escape);
    harness.run_steps(3);
    assert!(harness.query_by_label_contains(&banner).is_none(), "Esc left the original version");
    assert_eq!(chrome_visible(&harness), [false; 4], "and stayed in focus mode");
    assert!(walkthrough::shows(&harness, NEXT_CHANGE_HINT), "the file's own strip is back");

    // Back to latest in the strip does the same.
    open_origin(&mut harness, &banner);
    assert_eq!(chrome_visible(&harness), [false; 4]);
    harness.click_at(harness.get_by_role_and_label(Role::Button, BACK_LABEL).rect());
    harness.run_steps(3);
    assert!(
        harness.query_by_label_contains(&banner).is_none(),
        "Back to latest left the original version"
    );
    assert_eq!(chrome_visible(&harness), [false; 4], "in focus mode");

    // The next Esc leaves focus mode.
    harness.key_press(Key::Escape);
    harness.run_steps(3);
    assert_eq!(chrome_visible(&harness), [true; 4], "the second Esc gives the window back");
}

/// Opens the outdated thread's original version from the latest diff.
fn open_origin(harness: &mut Harness<'_, ReviewApp>, banner: &str) {
    const DETACHED: &str = "1 comment on lines not shown in this diff";
    wait_until(harness, "the outdated thread", |harness| {
        harness.query_by_label(SHOW_ORIGIN).is_some()
            || harness.query_by_label_contains(DETACHED).is_some()
    });
    if harness.query_by_label(SHOW_ORIGIN).is_none() {
        harness.click_at(harness.get_by_label_contains(DETACHED).rect());
        wait_until(harness, "the outdated card", |harness| {
            harness.query_by_label(SHOW_ORIGIN).is_some()
        });
    }
    harness.click_at(harness.get_by_label(SHOW_ORIGIN).rect());
    wait_until(harness, "the original version", |harness| {
        harness.query_by_label_contains(banner).is_some()
    });
    harness.run_steps(5);
}
