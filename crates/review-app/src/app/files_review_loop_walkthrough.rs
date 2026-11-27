//! The Files tab's review loop end to end, against the GitHub mock ([`super::github_mock`]):
//! Shift+J skips viewed files; with "open the next unviewed one" on (the default) Alt+V moves on
//! by itself; Ctrl+P goes to a file by its name (↓ picks the next match, Enter opens it and hands
//! the keys back, so J works at once); N past the last change says so under the arrows and a
//! second N opens the next file at its first change (so a third N notes its one change is the
//! last); once every file is viewed the progress line says so and its "Approve" approves.
//! Needs `git` on PATH, no network.
//! `cargo test -p review-app --bin CodeView app::files_review_loop_walkthrough`

use egui::{Key, Modifiers, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_store::PullRequestTab;
use serde_json::json;

use super::ReviewApp;
use super::github_mock::{ExtraFile, FILE, GitHubHost, NUMBER, REPOSITORY, open_pull_request};
use super::walkthrough::{
    app, files_tab, open_in_tree, shows_file_view, wait_for_diff, wait_until,
};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const A: &str = "src/a.rs";
const B: &str = "src/b.rs";
const C: &str = "src/c.rs";
const EDGE_HINT: &str = "Last change. N again: next file";
const LAST_HINT: &str = "Last change of the last file";

fn file(path: &'static str, value: u32) -> ExtraFile {
    ExtraFile {
        path,
        base: b"pub fn value() -> u32 {\n    0\n}\n".to_vec(),
        head: format!("pub fn value() -> u32 {{\n    {value}\n}}\n").into_bytes(),
    }
}

#[test]
fn steps_through_the_review_to_the_vote() {
    let scratch = ScratchDir::new("files-review-loop");
    let host =
        GitHubHost::with_pull_request_files(scratch.path(), &[file(A, 1), file(B, 2), file(C, 3)]);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    assert!(harness.state().services.settings.open_next_unviewed, "on by default");
    open_pull_request(&mut harness);
    files_tab(&mut harness);
    open_in_tree(&mut harness, A);
    wait_for_diff(&mut harness, A);

    // Shift+J skips the viewed file.
    harness.click_at(harness.get_by_label(&format!("Viewed {B}")).rect());
    harness.run_steps(3);
    harness.key_press_modifiers(Modifiers::SHIFT, Key::J);
    wait_for_diff(&mut harness, C);

    // Alt+V marks it viewed and opens the next unviewed file.
    harness.key_press_modifiers(Modifiers::ALT, Key::V);
    wait_for_diff(&mut harness, FILE);

    // Ctrl+P, a name, ↓ to the second match, Enter: that file opens, the filter is as before and
    // the keys are the diff's again, so J opens the next file.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::P);
    wait_until(&mut harness, "the filter to take the keys", |harness| {
        harness.ctx.text_edit_focused()
    });
    filter(&harness).type_text(".rs");
    harness.run_steps(3);
    harness.key_press(Key::ArrowDown);
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    wait_for_diff(&mut harness, B);
    assert!(!harness.ctx.text_edit_focused(), "the keys go back to the diff");
    assert_eq!(filter(&harness).value().as_deref(), Some(""), "the search from before is back");
    harness.key_press(Key::J);
    wait_for_diff(&mut harness, C);

    // N to the file's one change, N again: the note; N once more: the next file at its change.
    harness.key_press(Key::N);
    harness.run_steps(3);
    assert!(harness.query_by_label(EDGE_HINT).is_none(), "the first step moves");
    harness.key_press(Key::N);
    wait_until(&mut harness, "the note under the arrows", |harness| {
        harness.query_by_label(EDGE_HINT).is_some()
    });
    assert!(shows_file_view(&harness, C), "the file stays open");
    harness.key_press(Key::N);
    wait_for_diff(&mut harness, FILE);
    harness.run_steps(3);
    assert!(harness.query_by_label(EDGE_HINT).is_none(), "the note is gone");
    // The file's one change is in focus already: the next N finds none further, and the last file
    // has none after it.
    harness.key_press(Key::N);
    wait_until(&mut harness, "the note at the last file's one change", |harness| {
        harness.query_by_label(LAST_HINT).is_some()
    });

    // The rest viewed: the progress line offers the vote, and Approve approves.
    harness.key_press_modifiers(Modifiers::ALT, Key::V);
    wait_for_diff(&mut harness, A);
    harness.key_press_modifiers(Modifiers::ALT, Key::V);
    wait_until(&mut harness, "the done line", |harness| {
        harness.query_by_label("All 4 files viewed").is_some()
    });
    assert!(harness.query_by_role_and_label(Role::Link, "Overview").is_some());
    harness.click_at(harness.get_by_role_and_label(Role::Link, "Approve").rect());
    let reviews = format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/reviews");
    wait_until(&mut harness, "the approval", |_| host.find("POST", &reviews).is_some());
    let body: serde_json::Value =
        host.find("POST", &reviews).expect("the review").body_json().expect("a JSON body");
    assert_eq!(body["event"], json!("APPROVE"));

    // Overview leads to the vote's other choices.
    harness.click_at(harness.get_by_role_and_label(Role::Link, "Overview").rect());
    wait_until(&mut harness, "the Overview", |harness| {
        harness.state().current_tab() == Some(PullRequestTab::Overview)
    });
}

/// The file tree's filter field.
fn filter<'a>(harness: &'a Harness<'_, ReviewApp>) -> egui_kittest::Node<'a> {
    harness.get_by(|node| node.placeholder().is_some_and(|hint| hint.ends_with("Filter files")))
}
