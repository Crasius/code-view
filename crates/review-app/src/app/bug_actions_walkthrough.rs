//! Review actions that used to end in a dead end, against the GitHub mock ([`super::github_mock`]):
//! "Request changes" needs a comment and posts it as the review's body; a refused merge keeps its
//! failure on the button and in a toast with Retry (which runs the merge once more), "Fix token"
//! and "Open on GitHub"; the user's own vote shows on the vote button, whose click then opens the
//! menu instead of approving again; a pull request that failed to load offers Retry and the
//! platform in the header, and F5 loads it.
//! `cargo test -p review-app --bin CodeView bug_actions`

use egui::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

use super::action_e2e::{app, open_pull_request, press, type_into, wait_until};
use super::github_mock::{GitHubHost, NUMBER, REPOSITORY, TITLE};
use cv_testkit::ScratchDir;

const REQUEST_CHANGES: &str = "Request changes";

#[test]
fn bug_actions_request_changes_needs_a_comment_and_posts_it() {
    let scratch = ScratchDir::new("bug-actions-request-changes");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    press(&mut harness, "More votes");
    press(&mut harness, "Request changes…");
    wait_until(&mut harness, "the composer", |harness| {
        harness.query_by_role_and_label(Role::Button, REQUEST_CHANGES).is_some()
    });
    assert!(
        harness.get_by_role_and_label(Role::Button, REQUEST_CHANGES).accesskit_node().is_disabled(),
        "an empty change request cannot be submitted"
    );
    assert!(
        harness.query_by_label("GitHub needs a comment to request changes").is_some(),
        "the footer says why"
    );

    type_into(&mut harness, "Say what should change", "Please keep the old name as an alias.");
    harness.get_by_role_and_label(Role::Button, REQUEST_CHANGES).click();
    let reviews = format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/reviews");
    wait_until(&mut harness, "the review", |_| host.find("POST", &reviews).is_some());
    let body: serde_json::Value =
        host.find("POST", &reviews).expect("the review").body_json().expect("a JSON body");
    assert_eq!(
        body,
        json!({ "event": "REQUEST_CHANGES", "body": "Please keep the old name as an alias." })
    );
    wait_until(&mut harness, "the composer to close", |harness| {
        harness.query_by_role_and_label(Role::Button, REQUEST_CHANGES).is_none()
    });
}

#[test]
fn bug_actions_failed_merge_keeps_its_way_out_and_retries_once() {
    let scratch = ScratchDir::new("bug-actions-merge");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.refuse_merge();
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    let strategies = ["Merge commit", "Squash and merge", "Rebase and merge"];
    wait_until(&mut harness, "the merge button", |harness| {
        strategies
            .iter()
            .any(|label| harness.query_by_role_and_label(Role::Button, label).is_some())
    });
    let label = strategies
        .iter()
        .find(|label| harness.query_by_role_and_label(Role::Button, label).is_some())
        .expect("a merge strategy");
    harness.get_by_role_and_label(Role::Button, label).click();
    harness.step();
    wait_until(&mut harness, "the armed button", |harness| {
        harness
            .query_by(|node| node.label().is_some_and(|text| text.starts_with("Click again")))
            .is_some()
    });
    harness.get_by(|node| node.label().is_some_and(|text| text.starts_with("Click again"))).click();

    let merges = |host: &GitHubHost| {
        let merge = format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/merge");
        host.requests()
            .iter()
            .filter(|request| request.method.as_str() == "PUT" && request.url.path() == merge)
            .count()
    };
    // The toast and the merge button both name the failure.
    wait_until(&mut harness, "the failure toast", |harness| {
        harness.query_all_by_label_contains("Merge failed: Resource not accessible").count() >= 2
            && harness.query_by_role_and_label(Role::Button, "Retry").is_some()
    });
    assert!(harness.query_all_by_label("Fix token").next().is_some(), "a 403 offers Fix token");
    assert!(
        harness.query_by_role_and_label(Role::Button, "Open on GitHub").is_some(),
        "the toast leads to the pull request on GitHub"
    );
    assert!(
        harness
            .query_by(|node| node.role() == Role::Button
                && node.label().is_some_and(|text| text.starts_with("Merge failed: ")))
            .is_some(),
        "the merge button keeps the failure"
    );
    assert_eq!(merges(&host), 1);

    harness.get_by_role_and_label(Role::Button, "Retry").click();
    wait_until(&mut harness, "the retried merge", |_| merges(&host) == 2);
    wait_until(&mut harness, "the second failure", |harness| {
        harness.query_by_role_and_label(Role::Button, "Retry").is_some()
    });
    harness.run_steps(20);
    assert_eq!(merges(&host), 2, "Retry ran the merge once");
}

#[test]
fn bug_actions_own_vote_shows_and_a_click_opens_the_menu() {
    let scratch = ScratchDir::new("bug-actions-own-vote");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.sign_in_as_me();
    host.my_review("APPROVED");
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    wait_until(&mut harness, "the own vote", |harness| {
        harness.query_by_role_and_label(Role::Button, "Your vote: Approved").is_some()
    });
    harness.get_by_role_and_label(Role::Button, "Your vote: Approved").click();
    wait_until(&mut harness, "the vote menu", |harness| {
        harness.query_by_label("Request changes…").is_some()
    });
    let reviews = format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/reviews");
    assert!(host.find("POST", &reviews).is_none(), "a second click approves nothing");
}

#[test]
fn bug_actions_header_load_error_offers_retry_and_f5_loads() {
    let scratch = ScratchDir::new("bug-actions-load-error");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.fail_pull_request();
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    harness.get_by_role_and_label(Role::Tab, "Pipelines").click();

    wait_until(&mut harness, "the header's way out", |harness| {
        harness.query_all_by_role_and_label(Role::Button, "Retry").next().is_some()
            && harness.query_by_role_and_label(Role::Button, "Open on GitHub").is_some()
    });
    host.restore_pull_request();
    harness.key_press(egui::Key::F5);
    wait_until(&mut harness, "the pull request", |harness| harness.query_by_label(TITLE).is_some());
}
