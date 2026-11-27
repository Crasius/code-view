//! Links that reach the window from outside (the command line, a second start, the browser
//! extension): the real app against the mocked GitHub Enterprise host ([`super::github_mock`]).
//! A pull request opens once its token read it; one the token cannot see opens no page and says
//! why. Needs `git` on PATH, no network.

use std::time::Duration;

use egui::Role;
use egui_kittest::kittest::Queryable;

use super::github_mock::{GitHubHost, HIDDEN_NUMBER, READABLE_NUMBER, REPOSITORY};
use super::walkthrough::{app, wait_until};
use cv_runtime::route::{PullRequestKey, Route};
use cv_shell::single_instance::queue_link;
use cv_testkit::ScratchDir;

const CONNECTION_ID: &str = "gh";

#[test]
fn forwarded_link_opens_only_what_its_token_reads() {
    let scratch = ScratchDir::new("forwarded-link");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.add_linked_pull_requests(Duration::ZERO);
    let connection = host.connection(CONNECTION_ID, "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));

    // 404, as GitHub answers a private repository the token cannot see.
    let hidden = format!("{}/{REPOSITORY}/pull/{HIDDEN_NUMBER}", connection.base_url);
    queue_link(&harness.ctx, &hidden);
    let refusal = "widgets #9 did not open: connection Acme cannot find it.";
    wait_until(&mut harness, "the refusal", |harness| {
        harness.query_by_label_contains(refusal).is_some()
    });
    assert_eq!(harness.state().route, Route::MyPullRequests, "nothing opened");
    assert!(harness.query_by_role_and_label(Role::Button, "Fix token").is_some());

    let readable = format!("{}/{REPOSITORY}/pull/{READABLE_NUMBER}/files", connection.base_url);
    queue_link(&harness.ctx, &readable);
    wait_until(&mut harness, "the readable pull request", |harness| {
        harness.state().route == Route::PullRequest(key(READABLE_NUMBER))
    });

    // No connection serves it: the typed-link error with the connection setup.
    queue_link(&harness.ctx, "https://gitlab.com/team/app/-/merge_requests/1");
    wait_until(&mut harness, "the setup offer", |harness| harness.state().route == Route::Settings);
}

fn key(number: u64) -> PullRequestKey {
    PullRequestKey {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number,
    }
}
