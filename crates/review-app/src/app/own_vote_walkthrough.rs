//! The vote control on the user's own pull request, against the GitHub mock
//! ([`super::github_mock`]). GitHub refuses every review by the author with 422, so for the
//! author the header's Approve and the vote menu's entries stay in place but off, their tooltip
//! says why and that comments still work, and no review request goes out. For anyone else they
//! stay on.
//! `cargo test -p review-app --bin CodeView own_vote`

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::ReviewApp;
use super::action_e2e::{app, open_pull_request, press, wait_until};
use super::github_mock::{GitHubHost, NUMBER, REPOSITORY};
use cv_testkit::ScratchDir;

const APPROVE: &str = "Approve";
const REASON: &str = "GitHub does not let authors approve their own pull request";
const ENTRIES: [&str; 3] = ["Approve", "Approve with comment…", "Request changes…"];

#[test]
fn own_vote_author_on_github_cannot_vote_and_sees_why() {
    let scratch = ScratchDir::new("own-vote-author");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.sign_in_as_author();
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    wait_until(&mut harness, "Approve off for the author", |harness| {
        approve_buttons(harness).is_some_and(|enabled| enabled.iter().all(|is_on| !is_on))
    });
    harness.get_by_role_and_label(Role::Button, APPROVE).hover();
    wait_until(&mut harness, "the reason in the tooltip", |harness| {
        harness.query_all_by_label_contains(REASON).next().is_some()
    });
    assert!(
        harness.query_all_by_label_contains("Comments and replies still work").next().is_some(),
        "the tooltip says what still works"
    );
    harness.get_by_role_and_label(Role::Button, APPROVE).click();

    press(&mut harness, "More votes");
    wait_until(&mut harness, "the vote menu", |harness| menu_entries(harness).is_some());
    let entries = menu_entries(&harness).expect("the vote menu");
    assert!(entries.iter().all(|is_on| !is_on), "both votes are off in the menu: {entries:?}");
    // A click in the menu closes it, so each entry gets a menu of its own.
    for label in ENTRIES {
        if harness.query_by_role_and_label(Role::MenuItem, label).is_none() {
            press(&mut harness, "More votes");
            wait_until(&mut harness, "the vote menu", |harness| menu_entries(harness).is_some());
        }
        harness.get_by_role_and_label(Role::MenuItem, label).click();
        harness.run_steps(2);
    }
    harness.run_steps(20);
    assert!(
        harness.query_by_role_and_label(Role::Button, "Request changes").is_none(),
        "no composer opens"
    );
    let reviews = format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/reviews");
    assert!(host.find("POST", &reviews).is_none(), "no review request goes out");
}

#[test]
fn own_vote_reviewer_on_github_keeps_both_votes() {
    let scratch = ScratchDir::new("own-vote-reviewer");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.sign_in_as_me();
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    wait_until(&mut harness, "who the user is", |_| host.find("GET", "/api/v3/user").is_some());
    harness.run_steps(20);
    wait_until(&mut harness, "Approve on for a reviewer", |harness| {
        approve_buttons(harness).is_some_and(|enabled| enabled.iter().all(|is_on| *is_on))
    });
    press(&mut harness, "More votes");
    wait_until(&mut harness, "the vote menu", |harness| menu_entries(harness).is_some());
    let entries = menu_entries(&harness).expect("the vote menu");
    assert!(entries.iter().all(|is_on| *is_on), "both votes are on in the menu: {entries:?}");
}

/// Whether each "Approve" button is enabled, once at least one shows.
fn approve_buttons(harness: &Harness<'_, ReviewApp>) -> Option<Vec<bool>> {
    let enabled: Vec<bool> = harness
        .query_all_by_role_and_label(Role::Button, APPROVE)
        .map(|node| !node.accesskit_node().is_disabled())
        .collect();
    (!enabled.is_empty()).then_some(enabled)
}

/// Whether each vote menu entry is enabled, once all of them show.
fn menu_entries(harness: &Harness<'_, ReviewApp>) -> Option<Vec<bool>> {
    ENTRIES
        .iter()
        .map(|label| {
            harness
                .query_by_role_and_label(Role::MenuItem, label)
                .map(|node| !node.accesskit_node().is_disabled())
        })
        .collect()
}
