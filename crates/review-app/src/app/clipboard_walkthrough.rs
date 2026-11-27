//! Pull request links copied in another app: the real app against the mocked GitHub Enterprise
//! host ([`super::github_mock`]) with a fake clipboard, the window losing and regaining focus like
//! after a switch to the browser. Links the token cannot read stay closed with their ways out;
//! hosts without a clear answer still open. Needs `git` on PATH, no network, never the real
//! clipboard.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::github_mock::{
    FAILING_NUMBER, FORBIDDEN_NUMBER, GitHubHost, HIDDEN_NUMBER, NUMBER, READABLE_NUMBER,
    REPOSITORY, SLOW_NUMBER, TITLE, open_pull_request, pull_path,
};
use super::walkthrough::{
    app, opened_urls, press, saved_settings, set_focus, settle_background, type_into, wait_until,
    wait_until_unsettled,
};
use cv_runtime::cache::page_cache::pull_request_part;
use cv_runtime::route::{PullRequestKey, Route};
use cv_shell::links::clipboard_links::ClipboardSource;
use cv_shell::links::link_access::BUDGET;
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const CONNECTION_ID: &str = "gh";

#[derive(Default)]
struct FakeClipboard(Mutex<Option<String>>);

impl FakeClipboard {
    fn copy(&self, text: &str) {
        *self.0.lock().unwrap() = Some(text.to_owned());
    }
}

impl ClipboardSource for FakeClipboard {
    fn read_text(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
}

#[test]
fn clipboard_link_opens_once_with_back_and_not_again_after_a_restart() {
    let scratch = ScratchDir::new("clipboard-open");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.add_linked_pull_requests(Duration::ZERO);
    let connection = host.connection(CONNECTION_ID, "Acme");
    let link = format!("{}/{REPOSITORY}/pull/{NUMBER}", connection.base_url);
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    let clipboard = fake_clipboard(&mut harness);

    clipboard.copy(&format!("  {link}/files#diff-1\n"));
    come_back(&mut harness);
    wait_until(&mut harness, "the copied pull request", |harness| {
        harness.state().route == Route::PullRequest(key(NUMBER))
    });
    wait_until(&mut harness, "the toast", |harness| {
        harness.query_by_label_contains("Opened widgets #7 from the clipboard").is_some()
    });
    assert!(
        harness.query_by_role_and_label(Role::Button, "Turn off").is_some(),
        "the toast offers to turn it off"
    );
    assert_eq!(
        saved_settings(&harness).last_clipboard_link.as_deref(),
        Some(link.as_str()),
        "the canonical link is saved"
    );

    // Back returns where the user was, like after any navigation.
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Back").rect());
    wait_until(&mut harness, "the start page", |harness| {
        harness.state().route == Route::MyPullRequests
    });

    // The same link, also in another form, does not open again.
    come_back(&mut harness);
    clipboard.copy(&link);
    come_back(&mut harness);
    settle_background(&mut harness);
    assert_eq!(harness.state().route, Route::MyPullRequests, "a handled link stays handled");

    // Not after a restart either.
    drop(harness);
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    let restarted = fake_clipboard(&mut harness);
    restarted.copy(&link);
    come_back(&mut harness);
    settle_background(&mut harness);
    assert_eq!(harness.state().route, Route::MyPullRequests, "the restart remembers the link");

    // A new link opens, and its page reuses the check's read of the detail.
    restarted.copy(&format!("{}/{REPOSITORY}/pull/{READABLE_NUMBER}", connection.base_url));
    come_back(&mut harness);
    // The frame that took the check's answer into the page cache opens the page at its end.
    wait_until_unsettled(&mut harness, "pull request 8", |harness| {
        harness.state().route == Route::PullRequest(key(READABLE_NUMBER))
    });
    let opened_at = harness.state().services.now();
    harness.step();
    let first_frame_at = harness.state().services.now();
    assert!(
        harness.query_by_label_contains(TITLE).is_some(),
        "the first frame of the page shows the detail the check read"
    );
    // The window goes to the background first: the live probe, which asks the detail's URL too,
    // is due `cv_pr_page::live::INTERVAL` of egui time after the page opened, and a settle that
    // steps more frames on a slow machine would reach it; unfocused, it waits for the focus, while
    // the page's own reads go out as with the window in front.
    set_focus(&mut harness, false);
    settle_background(&mut harness);
    // The page cache still holds the check's answer, stored by the frame that opened the page. A
    // detail the page read itself is stored on one of its own frames, from `first_frame_at` on,
    // also when its GET joined one in flight (`review_core::http`) and never reached the host.
    let detail = pull_request_part(&key(READABLE_NUMBER).storage_key(), "detail");
    let stored_at = harness.state().services.cache.stored_at(&detail);
    assert!(
        stored_at.is_some_and(|at| at <= opened_at),
        "the check's answer, not a read of the page's: stored at {stored_at:?}, the page opened \
         at {opened_at} and drew its first frame at {first_frame_at}"
    );
    // On the wire: the check and the page's pipelines, which GitHub lists by head commit
    // (`provider-github`'s `pipelines::runs`), and no live probe. A page read that joined the
    // pipelines' GET would not show here; the page cache above catches it.
    assert_eq!(
        host.count("GET", &pull_path(READABLE_NUMBER)),
        2,
        "the check's read and the pipelines' head commit, no probe"
    );
}

#[test]
fn clipboard_link_the_token_cannot_read_stays_closed_with_ways_out() {
    let scratch = ScratchDir::new("clipboard-no-access");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.add_linked_pull_requests(Duration::ZERO);
    let connection = host.connection(CONNECTION_ID, "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    let clipboard = fake_clipboard(&mut harness);

    // 404, as GitHub answers a private repository the token cannot see.
    let hidden = format!("{}/{REPOSITORY}/pull/{HIDDEN_NUMBER}", connection.base_url);
    clipboard.copy(&hidden);
    come_back(&mut harness);
    let refusal = "widgets #9 did not open: connection Acme cannot find it.";
    wait_until(&mut harness, "the refusal", |harness| {
        harness.query_by_label_contains(refusal).is_some()
    });
    assert_eq!(harness.state().route, Route::MyPullRequests, "nothing opened");
    assert!(harness.query_by_role_and_label(Role::Button, "Fix token").is_some());

    // Once per copy: neither the same copy nor another form of the link checks or toasts again.
    come_back(&mut harness);
    settle_background(&mut harness);
    clipboard.copy(&format!("{hidden}/files#diff-1"));
    come_back(&mut harness);
    settle_background(&mut harness);
    assert_eq!(host.count("GET", &pull_path(HIDDEN_NUMBER)), 1, "one check per copied link");
    assert_eq!(harness.query_all_by_label_contains(refusal).count(), 1, "one toast");

    harness.click_at(harness.get_by_role_and_label(Role::Button, "Open on GitHub").rect());
    let opened = step_collecting_urls(&mut harness);
    assert_eq!(opened, [hidden], "the pull request's page on GitHub");
    assert_eq!(harness.state().route, Route::MyPullRequests);

    // 403: the token reaches the host but not the repository.
    clipboard.copy(&format!("{}/{REPOSITORY}/pull/{FORBIDDEN_NUMBER}", connection.base_url));
    come_back(&mut harness);
    let forbidden = "widgets #10 did not open: the token of connection Acme has no access to it.";
    wait_until(&mut harness, "the 403 refusal", |harness| {
        harness.query_by_label_contains(forbidden).is_some()
    });
    assert_eq!(harness.state().route, Route::MyPullRequests);
    assert!(
        harness.state().services.providers.issues().all().is_empty(),
        "one pull request's refusal raises no connection-wide access banner"
    );
    let fixes = harness.query_all_by_role_and_label(Role::Button, "Fix token").count();
    assert_eq!(fixes, 1, "only the toast offers Fix token");
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Fix token").rect());
    wait_until(&mut harness, "the token setup", |harness| harness.state().route == Route::Settings);
}

/// A 5xx or no answer within the check's budget says nothing about access: the page opens as it
/// did before the check, and reports the failure itself with its Retry.
#[test]
fn clipboard_link_opens_when_the_host_gives_no_clear_answer() {
    let scratch = ScratchDir::new("clipboard-unclear");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.add_linked_pull_requests(BUDGET * 2);
    let connection = host.connection(CONNECTION_ID, "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    let clipboard = fake_clipboard(&mut harness);

    clipboard.copy(&format!("{}/{REPOSITORY}/pull/{FAILING_NUMBER}", connection.base_url));
    come_back(&mut harness);
    wait_until(&mut harness, "the failing pull request", |harness| {
        harness.state().route == Route::PullRequest(key(FAILING_NUMBER))
    });
    assert!(harness.query_by_label_contains("did not open").is_none(), "500 is not no access");

    clipboard.copy(&format!("{}/{REPOSITORY}/pull/{SLOW_NUMBER}", connection.base_url));
    let copied_at = Instant::now();
    come_back(&mut harness);
    wait_until(&mut harness, "the slow pull request", |harness| {
        harness.state().route == Route::PullRequest(key(SLOW_NUMBER))
    });
    assert!(copied_at.elapsed() >= BUDGET, "opened once the budget ran out, not before");
    assert!(
        harness.query_by_label_contains("did not open").is_none(),
        "a timeout is not no access"
    );
}

#[test]
fn clipboard_link_waits_while_a_reply_is_written() {
    let scratch = ScratchDir::new("clipboard-writing");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.add_linked_pull_requests(Duration::ZERO);
    let connection = host.connection(CONNECTION_ID, "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    let clipboard = fake_clipboard(&mut harness);
    open_pull_request(&mut harness);

    // The open pull request's own link changes nothing.
    clipboard.copy(&format!("{}/{REPOSITORY}/pull/{NUMBER}", connection.base_url));
    come_back(&mut harness);
    settle_background(&mut harness);
    assert!(
        harness.query_by_label_contains("from the clipboard").is_none(),
        "no toast for the page already open"
    );
    assert!(harness.query_by_label_contains("You copied a link").is_none());

    press(&mut harness, "Reply");
    type_into(&mut harness, "Reply", "Half an answer");

    clipboard.copy(&format!("{}/{REPOSITORY}/pull/{READABLE_NUMBER}", connection.base_url));
    come_back(&mut harness);
    wait_until(&mut harness, "the toast", |harness| {
        harness.query_by_label_contains("You copied a link to widgets #8").is_some()
    });
    assert_eq!(harness.state().route, Route::PullRequest(key(NUMBER)), "the reply keeps its page");

    harness.click_at(harness.get_by_role_and_label(Role::Button, "Open").rect());
    wait_until(&mut harness, "pull request 8", |harness| {
        harness.state().route == Route::PullRequest(key(READABLE_NUMBER))
    });
}

#[test]
fn clipboard_links_respect_the_setting_and_unknown_hosts() {
    let scratch = ScratchDir::new("clipboard-setting");
    let host = GitHubHost::with_pull_request(scratch.path());
    let connection = host.connection(CONNECTION_ID, "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    let clipboard = fake_clipboard(&mut harness);

    // Off: nothing is read or opened.
    harness.state_mut().services.settings.open_clipboard_links = false;
    clipboard.copy(&format!("{}/{REPOSITORY}/pull/{NUMBER}", connection.base_url));
    come_back(&mut harness);
    settle_background(&mut harness);
    assert_eq!(harness.state().route, Route::MyPullRequests);
    assert!(harness.query_by_label_contains("from the clipboard").is_none());

    // On again: the link opens, and "Turn off" in its toast saves the choice.
    harness.state_mut().services.settings.open_clipboard_links = true;
    come_back(&mut harness);
    wait_until(&mut harness, "the copied pull request", |harness| {
        harness.state().route == Route::PullRequest(key(NUMBER))
    });
    wait_until(&mut harness, "the toast", |harness| {
        harness.query_by_role_and_label(Role::Button, "Turn off").is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Turn off").rect());
    wait_until(&mut harness, "the setting off", |harness| {
        !harness.state().services.settings.open_clipboard_links
    });
    assert!(!saved_settings(&harness).open_clipboard_links, "saved");
    harness.state_mut().services.settings.open_clipboard_links = true;

    // A pull request on a host no connection knows does nothing.
    clipboard.copy("https://example.org/acme/widgets/pull/1");
    come_back(&mut harness);
    settle_background(&mut harness);
    assert_eq!(harness.state().route, Route::PullRequest(key(NUMBER)));
    assert!(harness.query_by_label_contains("copied link").is_none(), "no toast for unknown hosts");

    // One on a platform's cloud host offers the setup for it.
    clipboard.copy("https://gitlab.com/group/app/-/merge_requests/4");
    come_back(&mut harness);
    let offer = "The copied link is for gitlab.com, which has no connection yet.";
    wait_until(&mut harness, "the offer", |harness| {
        harness.query_by_label_contains(offer).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Add connection").rect());
    wait_until(&mut harness, "the setup", |harness| {
        harness.state().route == Route::Settings
            && harness.query_by_label_contains("Add a connection").is_some()
    });
}

fn key(number: u64) -> PullRequestKey {
    PullRequestKey {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number,
    }
}

/// Runs the first frame (the start-up look finds no clipboard), then hands the app a fake one.
fn fake_clipboard(harness: &mut Harness<'_, ReviewApp>) -> Arc<FakeClipboard> {
    harness.step();
    let clipboard = Arc::new(FakeClipboard::default());
    harness.state_mut().clipboard.set_source(clipboard.clone());
    clipboard
}

/// The window goes to the background and comes back, like after copying in the browser. The frame
/// it comes back in starts the clipboard read as background work, so [`settle_background`] after
/// it waits for the read and for what the read started.
fn come_back(harness: &mut Harness<'_, ReviewApp>) {
    set_focus(harness, false);
    set_focus(harness, true);
}

/// The URLs the next frames ask the system to open.
fn step_collecting_urls(harness: &mut Harness<'_, ReviewApp>) -> Vec<String> {
    let mut opened = Vec::new();
    for _ in 0..3 {
        harness.step();
        opened.extend(opened_urls(harness));
    }
    opened
}
