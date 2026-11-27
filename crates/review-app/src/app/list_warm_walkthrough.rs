//! Opening a pull request from the list, end to end: the click (or the title bar's search) hands
//! the row to the page, whose header paints it while the detail is still on its way; a pointer
//! resting on a row reads the pages around it ahead, so the click then fetches no commits again
//! and joins a detail request still on its way.

use std::time::{Duration, Instant};

use egui::{Key, Modifiers, Pos2, Rect, Role, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{Connection, ProviderKind};
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::ReviewApp;
use super::action_e2e::{app, wait_until};
use super::github_mock::{GitHubHost, NUMBER, OWNER, REPOSITORY, TITLE, search_row};
use super::request_budget::wait_idle;
use cv_runtime::route::{PullRequestKey, Route};
use cv_testkit::Clicks;
use cv_testkit::{self as test_support, ScratchDir};

/// Longer than any test runs: the detail never arrives while the header is checked.
const DETAIL_DELAY: Duration = Duration::from_secs(120);
/// How long the request for the detail may take to reach the mock in real time.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// A GitHub Enterprise stand-in whose "mine" search lists pull request [`NUMBER`], and whose detail
/// answers only after [`DETAIL_DELAY`]. Everything else is a quick 404.
struct SlowDetail {
    runtime: tokio::runtime::Runtime,
    server: MockServer,
}

impl SlowDetail {
    fn start() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("mock runtime");
        let server = runtime.block_on(MockServer::start());
        cv_testkit::fresh_origin(&server.uri());
        let ok = |body| ResponseTemplate::new(200).set_body_json(body);
        let search = json!({ "data": { "search": {
            "pageInfo": { "hasNextPage": false, "endCursor": null },
            "nodes": [search_row(NUMBER, TITLE, REPOSITORY)]
        } } });
        let mocks = [
            Mock::given(method("GET"))
                .and(path(format!("/api/v3/users/{OWNER}")))
                .respond_with(ok(json!({ "login": OWNER, "type": "Organization" }))),
            Mock::given(method("POST"))
                .and(path("/api/graphql"))
                .and(body_string_contains("search(type: ISSUE"))
                .respond_with(ok(search)),
            // The detail is the pull and its reviews, asked together: only the pull is slow.
            Mock::given(method("GET"))
                .and(path(format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}")))
                .respond_with(ok(json!({})).set_delay(DETAIL_DELAY)),
            Mock::given(method("GET"))
                .and(path(format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/reviews")))
                .respond_with(ok(json!([]))),
        ];
        for mock in mocks {
            runtime.block_on(mock.mount(&server));
        }
        Self { runtime, server }
    }

    fn connection(&self) -> Connection {
        Connection {
            id: "gh".to_owned(),
            name: "Acme".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: self.server.uri(),
            organizations: vec![OWNER.to_owned()],
            token: Some("mock-only".to_owned()),
        }
    }

    /// Waits in real time, without a frame, until the detail was asked for: the request leaves on
    /// the app's background runtime, which may not have sent it yet when the page painted.
    fn wait_for_detail_request(&self) {
        let started = Instant::now();
        while !self.was_detail_asked() {
            assert!(started.elapsed() < REQUEST_TIMEOUT, "the detail was never asked for");
            test_support::let_threads_run(Duration::from_millis(10));
        }
    }

    /// The detail was asked for (and is held back).
    fn was_detail_asked(&self) -> bool {
        self.detail_requests() > 0
    }

    /// The requests for the detail so far.
    fn detail_requests(&self) -> usize {
        let detail = format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}");
        let requests = self.runtime.block_on(self.server.received_requests()).unwrap_or_default();
        requests.iter().filter(|request| request.url.path() == detail).count()
    }
}

#[test]
fn the_header_paints_the_clicked_row_while_the_detail_loads() {
    let scratch = ScratchDir::new("list-warm-header");
    let host = SlowDetail::start();
    let mut harness = app(&scratch, &[host.connection()]);
    let row = format!("{NUMBER} {TITLE}");
    wait_until(&mut harness, "the pull request row", |harness| {
        harness.query_by_label(&row).is_some()
    });
    harness.click_at(harness.get_by_label(&row).rect());
    // The frame the click lands in leaves the list, and the page it opens paints in that frame.
    let mut frames = 0;
    while harness.query_by_label(&row).is_some() {
        assert!(frames < 2, "the click opens the page");
        harness.step();
        frames += 1;
    }
    assert!(harness.query_by_label(TITLE).is_some(), "the header shows the row's title at once");
    assert!(
        harness.query_by_label("Copy link to #7").is_some(),
        "and its number, which copies the row's link"
    );
    // While the detail is on its way.
    host.wait_for_detail_request();
    assert!(harness.query_by_label("Updating…").is_some(), "the row is not shown as final");
    assert!(
        harness.query_by_label("More actions").is_none(),
        "votes, merge and the menu wait for the detail"
    );
}

#[test]
fn the_header_paints_the_row_the_title_bar_search_opened() {
    let scratch = ScratchDir::new("list-warm-jump");
    let host = SlowDetail::start();
    let mut harness = app(&scratch, &[host.connection()]);
    let row = format!("{NUMBER} {TITLE}");
    wait_until(&mut harness, "the pull request row", |harness| {
        harness.query_by_label(&row).is_some()
    });
    harness.key_press_modifiers(Modifiers::COMMAND, Key::K);
    harness.event(egui::Event::Text(TITLE.to_owned()));
    for _ in 0..3 {
        harness.step();
    }
    harness.key_press(Key::Enter);
    // The frame the Enter lands in leaves the list, and the page it opens paints in that frame.
    let mut frames = 0;
    while harness.query_by_label(&row).is_some() {
        assert!(frames < 2, "Enter opens the page");
        harness.step();
        frames += 1;
    }
    assert!(
        matches!(harness.state().route, Route::PullRequest(PullRequestKey { number: NUMBER, .. })),
        "the search opened the pull request"
    );
    assert!(harness.query_by_label(TITLE).is_some(), "the header shows the row's title at once");
    // While the detail is on its way.
    host.wait_for_detail_request();
    assert!(harness.query_by_label("Updating…").is_some(), "the row is not shown as final");
}

/// The list row of pull request [`NUMBER`], once it shows.
fn row_rect(harness: &mut Harness<'_, ReviewApp>) -> Rect {
    let row = format!("{NUMBER} {TITLE}");
    wait_until(harness, "the pull request row", |harness| harness.query_by_label(&row).is_some());
    harness.get_by_label(&row).rect()
}

/// A point on the row's title, clear of its buttons.
fn on_title(row: Rect) -> Pos2 {
    row.left_center() + vec2(120.0, 0.0)
}

/// The pull request's detail (`GET /pulls/7`, with its reviews), and the repository over git: the
/// mirror's fetches of commits and blobs.
fn pulls_and_git(host: &GitHubHost) -> (usize, usize) {
    let requests: Vec<Request> = host.requests();
    let pull = format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}");
    let git = format!("/{REPOSITORY}.git/");
    let pulls = requests.iter().filter(|request| request.url.path() == pull).count();
    let fetches = requests.iter().filter(|request| request.url.path().starts_with(&git)).count();
    (pulls, fetches)
}

#[test]
fn a_resting_pointer_warms_the_page_so_the_click_fetches_nothing_again() {
    let scratch = ScratchDir::new("list-warm-rest");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    let row = row_rect(&mut harness);
    assert_eq!(pulls_and_git(&host), (0, 0), "the list alone asks for neither");
    harness.hover_at(on_title(row));
    // The detail lands first, the commits and the counted files after it. The detail is fresh
    // only for a while after it landed, longer than the harness's quick clock may leave it here
    // (the click joining a detail on its way is the next test).
    wait_until(&mut harness, "the warmed page", |harness| {
        harness.query_by_label_contains("lines changed in 1 file").is_some()
    });
    let (pulls, fetches) = pulls_and_git(&host);
    assert_eq!(pulls, 1, "the warm-up asked for the detail once");
    assert!(fetches > 0, "and fetched the commits, a small pull request counted");

    harness.click_at(harness.get_by_label(&format!("{NUMBER} {TITLE}")).rect());
    wait_until(&mut harness, "the listed files", |harness| {
        harness
            .query_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files ("))
            })
            .is_some()
    });
    wait_idle(&mut harness);
    assert!(harness.query_by_label("More actions").is_some(), "the detail is on screen");
    assert_eq!(
        pulls_and_git(&host).1,
        fetches,
        "no fetch: the commits and the counted blobs came from the mirror"
    );
}

/// The page a click opens while the read ahead of its detail is on its way joins that request
/// instead of sending its own (the HTTP layer shares identical requests in flight).
#[test]
fn a_click_while_the_detail_is_read_ahead_joins_its_request() {
    let scratch = ScratchDir::new("list-warm-join");
    let host = SlowDetail::start();
    let mut harness = app(&scratch, &[host.connection()]);
    let row = row_rect(&mut harness);
    harness.hover_at(on_title(row));
    wait_until(&mut harness, "the read ahead of the rested row", |_| host.was_detail_asked());
    harness.click_at(harness.get_by_label(&format!("{NUMBER} {TITLE}")).rect());
    for _ in 0..10 {
        harness.step();
    }
    // Time for a second request to reach the mock, would the page send one.
    test_support::let_threads_run(Duration::from_millis(300));
    harness.step();
    assert!(matches!(harness.state().route, Route::PullRequest(_)), "the click opened the page");
    assert!(harness.query_by_label("Updating…").is_some(), "which waits for the detail");
    assert_eq!(host.detail_requests(), 1, "on the request the list sent");
}

#[test]
fn a_pointer_that_moves_on_before_the_dwell_starts_nothing() {
    let scratch = ScratchDir::new("list-warm-pass");
    let host = SlowDetail::start();
    let mut harness = app(&scratch, &[host.connection()]);
    let row = row_rect(&mut harness);
    // A quarter second a frame: the pointer never stays the 400 ms on the row.
    for step in 0..8u8 {
        harness.hover_at(on_title(row) + vec2(f32::from(step) * 6.0, 0.0));
        harness.step();
    }
    harness.hover_at(Pos2::new(row.center().x, row.bottom() + 200.0));
    for _ in 0..8 {
        harness.step();
    }
    assert!(!host.was_detail_asked(), "nothing warmed while the pointer was on its way");

    harness.hover_at(on_title(row));
    for _ in 0..3 {
        harness.step();
    }
    wait_until(&mut harness, "the warm-up of the rested row", |_| host.was_detail_asked());
}

/// The detail requests of pull request `number`.
fn detail_requests(host: &GitHubHost, number: u64) -> usize {
    let pull = format!("/api/v3/repos/{REPOSITORY}/pulls/{number}");
    host.requests().iter().filter(|request| request.url.path() == pull).count()
}

/// Resting on the second row reads the first three: the rested one and those painted above and
/// below it (a draft too); the fourth stays unread.
#[test]
fn a_resting_pointer_reads_the_rows_around_it_ahead() {
    let scratch = ScratchDir::new("list-warm-around");
    let mut draft = search_row(8, "Charlie", REPOSITORY);
    draft["isDraft"] = json!(true);
    let rows = [
        search_row(5, "Alpha", REPOSITORY),
        search_row(6, "Bravo", REPOSITORY),
        draft,
        search_row(9, "Delta", REPOSITORY),
    ];
    let host = GitHubHost::start(OWNER, &rows);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    let rows = [(5, "5 Alpha"), (6, "6 Bravo"), (8, "8 Charlie"), (9, "9 Delta")];
    wait_until(&mut harness, "the rows", |harness| {
        rows.iter().all(|(_, label)| harness.query_by_label(label).is_some())
    });
    // Top to bottom, as painted: (number, rect).
    let mut painted: Vec<(u64, Rect)> =
        rows.iter().map(|(number, label)| (*number, harness.get_by_label(label).rect())).collect();
    painted.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
    let order: Vec<u64> = painted.iter().map(|(number, _)| *number).collect();
    let rested = painted[1].1;

    harness.hover_at(on_title(rested));
    wait_until(&mut harness, "the reads around the rested row", |_| {
        order[..3].iter().all(|number| detail_requests(&host, *number) > 0)
    });
    wait_idle(&mut harness);
    for number in &order[..3] {
        assert_eq!(
            detail_requests(&host, *number),
            1,
            "#{number}, at or beside the pointer, read once"
        );
    }
    assert_eq!(detail_requests(&host, order[3]), 0, "#{}, two rows away, not read", order[3]);
}
