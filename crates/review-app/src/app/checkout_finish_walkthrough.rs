//! A checkout's worktree once its pull request is merged elsewhere, end to end: the mocked GitHub
//! host ([`super::github_mock`]) answers the pull request merged, a reload (F5) or the next app
//! start sees it, and Settings › Local checkouts decides. By default a clean worktree goes
//! silently; one with files of the user's stays and the page offers the removal with the reason;
//! "Ask me first" offers, "Keep worktree" does nothing, and a merge from Code View always asks.
//! Needs `git` on PATH, no network.
//! `cargo test -p review-app checkout_finish`

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_store::AppSettings;
use review_store::checkouts::WhenFinished;

use super::ReviewApp;
use super::checkout_walkthrough::{
    Setup, click_last, open_form, shows, start, tap, wait_ready, worktree_of,
};
use super::walkthrough::{app, wait_until};
use cv_runtime::route::Route;
use cv_testkit::Clicks;

const REMOVE_LINK: &str = "Remove worktree…";

/// A done checkout in a new worktree, with `mode` for merges elsewhere.
fn checked_out(name: &str, mode: WhenFinished) -> Setup {
    let mut setup = start(name);
    setup.harness.state_mut().services.settings.checkouts.when_finished = mode;
    open_form(&mut setup.harness);
    click_last(&mut setup.harness, "Check out");
    wait_ready(&mut setup.harness);
    setup
}

/// The host has the pull request merged; F5 shows it on the page.
fn merged_elsewhere(setup: &mut Setup) {
    setup.host.answer_merged();
    setup.harness.key_press(egui::Key::F5);
    wait_until(&mut setup.harness, "the merged pull request", |harness| shows(harness, "Merged"));
}

fn has_record(harness: &Harness<'_, ReviewApp>) -> bool {
    !harness.state().services.checkouts.records().all().is_empty()
}

fn silent(harness: &Harness<'_, ReviewApp>) -> bool {
    harness.state().services.toasts.texts().is_empty()
}

#[test]
fn checkout_finish_removes_a_clean_worktree_silently_by_default() {
    let mut setup = checked_out("finish-auto", WhenFinished::RemoveAutomatically);
    let worktree = worktree_of(&setup.clone);
    merged_elsewhere(&mut setup);
    let harness = &mut setup.harness;
    wait_until(harness, "the removed worktree", |harness| !has_record(harness));
    assert!(!worktree.exists(), "the worktree went");
    assert!(silent(harness), "no toast: {:?}", harness.state().services.toasts.texts());
    assert!(shows(harness, "Check out"), "the page offers a new checkout");
}

#[test]
fn checkout_finish_offers_the_removal_in_ask_mode_and_does_nothing_in_keep_mode() {
    let mut ask = checked_out("finish-ask", WhenFinished::Ask);
    merged_elsewhere(&mut ask);
    wait_until(&mut ask.harness, "the offer", |harness| shows(harness, REMOVE_LINK));
    assert!(worktree_of(&ask.clone).exists() && has_record(&ask.harness), "nothing removed");
    tap(&mut ask.harness, REMOVE_LINK);
    assert!(shows(&ask.harness, "Remove this worktree?"), "the link asks first");

    let mut keep = checked_out("finish-keep", WhenFinished::Keep);
    merged_elsewhere(&mut keep);
    keep.harness.run_steps(20);
    assert!(!shows(&keep.harness, REMOVE_LINK), "keep mode offers nothing");
    assert!(worktree_of(&keep.clone).exists() && has_record(&keep.harness), "nothing removed");
}

#[test]
fn checkout_finish_keeps_a_worktree_with_untracked_files_and_says_why() {
    let mut setup = checked_out("finish-dirty", WhenFinished::RemoveAutomatically);
    let worktree = worktree_of(&setup.clone);
    std::fs::write(worktree.join("notes.txt"), "mine\n").expect("untracked file");
    merged_elsewhere(&mut setup);
    let harness = &mut setup.harness;
    wait_until(harness, "the fallback offer", |harness| shows(harness, REMOVE_LINK));
    assert!(worktree.join("notes.txt").exists(), "the user's file stays");
    let log = harness.state().services.checkouts.records().all()[0].log.join("\n");
    assert!(log.contains("worktree kept: 1 uncommitted file in the worktree"), "{log}");
    assert!(silent(harness), "no toast");
    tap(harness, REMOVE_LINK);
    assert!(shows(harness, "It stayed when the pull request was done: 1 uncommitted file"));
}

#[test]
fn checkout_finish_asks_after_a_merge_from_code_view() {
    let mut setup = checked_out("finish-here", WhenFinished::RemoveAutomatically);
    setup.host.accept_merge();
    tap(&mut setup.harness, "Merge commit");
    tap(&mut setup.harness, "Click again to merge");
    wait_until(&mut setup.harness, "the merge", |harness| {
        harness.state().services.toasts.texts().contains(&"Pull request merged.")
    });
    assert!(setup.harness.state().services.checkouts.records().all()[0].merged_here);
    merged_elsewhere(&mut setup);
    wait_until(&mut setup.harness, "the offer", |harness| shows(harness, REMOVE_LINK));
    assert!(worktree_of(&setup.clone).exists(), "a merge from Code View removes nothing");
}

#[test]
fn checkout_finish_applies_at_the_next_start() {
    let Setup { scratch, host, harness, clone } =
        checked_out("finish-start", WhenFinished::RemoveAutomatically);
    drop(harness);
    host.answer_merged();
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    let worktree = worktree_of(&clone);
    wait_until(&mut harness, "the removal at start", |_| !worktree.exists());
    wait_until(&mut harness, "the dropped record", |harness| !has_record(harness));
}

#[test]
fn checkout_finish_setting_is_a_select_with_its_help_and_saves() {
    let Setup { scratch: _scratch, host: _host, mut harness, clone: _clone } =
        start("finish-select");
    harness.state_mut().navigator.go(Route::Settings);
    wait_until(&mut harness, "Settings", |harness| harness.state().route == Route::Settings);
    harness.click_at(
        harness.query_all_by_label("Local checkouts").next().expect("the section").rect(),
    );
    let label = "When a pull request is merged or closed elsewhere";
    wait_until(&mut harness, "the row", |harness| shows(harness, label));
    assert!(shows(
        &harness,
        "Applies when a merge or close is detected on the server, not when you merge from Code View. \
         Worktrees with uncommitted changes are never removed automatically."
    ));
    let select = || {
        harness
            .query_all_by_role(Role::ComboBox)
            .find(|node| node.value().is_some_and(|value| value == "Remove worktree automatically"))
    };
    let select = select().expect("the select shows the default");
    harness.click_at(select.rect());
    harness.run_steps(3);
    assert!(shows(&harness, "Ask me first"), "the select offers its three choices");
    click_last(&mut harness, "Keep worktree");
    harness.run_steps(3);
    assert_eq!(harness.state().services.settings.checkouts.when_finished, WhenFinished::Keep);
    let saved =
        AppSettings::load(&harness.state().services.paths.settings_file()).expect("saved settings");
    assert_eq!(saved.checkouts.when_finished, WhenFinished::Keep);
}
