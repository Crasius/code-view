//! Live updates that meet a comment being written: a push to the commented file waits behind a
//! Reload card, a push to another file goes on screen at once, and a held update follows by
//! itself once the comment is sent.

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::super::ReviewApp;
use super::super::github_mock::{ExtraFile, GitHubHost, open_diff, open_pull_request};
use super::super::walkthrough::{app, has, hover_until_add_comment, press, type_into, wait_until};
use super::{commits_tab, has_draft, reload_button};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

/// Changed besides the file the pushes of these tests change.
const OTHER_FILE: &str = "app/other.rs";
const DRAFT: &str = "Half a comment";

#[test]
fn live_push_to_the_file_of_a_comment_draft_waits_for_reload() {
    let scratch = ScratchDir::new("live-held-push");
    let mut host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    let diff = open_diff(&mut harness);
    write_comment(&mut harness, diff);

    host.push_lines_on_top("/// The greeting every caller shows.\n");
    wait_until(&mut harness, "the reload card", |harness| reload_button(harness).is_some());
    assert!(has(&harness, "1 new commit by Test"), "the card says what came in");
    assert!(commits_tab(&harness, 1), "nothing moves under the draft");

    harness.click_at(harness.get_by_role_and_label(Role::Button, "Reload").rect());
    wait_until(&mut harness, "the pushed commit", |harness| commits_tab(harness, 2));
    assert!(has_draft(&harness, DRAFT), "the draft stays");
    press(&mut harness, "Show changes");
    wait_until(&mut harness, "the pushed range", |harness| {
        has(harness, "Showing what was pushed since you last looked")
    });
    press(&mut harness, "Show all changes");
    wait_until(&mut harness, "all changes again", |harness| {
        !has(harness, "Showing what was pushed since you last looked")
    });
}

#[test]
fn live_push_to_another_file_applies_beside_a_comment_draft() {
    let scratch = ScratchDir::new("live-other-file");
    let other = ExtraFile {
        path: OTHER_FILE,
        base: b"pub fn other() {}\n".to_vec(),
        head: b"pub fn other() -> u32 {\n    2\n}\n".to_vec(),
    };
    let mut host = GitHubHost::with_pull_request_files(scratch.path(), &[other]);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    open_diff(&mut harness);
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, OTHER_FILE).rect());
    let label = format!("Diff of {OTHER_FILE}");
    wait_until(&mut harness, "the other file's diff", |harness| {
        harness.query_by_label(&label).is_some()
    });
    let diff = harness.get_by_label(&label).rect();
    write_comment(&mut harness, diff);
    wait_until(&mut harness, "the commit count", |harness| commits_tab(harness, 1));

    host.push_lines_on_top("/// The greeting every caller shows.\n");
    wait_until(&mut harness, "the pushed commit", |harness| commits_tab(harness, 2));
    assert!(reload_button(&harness).is_none(), "the push does not touch the commented file");
    assert!(!has(&harness, "Updated: "), "an applied update shows no card");
    assert!(has_draft(&harness, DRAFT), "the draft stays where it is");
}

#[test]
fn live_held_update_applies_by_itself_once_the_comment_is_sent() {
    let scratch = ScratchDir::new("live-held-sent");
    let mut host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    let diff = open_diff(&mut harness);
    write_comment(&mut harness, diff);

    host.push_lines_on_top("/// The greeting every caller shows.\n");
    wait_until(&mut harness, "the reload card", |harness| reload_button(harness).is_some());
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Comment").rect());
    wait_until(&mut harness, "the pushed commit", |harness| commits_tab(harness, 2));
    assert!(reload_button(&harness).is_none(), "the card left with the draft");
    assert!(!has(&harness, "Updated: "), "an update applied by itself shows no card");
}

/// Opens a new comment on the first line of `diff` through the hover button and types [`DRAFT`].
fn write_comment(harness: &mut Harness<'_, ReviewApp>, diff: egui::Rect) {
    // Down the right (head) pane.
    hover_until_add_comment(harness, diff, &[0.75]);
    harness.click_at(harness.get_by_label("Add comment").rect());
    type_into(harness, "Leave a comment", DRAFT);
}
