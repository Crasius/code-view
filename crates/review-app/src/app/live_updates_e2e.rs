//! Live updates end to end: the real app against the mocked GitHub host ([`super::github_mock`]),
//! which changes while the page is open. The harness clock runs a quarter second per frame, so the
//! 30-second probe comes due within a few hundred frames. Needs `git` on PATH, no network.

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::github_mock::{FILE, GitHubHost, THREAD_TEXT, open_diff, open_pull_request};
use super::walkthrough::{app, has, press, type_into, wait_until};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

mod held;

#[test]
fn live_push_while_viewing_applies_without_a_card_and_keeps_the_file() {
    let scratch = ScratchDir::new("live-push");
    let mut host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    open_diff(&mut harness);
    wait_until(&mut harness, "the commit count", |harness| commits_tab(harness, 1));

    host.push_lines_on_top("/// The greeting every caller shows.\n");
    wait_until(&mut harness, "the pushed commit", |harness| commits_tab(harness, 2));
    assert!(reload_button(&harness).is_none(), "nobody was writing, so nothing asks for a reload");
    assert!(!has(&harness, "1 new commit by Test"), "an applied update shows no card");
    assert!(!has(&harness, "Updated: "), "an applied update shows no card");
    let diff = format!("Diff of {FILE}");
    assert!(harness.query_by_label(&diff).is_some(), "the same file stays open");
    assert!(harness.query_by_label(THREAD_TEXT).is_some(), "the thread stays in the diff");
}

#[test]
fn live_reply_while_writing_waits_for_reload_and_keeps_the_draft() {
    let scratch = ScratchDir::new("live-reply");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the existing thread", |harness| {
        harness.query_by_label(THREAD_TEXT).is_some()
    });
    press(&mut harness, "Reply");
    type_into(&mut harness, "Reply", "Half an answer");

    host.publish_reply("dee", "Kept it as an alias.");
    wait_until(&mut harness, "the reload card", |harness| reload_button(harness).is_some());
    assert!(
        harness.query_by_label("Kept it as an alias.").is_none(),
        "nothing reloads underneath the draft"
    );
    assert!(has(&harness, "1 new comment by dee"), "the card says what came in");

    harness.click_at(harness.get_by_role_and_label(Role::Button, "Reload").rect());
    wait_until(&mut harness, "the new reply", |harness| {
        harness.query_by_label("Kept it as an alias.").is_some()
    });
    assert!(harness.query_by_label("New").is_some(), "the reply that came in live is marked");
    let draft = harness.query_by(|node| {
        matches!(node.role(), Role::TextInput | Role::MultilineTextInput)
            && node.value().as_deref() == Some("Half an answer")
    });
    assert!(draft.is_some(), "the reply box stays open with the draft");
}

/// The Commits tab counts `count` commits: the page shows that version of the pull request.
fn commits_tab(harness: &Harness<'_, ReviewApp>, count: usize) -> bool {
    harness.query_by_role_and_label(Role::Tab, &format!("Commits ({count})")).is_some()
}

fn reload_button<'h>(harness: &'h Harness<'_, ReviewApp>) -> Option<egui_kittest::Node<'h>> {
    harness.query_by_role_and_label(Role::Button, "Reload")
}
