//! A pull request page the app itself opens in the browser stays there: the click records a
//! pass ([`cv_shell::browser::passes`]) and the native host's `check` for that pull request answers
//! `pass` without a read, while another pull request is still read and answered `open`. The real
//! app and the real host side against the mocked GitHub Enterprise host
//! ([`super::github_mock`]). Needs `git` on PATH, no network.

use std::time::{Duration, SystemTime};

use cv_runtime::route::PullRequestKey;
use cv_shell::browser::host::{self, InstalledApp};
use cv_shell::browser::passes;
use cv_shell::single_instance::queue_link;
use cv_testkit::ScratchDir;
use egui::Role;
use egui_kittest::kittest::Queryable;
use review_store::AppPaths;
use serde_json::{Value, json};

use super::github_mock::{GitHubHost, HIDDEN_NUMBER, READABLE_NUMBER, REPOSITORY};
use super::walkthrough::{app, wait_until};
use cv_testkit::Clicks;

const CONNECTION_ID: &str = "gh";

#[test]
fn open_on_github_keeps_that_pull_request_in_the_browser() {
    let scratch = ScratchDir::new("browser-pass");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.add_linked_pull_requests(Duration::ZERO);
    let connection = host.connection(CONNECTION_ID, "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
    let list = passes::file(&paths);

    // A link the token cannot read: its toast offers the page on GitHub.
    let hidden = format!("{}/{REPOSITORY}/pull/{HIDDEN_NUMBER}", connection.base_url);
    queue_link(&harness.ctx, &hidden);
    wait_until(&mut harness, "the way out to GitHub", |harness| {
        harness.query_by_role_and_label(Role::Button, "Open on GitHub").is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Open on GitHub").rect());
    wait_until(&mut harness, "the pass", |_| {
        passes::contains(&list, &key(HIDDEN_NUMBER), SystemTime::now())
    });
    assert!(
        !passes::contains(&list, &key(READABLE_NUMBER), SystemTime::now()),
        "only the pull request the app opened has a pass"
    );

    // The host, as the browser starts it, on the same data folders.
    let side = InstalledApp {
        paths,
        http: cv_testkit::http_client(),
        factory: cv_providers::create_provider,
    };
    let hidden_reads = detail_reads(&host, HIDDEN_NUMBER);
    let check = |url: &str| {
        let answer = host::answer(&json!({ "type": "check", "url": url }), &side);
        answer.get("state").and_then(Value::as_str).map(str::to_owned)
    };
    assert_eq!(check(&format!("{hidden}/files")).as_deref(), Some("pass"));
    assert_eq!(detail_reads(&host, HIDDEN_NUMBER), hidden_reads, "a pass costs no read");

    let readable = format!("{}/{REPOSITORY}/pull/{READABLE_NUMBER}", connection.base_url);
    let readable_reads = detail_reads(&host, READABLE_NUMBER);
    assert_eq!(check(&readable).as_deref(), Some("open"));
    assert_eq!(detail_reads(&host, READABLE_NUMBER), readable_reads + 1, "read once");
}

fn key(number: u64) -> PullRequestKey {
    PullRequestKey {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number,
    }
}

/// The host's answers to `GET /pulls/{number}`.
fn detail_reads(host: &GitHubHost, number: u64) -> usize {
    let detail = format!("/api/v3/repos/{REPOSITORY}/pulls/{number}");
    host.requests().iter().filter(|request| request.url.path() == detail).count()
}
