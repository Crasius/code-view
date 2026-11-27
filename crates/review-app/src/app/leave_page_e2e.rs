//! Leaving a pull request through the title bar's breadcrumb or the navigation rail: the page being
//! left must not run in the frame of the click. The real app with the real GitHub provider against
//! the mocked host ([`super::github_mock`]); time is simulated, so the live probe comes due within a
//! few hundred frames. Needs `git` on PATH, no network.
//!
//! The probe is made due while the window is inactive, so it is pending, and the click is the
//! window's first active frame: had the page run in that frame, the probe would have gone out.

use egui::{Event, PointerButton, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::github_mock::{GitHubHost, NUMBER, REPOSITORY, open_pull_request};
use super::walkthrough::{app, wait_idle, wait_until};
use cv_runtime::route::{PullRequestKey, Route};
use cv_testkit::ScratchDir;

/// More than the probe interval with a token (`live::INTERVAL`, 30 s).
const PROBE_DUE_AFTER: f64 = 35.0;

#[test]
fn leave_page_by_breadcrumb_or_rail_sends_nothing_for_the_page_left() {
    let scratch = ScratchDir::new("leave-page");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    pump(&mut harness, 5.0);

    // Control: a probe that came due while the window was away goes out in its first active frame.
    let before = pull_request_reads(&host);
    make_probe_due_while_inactive(&mut harness);
    set_focus(&mut harness, true);
    // The probe leaves in time; its arrival at the host goes at the machine's pace.
    pump(&mut harness, 2.0);
    wait_until(&mut harness, "the pending probe to go out once the window is active", |_| {
        pull_request_reads(&host) > before
    });

    // The breadcrumb click is the first active frame: the pull request page does not run in it.
    pump(&mut harness, 5.0);
    make_probe_due_while_inactive(&mut harness);
    let before = pull_request_reads(&host);
    // Opened from the list, the pull request's trail leads back to it.
    click_as_first_active_frame(&mut harness, Role::Link, "My pull requests");
    pump(&mut harness, 5.0);
    assert_eq!(harness.state().route, Route::MyPullRequests, "the breadcrumb leads to the list");
    assert_eq!(
        pull_request_reads(&host),
        before,
        "nothing went out for the pull request left by the breadcrumb"
    );

    // The same through the rail, after coming back to the pull request.
    let key = PullRequestKey {
        connection_id: "gh".to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number: NUMBER,
    };
    harness.state_mut().navigator.go(Route::PullRequest(key));
    pump(&mut harness, 5.0);
    make_probe_due_while_inactive(&mut harness);
    let before = pull_request_reads(&host);
    click_as_first_active_frame(&mut harness, Role::Button, "Pull requests");
    pump(&mut harness, 5.0);
    assert_eq!(harness.state().route, Route::MyPullRequests, "the rail leads to the list");
    assert_eq!(
        pull_request_reads(&host),
        before,
        "nothing went out for the pull request left by the rail"
    );
}

/// REST reads of the pull request: its detail (the probe) and everything below it.
fn pull_request_reads(host: &GitHubHost) -> usize {
    let detail = format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}");
    host.requests()
        .iter()
        .filter(|request| {
            let path = request.url.path();
            path == detail || path.starts_with(&format!("{detail}/"))
        })
        .count()
}

/// The window loses focus and a probe interval passes: the probe is pending, not sent. The reads
/// still in flight from before (the reload after coming back to the page) land here, so a busy
/// machine does not count them against the click that follows.
fn make_probe_due_while_inactive(harness: &mut Harness<'_, ReviewApp>) {
    set_focus(harness, false);
    pump(harness, PROBE_DUE_AFTER);
    wait_idle(harness);
    assert!(harness.state().services.refresh.pending_count() > 0, "the probe waits for the window");
}

/// Presses on the widget while the window is inactive and releases it in the window's first active
/// frame, the frame in which the click counts.
fn click_as_first_active_frame(harness: &mut Harness<'_, ReviewApp>, role: Role, label: &str) {
    let position = harness.get_by_role_and_label(role, label).rect().center();
    harness.hover_at(position);
    harness.step();
    let button = |pressed| Event::PointerButton {
        pos: position,
        button: PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    harness.event(button(true));
    harness.step();
    focus_input(harness, true);
    harness.event(button(false));
    harness.step();
}

fn set_focus(harness: &mut Harness<'_, ReviewApp>, is_focused: bool) {
    focus_input(harness, is_focused);
    harness.step();
}

fn focus_input(harness: &mut Harness<'_, ReviewApp>, is_focused: bool) {
    harness.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().focused =
        Some(is_focused);
    harness.input_mut().focused = is_focused;
}

/// Advances simulated time; each frame also gives background work a moment of real time.
fn pump(harness: &mut Harness<'_, ReviewApp>, seconds: f64) {
    let time = |harness: &Harness<'_, ReviewApp>| harness.ctx.input(|input| input.time);
    let end = time(harness) + seconds;
    while time(harness) < end {
        harness.step();
        cv_testkit::let_threads_run(std::time::Duration::from_millis(3));
    }
}
