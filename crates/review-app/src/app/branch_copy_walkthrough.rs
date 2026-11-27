//! Copying branch names and pull request links, offline against the mocked GitHub Enterprise host
//! ([`super::github_mock`]), whose pull request merges `feature` into `main`.
//!
//! Checks the list row's context menu ("Copy source branch", the list stays), then the header's
//! branch chips: a click on the source chip copies it and confirms with a check mark that goes
//! away again, and Enter on the focused target chip copies the target. Reads the copies from
//! egui's output, never the real clipboard.
//! `cargo test -p review-app branch_copy`
//!
//! The second walkthrough does the same for the link: the row menu's "Copy link", a click on the
//! header's number and Enter on it, with the check mark that replaces its prefix for a moment.
//! `cargo test -p review-app number_copy`

use egui::{OutputCommand, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::github_mock::{GitHubHost, NUMBER, REPOSITORY, TITLE, open_pull_request};
use super::walkthrough::{app, secondary_press, wait_until};
use cv_runtime::route::Route;
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const SOURCE_CHIP: &str = "Copy branch name: feature";
const TARGET_CHIP: &str = "Copy branch name: main";

#[test]
fn branch_copy_from_the_row_menu_and_the_header_chips() {
    let scratch = ScratchDir::new("branch-copy");
    let host = GitHubHost::with_pull_request(scratch.path());
    let connection = host.connection("gh", "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));

    // The row's menu copies the source branch; the row does not open.
    let row = format!("{NUMBER} {TITLE}");
    wait_until(&mut harness, "the pull request row", |harness| {
        harness.query_by_label(&row).is_some()
    });
    secondary_press(&mut harness, &row);
    wait_until(&mut harness, "the menu entry", |harness| {
        harness.query_by_role_and_label(Role::MenuItem, "Copy source branch").is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::MenuItem, "Copy source branch").rect());
    assert_eq!(copied(&mut harness).as_deref(), Some("feature"), "the menu copies the source");
    assert_eq!(harness.state().route, Route::MyPullRequests, "the list stays");

    // A click on the source chip copies it and confirms in place.
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the branch chips", |harness| {
        harness.query_by_role_and_label(Role::Button, SOURCE_CHIP).is_some()
            && harness.query_by_role_and_label(Role::Button, TARGET_CHIP).is_some()
    });
    let source = harness.get_by_role_and_label(Role::Button, SOURCE_CHIP).rect();
    harness.get_by_role_and_label(Role::Button, SOURCE_CHIP).hover();
    harness.run_steps(10);
    assert!(harness.query_by_label("Copy branch name").is_some(), "the tooltip");
    harness.click_at(harness.get_by_role_and_label(Role::Button, SOURCE_CHIP).rect());
    assert_eq!(copied(&mut harness).as_deref(), Some("feature"), "the chip copies the source");
    let confirmed = harness.get_by_role_and_label(Role::Button, "Copied feature").rect();
    assert_eq!(confirmed, source, "the confirmation keeps the chip's place and size");
    // The check mark goes away after a moment (harness frames are 1/60 s).
    harness.run_steps(120);
    assert!(harness.query_by_role_and_label(Role::Button, SOURCE_CHIP).is_some(), "back");

    // Enter on the focused target chip copies the target.
    harness.get_by_role_and_label(Role::Button, TARGET_CHIP).focus();
    harness.step();
    // The copy happens on the key's press; a whole key press would report the release frame's
    // output only.
    harness.key_down(egui::Key::Enter);
    assert_eq!(copied(&mut harness).as_deref(), Some("main"), "Enter copies the target");
    harness.key_up(egui::Key::Enter);
    harness.step();
    assert!(harness.query_by_role_and_label(Role::Button, "Copied main").is_some());
}

const NUMBER_LINK: &str = "Copy link to #7";

#[test]
fn number_copy_from_the_row_menu_and_the_header_number() {
    let scratch = ScratchDir::new("number-copy");
    let host = GitHubHost::with_pull_request(scratch.path());
    let connection = host.connection("gh", "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    let url_tail = format!("/{REPOSITORY}/pull/{NUMBER}");

    // The row's menu copies the link; the row does not open.
    let row = format!("{NUMBER} {TITLE}");
    wait_until(&mut harness, "the pull request row", |harness| {
        harness.query_by_label(&row).is_some()
    });
    secondary_press(&mut harness, &row);
    wait_until(&mut harness, "the menu entry", |harness| {
        harness.query_by_role_and_label(Role::MenuItem, "Copy link").is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::MenuItem, "Copy link").rect());
    let link = copied(&mut harness).expect("the menu copies the link");
    assert!(link.ends_with(&url_tail), "the row menu copies the web address: {link}");
    assert_eq!(harness.state().route, Route::MyPullRequests, "the list stays");

    // A click on the number copies the same address and confirms in place.
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the number", |harness| {
        harness.query_by_role_and_label(Role::Button, NUMBER_LINK).is_some()
    });
    let number = harness.get_by_role_and_label(Role::Button, NUMBER_LINK).rect();
    harness.get_by_role_and_label(Role::Button, NUMBER_LINK).hover();
    harness.run_steps(10);
    assert!(harness.query_by_label("Copy link to pull request").is_some(), "the tooltip");
    harness.click_at(harness.get_by_role_and_label(Role::Button, NUMBER_LINK).rect());
    // The list and the detail come from different mock payloads; both end in the same path.
    let copy = copied(&mut harness).expect("the number copies the link");
    assert!(copy.ends_with(&url_tail), "the number copies the web address: {copy}");
    let confirmed = harness.get_by_role_and_label(Role::Button, "Copied link to #7").rect();
    assert_eq!(confirmed, number, "the confirmation keeps the number's place and size");
    harness.run_steps(120);
    assert!(harness.query_by_role_and_label(Role::Button, NUMBER_LINK).is_some(), "back");

    // Enter on the focused number copies too.
    harness.get_by_role_and_label(Role::Button, NUMBER_LINK).focus();
    harness.step();
    harness.key_down(egui::Key::Enter);
    assert_eq!(copied(&mut harness).as_deref(), Some(copy.as_str()), "Enter copies the link");
    harness.key_up(egui::Key::Enter);
    harness.step();
    assert!(harness.query_by_role_and_label(Role::Button, "Copied link to #7").is_some());
}

/// The text the app put on the clipboard in the next few frames, if any.
fn copied(harness: &mut Harness<'_, ReviewApp>) -> Option<String> {
    (0..4).find_map(|_| {
        harness.step();
        harness.output().platform_output.commands.iter().find_map(|command| match command {
            OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
    })
}
