//! Removing a connection while one of its pull requests is still open in the background: the page
//! closes before retention deletes the connection's drafts, read marks and viewed state, so the
//! page's last save cannot bring the folders back, and neither back nor an old link opens the page
//! again. The real app with the real GitHub provider
//! against the mocked host ([`super::github_mock`]). Needs `git` on PATH, no network.

use egui::Role;
use egui_kittest::kittest::Queryable;
use review_store::stable_hash;

use super::github_mock::{GitHubHost, open_pull_request};
use super::walkthrough::{app, press, wait_until};
use cv_runtime::route::Route;
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

#[test]
fn removing_a_connection_closes_its_pages_before_its_data_goes() {
    let scratch = ScratchDir::new("remove-connection");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    let paths = harness.state().services.paths.clone();
    let threads = paths.threads_dir().join(stable_hash("gh"));
    let viewed = paths.viewed_dir().join(stable_hash("gh"));
    open_pull_request(&mut harness);
    let Route::PullRequest(key) = harness.state().route.clone() else {
        panic!("the pull request is open");
    };
    wait_until(&mut harness, "the first visit's read marks and the last place", |_| {
        threads.exists() && viewed.exists()
    });

    harness.state_mut().navigator.go(Route::Settings);
    harness.step();
    wait_until(&mut harness, "the connection's row", |harness| {
        harness.query_by_role_and_label(Role::Button, "Acme").is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Acme").rect());
    press(&mut harness, "Remove…");
    press(&mut harness, "Remove connection");
    assert!(
        harness.state().pull_requests.iter().all(|(key, _)| key.connection_id != "gh"),
        "the removed connection's page is closed"
    );
    wait_until(&mut harness, "the connection's folders to go", |_| {
        !threads.exists() && !viewed.exists()
    });
    assert!(
        harness.query_by_role_and_label(Role::Button, "Acme").is_none(),
        "the connection is gone from the list"
    );

    // Back leads past the removed connection's page, and an old link to it opens nothing.
    harness.state_mut().navigator.back();
    harness.step();
    assert_eq!(harness.state().route, Route::MyPullRequests, "back skips the pull request");
    harness.state_mut().navigator.go(Route::PullRequest(key));
    harness.step();
    assert_eq!(harness.state().route, Route::MyPullRequests, "the pull request is refused");
    assert!(harness.state().pull_requests.is_empty(), "no page opened for it");
    for _ in 0..30 {
        harness.step();
    }
    assert!(!threads.exists() && !viewed.exists(), "nothing was written for it");

    drop(harness);
    assert!(!threads.exists(), "no last save brought the drafts and read marks back");
    assert!(!viewed.exists(), "nor the viewed state");
}
