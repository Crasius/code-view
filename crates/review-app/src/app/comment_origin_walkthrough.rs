//! "Show original version" end to end: the real GitHub provider against the mocked host, whose
//! pull request was force pushed after its review thread was written. The outdated thread offers
//! the version it was written on, in the overview and in the diff; the app fetches the replaced
//! head by id, shows the file as it was with the comment at its line, and "Back to latest" (or
//! Esc) returns to the latest diff where it was left. When the host collected the replaced commit,
//! the comment's own lines stand in. No network, no real token.

use std::fmt::Write as _;

use egui::{Rect, Role, TouchPhase};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::github_mock::{CURRENT_TEXT, FILE, GitHubHost, THREAD_TEXT, open_pull_request};
use super::walkthrough::{app, files_tab, shows_file_view, wait_until, wheel};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const SHOW_ORIGIN: &str = "Show original version";
const BACK_LABEL: &str = "Back to latest";
/// The gutter marker of the outdated thread by cy, in the version it was written on.
const MARKER: &str = "Thread by cy, click to collapse it";
/// The latest diff keeps outdated threads above the code, collapsed.
const DETACHED: &str = "1 comment on lines not shown in this diff";

/// The rewritten head: the renamed greeting under a long doc comment, so line 1 is a new line now
/// and the diff is long enough to scroll.
fn rewritten_file() -> String {
    let mut text = (1..=40).fold(String::new(), |mut text, line| {
        writeln!(text, "/// Line {line} of the greeting's documentation.").unwrap();
        text
    });
    text.push_str("pub fn greeting() -> &'static str {\n    \"Hello, world\"\n}\n\npub fn farewell() -> &'static str {\n    \"Bye\"\n}\n");
    for value in 1..=40 {
        writeln!(text, "pub const WIDGET_{value}: u32 = {value};").unwrap();
    }
    text
}

#[test]
fn comment_origin_shows_the_version_an_outdated_thread_was_written_on() {
    let scratch = ScratchDir::new("comment-origin");
    let mut host = GitHubHost::with_pull_request(scratch.path());
    let replaced = host.force_push_outdating_the_thread(&rewritten_file(), true);
    let banner = format!("Showing the version from {}", &replaced[..7]);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    // The overview's activity: the card says it is outdated and offers the original version.
    wait_until(&mut harness, "the outdated thread", |harness| {
        harness.query_by_label(SHOW_ORIGIN).is_some()
    });
    assert!(harness.query_by_label("Outdated").is_some(), "the card is marked outdated");
    harness.get_by_label(SHOW_ORIGIN).scroll_to_me();
    settle(&mut harness);
    harness.click_at(harness.get_by_label(SHOW_ORIGIN).rect());
    wait_until(&mut harness, "the original version from the overview", |harness| {
        harness.query_by_label_contains(&banner).is_some()
    });
    assert!(shows_file_view(&harness, FILE), "the Files tab shows it");
    harness.key_press(egui::Key::Escape);
    settle(&mut harness);
    assert!(harness.query_by_label_contains(&banner).is_none(), "Esc goes back");

    // The latest diff: the outdated thread waits above the code with its pill and the action.
    open_latest_diff(&mut harness);
    settle(&mut harness);
    scroll(&mut harness, 300.0);
    let latest_card = rect_of(&harness, CURRENT_TEXT);
    let latest_diff = rect_of_diff(&harness);
    assert!(
        latest_diff.contains_rect(latest_card),
        "mona's card is on screen after scrolling: {latest_card:?} in {latest_diff:?}"
    );

    // The original version: the replaced head, fetched by id, with the comment at its line 1.
    harness.click_at(harness.get_by_label(SHOW_ORIGIN).rect());
    wait_until(&mut harness, "the original version", |harness| {
        harness.query_by_label_contains(&banner).is_some()
            && harness.query_by_label(MARKER).is_some()
    });
    assert!(
        harness.query_by_label("Outdated").is_none(),
        "in its own version the thread is current"
    );
    assert!(harness.query_by_label(SHOW_ORIGIN).is_none());
    assert!(
        harness.query_by_label(CURRENT_TEXT).is_none(),
        "the latest version's threads stay with it"
    );
    assert!(harness.query_by_role_and_label(Role::Button, BACK_LABEL).is_some());
    assert!(
        harness.query_by_label("Open on GitHub").is_some(),
        "the comment's page is one click away"
    );
    let origin_diff = rect_of_diff(&harness);
    let marker = rect_of(&harness, MARKER);
    let comment = rect_of(&harness, THREAD_TEXT);
    println!("origin diff {origin_diff:?}, marker {marker:?}, comment {comment:?}");
    // Line 1 is the diff's first row: the marker sits in it, the card right below it.
    let row = marker.center().y - origin_diff.top();
    assert!(row > 0.0 && row < 16.0, "the marker is on the first row: {row} below the diff's top");
    assert!(comment.top() > marker.bottom(), "the card follows its line");

    // Nothing moves while the version stays on screen.
    let stable = |harness: &Harness<'_, ReviewApp>| {
        [
            rect_of(harness, &banner),
            rect_of(harness, MARKER),
            rect_of(harness, THREAD_TEXT),
            rect_of(harness, BACK_LABEL),
        ]
    };
    let before = stable(&harness);
    harness.run_steps(30);
    assert_eq!(stable(&harness), before, "the banner, the marker and the card stay put");

    // Back to latest: the latest diff where it was left, scrolled as before.
    harness.click_at(harness.get_by_role_and_label(Role::Button, BACK_LABEL).rect());
    wait_until(&mut harness, "the latest diff", |harness| {
        harness.query_by_label_contains(&banner).is_none()
            && harness.query_by_label(CURRENT_TEXT).is_some()
    });
    assert_eq!(
        rect_of(&harness, CURRENT_TEXT),
        latest_card,
        "the latest diff keeps its scroll position"
    );
}

#[test]
fn comment_origin_falls_back_to_the_comments_lines_when_the_commit_is_gone() {
    let scratch = ScratchDir::new("comment-origin-gone");
    let mut host = GitHubHost::with_pull_request(scratch.path());
    let replaced = host.force_push_outdating_the_thread(&rewritten_file(), false);
    let banner = format!(
        "The version from {} is not available: showing the lines the comment was written on",
        &replaced[..7]
    );
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    open_latest_diff(&mut harness);

    harness.click_at(harness.get_by_label(SHOW_ORIGIN).rect());
    wait_until(&mut harness, "the comment's own lines", |harness| {
        harness.query_by_label(&banner).is_some() && harness.query_by_label(MARKER).is_some()
    });
    assert!(
        harness.query_by_label(THREAD_TEXT).is_some(),
        "the thread sits on the hunk GitHub kept with it"
    );
    for label in ["Retry", "Open on GitHub", BACK_LABEL] {
        assert!(harness.query_by_label(label).is_some(), "{label} is offered");
    }
    harness.click_at(harness.get_by_role_and_label(Role::Button, BACK_LABEL).rect());
    wait_until(&mut harness, "the latest diff", |harness| {
        harness.query_by_label(&banner).is_none() && harness.query_by_label(SHOW_ORIGIN).is_some()
    });
}

/// Opens [`FILE`] in the Files tab and the section with its outdated thread.
fn open_latest_diff(harness: &mut Harness<'_, ReviewApp>) {
    files_tab(harness);
    wait_until(harness, "the file tree", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, FILE).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, FILE).rect());
    wait_until(harness, "the latest diff", |harness| {
        shows_file_view(harness, FILE) && harness.query_by_label_contains(DETACHED).is_some()
    });
    harness.click_at(harness.get_by_label_contains(DETACHED).rect());
    wait_until(harness, "the outdated card", |harness| {
        harness.query_by_label(SHOW_ORIGIN).is_some()
    });
    assert!(harness.query_by_label("Outdated").is_some(), "the card is marked outdated");
}

/// Wheel-scrolls the diff down by `points` and lets the smooth scroll arrive.
fn scroll(harness: &mut Harness<'_, ReviewApp>, points: f32) {
    let mouse = |delta: f32, phase| wheel(delta, phase, egui::MouseWheelSource::Wheel);
    harness.hover_at(rect_of_diff(harness).center());
    harness.event(mouse(0.0, TouchPhase::Start));
    harness.event(mouse(-points, TouchPhase::Move));
    harness.event(mouse(0.0, TouchPhase::End));
    for _ in 0..60 {
        harness.step();
    }
}

fn rect_of(harness: &Harness<'_, ReviewApp>, label: &str) -> Rect {
    harness.get_by_label_contains(label).rect()
}

fn rect_of_diff(harness: &Harness<'_, ReviewApp>) -> Rect {
    harness.get_by_label(&format!("Diff of {FILE}")).rect()
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..20 {
        harness.step();
    }
}
