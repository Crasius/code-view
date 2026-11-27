//! Review actions end to end: the real app with the real GitHub provider against a mocked GitHub
//! Enterprise host ([`super::github_mock`]), driven headless through its accessibility tree and
//! pointer events like a user. Each test asserts the request that reached the host. Runs with
//! `cargo test`; needs `git` on PATH, no network.

use std::time::{Duration, Instant};

use egui::{CursorIcon, PointerButton, Pos2, Role, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{AccessKitNode, NodeT, Queryable};
use review_core::Connection;
use review_store::{AppPaths, ConnectionStore, DiffLayout, FilesStart};
use serde_json::{Value, json};

use super::ReviewApp;
use super::github_mock::{
    self, FILE, GitHubHost, NUMBER, REPOSITORY, THREAD_ID, THREAD_TEXT, TITLE,
};
use cv_testkit::Clicks;
use cv_testkit::{self as test_support, ScratchDir};

/// How long a wait may go on while nothing is in flight in the background: the app is idle, and
/// what it waits for did not come.
const TIMEOUT: Duration = Duration::from_secs(60);
/// How long a wait may go on at all. Work in flight counts only here: it goes at the pace of git
/// and the machine, which the whole suite and whatever else runs there share (a git start took
/// 1 s at the median while another project's tests kept the machine 83 % busy). Below the
/// watchdog's limit, so a task that never ends fails its own test, not the test binary.
pub(super) const LIMIT: Duration = Duration::from_secs(180);

#[test]
fn action_line_comment_via_hover_button() {
    let scratch = ScratchDir::new("action-hover-comment");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    let diff = open_diff(&mut harness);

    // The "+" only exists while a line is hovered; move down the right (head) pane until it shows.
    let x = diff.left() + diff.width() * 0.75;
    let mut y = diff.top() + 2.0;
    while harness.query_by_label("Add comment").is_none() {
        assert!(
            y < diff.top() + 120.0,
            "no comment button appeared while hovering the first lines"
        );
        harness.hover_at(pos2(x, y));
        harness.step();
        y += 3.0;
    }
    harness.click_at(harness.get_by_label("Add comment").rect());
    wait_until(&mut harness, "the comment composer", |harness| {
        harness.query_by_label("Comment on line 1").is_some()
    });
    type_into(&mut harness, "Leave a comment", "Please keep the old name as an alias.");
    press_button(&mut harness, "Comment");

    let request = wait_for_request(
        &mut harness,
        &host,
        "POST",
        &format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/comments"),
    );
    assert_eq!(
        request,
        json!({ "body": "Please keep the old name as an alias.", "commit_id": host.head_sha(), "path": FILE, "line": 1, "side": "RIGHT" })
    );
    wait_until(&mut harness, "the success toast", |harness| {
        harness.query_by_label_contains("Comment added.").is_some()
    });
}

#[test]
fn action_line_comment_via_context_menu() {
    let scratch = ScratchDir::new("action-menu-comment");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    let diff = open_diff(&mut harness);

    // First row of the left (base) pane: the deleted line 1.
    let at = pos2(diff.left() + diff.width() * 0.25, diff.top() + 4.0);
    secondary_click(&mut harness, at);
    // The menu opens at the pointer; the hovered line's own "Add comment" sits in the gutter.
    let is_in_menu = |node: &egui_kittest::Node<'_>| node.rect().left() >= at.x - 1.0;
    wait_until(&mut harness, "the context menu", |harness| {
        harness.query_all_by_label("Add comment").any(|node| is_in_menu(&node))
    });
    harness.click_at(
        harness
            .query_all_by_label("Add comment")
            .find(is_in_menu)
            .expect("the menu's Add comment")
            .rect(),
    );
    wait_until(&mut harness, "the comment composer", |harness| {
        harness.query_by_label("Comment on line 1").is_some()
    });
    type_into(&mut harness, "Leave a comment", "Why was greet renamed?");
    press_button(&mut harness, "Comment");

    let request = wait_for_request(
        &mut harness,
        &host,
        "POST",
        &format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/comments"),
    );
    assert_eq!(
        request,
        json!({ "body": "Why was greet renamed?", "commit_id": host.head_sha(), "path": FILE, "line": 1, "side": "LEFT" })
    );
}

#[test]
fn action_reply_to_a_thread_in_the_overview() {
    let scratch = ScratchDir::new("action-reply");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    wait_until(&mut harness, "the existing thread", |harness| {
        harness.query_by_label(THREAD_TEXT).is_some()
    });
    press(&mut harness, "Reply");
    type_into(&mut harness, "Reply", "Yes, other crates use it.");
    press_button(&mut harness, "Reply");

    let request = wait_until_found(&mut harness, "the reply mutation", |_| {
        host.find_graphql("addPullRequestReviewThreadReply")
    });
    assert_eq!(
        request["variables"],
        json!({ "thread": THREAD_ID, "body": "Yes, other crates use it." })
    );
    wait_until(&mut harness, "the success toast", |harness| {
        harness.query_by_label_contains("Reply posted.").is_some()
    });
}

#[test]
fn action_approve_from_the_header() {
    let scratch = ScratchDir::new("action-approve");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    // The header's Approve comes first; the merge status card offers the same vote further down.
    wait_until(&mut harness, "Approve", |harness| {
        harness.query_all_by_label("Approve").next().is_some()
    });
    harness.click_at(
        harness.query_all_by_label("Approve").next().expect("the header's Approve").rect(),
    );
    harness.step();

    let request = wait_for_request(
        &mut harness,
        &host,
        "POST",
        &format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/reviews"),
    );
    assert_eq!(request, json!({ "event": "APPROVE" }));
}

/// After an approval the way often leads back to the list: it refreshes in the background at
/// once, so the rows are current before the user gets there.
#[test]
fn action_an_approval_refreshes_the_list_ahead() {
    let scratch = ScratchDir::new("action-approve-ahead");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.sign_in_as_me();
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    wait_until(&mut harness, "Approve", |harness| {
        harness.query_all_by_label("Approve").next().is_some()
    });
    harness.click_at(
        harness.query_all_by_label("Approve").next().expect("the header's Approve").rect(),
    );
    harness.step();

    let reviews = format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/reviews");
    wait_until_found(&mut harness, "the list's refresh after the approval", |_| {
        let requests = host.requests();
        let vote = requests.iter().position(|request| {
            request.method.as_str() == "POST" && request.url.path() == reviews
        })?;
        requests[vote..].iter().any(is_listing).then_some(())
    });
    assert!(
        harness.query_by_role_and_label(Role::Tab, "Overview").is_some(),
        "the page stays where it was"
    );
}

/// The overview's listing: GitHub's search.
fn is_listing(request: &wiremock::Request) -> bool {
    request.url.path() == "/api/graphql"
        && request.body_json::<Value>().is_ok_and(|body| {
            body["query"].as_str().is_some_and(|query| query.contains("search(type: ISSUE"))
        })
}

#[test]
fn action_add_a_label_from_the_catalog() {
    let scratch = ScratchDir::new("action-labels");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    wait_until(&mut harness, "the current label", |harness| {
        harness.query_by_label("bug").is_some()
    });
    press(&mut harness, "Edit labels");
    wait_until(&mut harness, "the label catalog", |harness| {
        harness.query_by_label("enhancement").is_some()
    });
    press(&mut harness, "enhancement");
    // Picks collect in a draft; Apply sends them in one request.
    press(&mut harness, "Apply");

    let request = wait_for_request(
        &mut harness,
        &host,
        "PUT",
        &format!("/api/v3/repos/{REPOSITORY}/issues/{NUMBER}/labels"),
    );
    assert_eq!(request, json!({ "labels": ["bug", "enhancement"] }));
    assert!(
        host.find("GET", &format!("/api/v3/repos/{REPOSITORY}/labels")).is_some(),
        "the picker loaded the catalog"
    );
}

#[test]
fn action_my_pull_requests_lists_every_host() {
    let scratch = ScratchDir::new("action-hosts");
    let acme =
        GitHubHost::start("acme", &[github_mock::search_row(11, "Tidy the build", "acme/widgets")]);
    let globex = GitHubHost::start(
        "globex",
        &[github_mock::search_row(22, "Bump the parser", "globex/engine")],
    );
    let mut harness = app(
        &scratch,
        &[acme.connection("acme", "Acme"), globex.connection("globex", "Globex Enterprise")],
    );

    wait_until(&mut harness, "rows of both hosts", |harness| {
        harness.query_by_label("11 Tidy the build").is_some()
            && harness.query_by_label("22 Bump the parser").is_some()
    });
    for (host, owner) in [(&acme, "acme"), (&globex, "globex")] {
        let search = host.find_graphql("search(type: ISSUE").expect("a search request");
        let query = search["variables"]["q"].as_str().unwrap_or_default();
        assert!(
            query.contains(&format!("org:{owner}")),
            "{owner} searched with its own qualifier: {query}"
        );
        // The default "All" chip lists everything open in the organisation, not only the user's own.
        assert!(!query.contains("@me"), "{query}");
    }
}

/// The Files tab opens on the summary of every changed file; a card's View opens the file view.
/// The breadcrumb ends at the pull request, whose crumb leads back to the summary of all files.
#[test]
fn action_folder_summary_opens_the_file_view_and_the_trail_leads_back() {
    let scratch = ScratchDir::new("action-folder-summary");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files"))
            })
            .rect(),
    );
    let view = format!("View {FILE}");
    wait_until(&mut harness, "the summary card", |harness| {
        harness.query_by_label("1 changed file").is_some()
            && harness.query_by_label(&view).is_some()
    });
    assert!(
        harness.query_by_label(&format!("Diff of {FILE}")).is_none(),
        "no file opens by itself"
    );

    harness.click_at(harness.get_by_label(&view).rect());
    wait_until(&mut harness, "the file view", |harness| {
        harness.query_by_label(&format!("Diff of {FILE}")).is_some()
    });
    assert!(
        harness.query_by_label("lib.rs (current page)").is_none()
            && harness.query_by_role_and_label(Role::Link, "src").is_none(),
        "the trail names neither the folders nor the file"
    );

    harness.click_at(
        harness.get_by_role_and_label(Role::Link, &format!("Pull request #{NUMBER}")).rect(),
    );
    wait_until(&mut harness, "the summary of all files", |harness| {
        harness.query_by_label("All files (current folder)").is_some()
    });
}

/// Changed besides [`FILE`]: in a folder the tree lists first.
const FIRST_FILE: &str = "app/first.rs";

fn with_first_file(scratch: &ScratchDir) -> GitHubHost {
    let first = github_mock::ExtraFile {
        path: FIRST_FILE,
        base: b"pub fn first() {}\n".to_vec(),
        head: b"pub fn first() -> u32 {\n    1\n}\n".to_vec(),
    };
    GitHubHost::with_pull_request_files(scratch.path(), &[first])
}

fn files_tab(harness: &mut Harness<'_, ReviewApp>) {
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files"))
            })
            .rect(),
    );
}

/// With "First file" chosen, the Files tab opens on the first file of the tree straight away: the
/// folder view never shows on the way. The trail's pull request crumb still leads to the folder
/// view of all files, and a click on a folder in the tree to that folder's.
#[test]
fn action_files_tab_opens_on_the_first_file_when_chosen() {
    let scratch = ScratchDir::new("action-first-file");
    let host = with_first_file(&scratch);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().services.settings.files_start = FilesStart::FirstFile;
    open_pull_request(&mut harness);
    files_tab(&mut harness);
    let diff = format!("Diff of {FIRST_FILE}");
    wait_until(&mut harness, "the first file's diff", |harness| {
        assert!(
            harness.query_by_label("2 changed files").is_none(),
            "the folder view showed before the first file"
        );
        harness.query_by_label(&diff).is_some()
    });
    assert!(
        harness.query_by_label("first.rs (current page)").is_none(),
        "the trail ends at the pull request"
    );

    harness.click_at(
        harness.get_by_role_and_label(Role::Link, &format!("Pull request #{NUMBER}")).rect(),
    );
    wait_until(&mut harness, "the folder view of all files", |harness| {
        harness.query_by_label("All files (current folder)").is_some()
            && harness.query_by_label("2 changed files").is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, "src").rect());
    wait_until(&mut harness, "the folder view of src", |harness| {
        harness.query_by_label("src (current folder)").is_some()
            && harness.query_by_label("1 changed file").is_some()
    });
}

/// A comment's link opens its own file, also when the Files tab would open on the first file.
#[test]
fn action_a_comment_link_wins_over_the_first_file() {
    let scratch = ScratchDir::new("action-first-file-link");
    let host = with_first_file(&scratch);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().services.settings.files_start = FilesStart::FirstFile;
    open_pull_request(&mut harness);
    let link = format!("{FILE}:");
    wait_until(&mut harness, "the comment's link", |harness| {
        harness.query_all_by_label_contains(&link).next().is_some()
    });
    harness.click_at(harness.query_all_by_label_contains(&link).next().expect("the link").rect());
    let diff = format!("Diff of {FILE}");
    wait_until(&mut harness, "the commented file", |harness| {
        assert!(
            harness.query_by_label(&format!("Diff of {FIRST_FILE}")).is_none(),
            "the first file opened instead of the comment's"
        );
        harness.query_by_label(&diff).is_some()
    });
}

/// The app with `connections` saved, a side-by-side diff and the "My pull requests" start page.
pub(super) fn app(scratch: &ScratchDir, connections: &[Connection]) -> Harness<'static, ReviewApp> {
    let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    for connection in connections {
        store.upsert(connection.clone()).expect("store the connection");
    }
    let mut harness =
        super::startup::start(Harness::builder().with_size([1440.0, 900.0]), |creation| {
            ReviewApp::new(creation, paths, cv_providers::create_provider)
        });
    harness.state_mut().services.settings.diff_layout = DiffLayout::SideBySide;
    harness
}

/// Opens the pull request from its row in "My pull requests" and waits for its tabs.
pub(super) fn open_pull_request(harness: &mut Harness<'_, ReviewApp>) {
    let row = format!("{NUMBER} {TITLE}");
    wait_until(harness, "the pull request row", |harness| harness.query_by_label(&row).is_some());
    harness.click_at(harness.get_by_label(&row).rect());
    wait_until(harness, "the pull request", |harness| {
        harness.query_by_role_and_label(Role::Tab, "Overview").is_some()
    });
}

/// Opens [`FILE`] in the Files tab once the mirror fetched it; returns the diff's area.
pub(super) fn open_diff(harness: &mut Harness<'_, ReviewApp>) -> egui::Rect {
    // "Files" gains its count once the file list is in.
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files"))
            })
            .rect(),
    );
    wait_until(harness, "the file tree", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, FILE).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, FILE).rect());
    let label = format!("Diff of {FILE}");
    // The existing thread's card sits under line 3 once the diff rows are built.
    wait_until(harness, "the diff", |harness| {
        harness.query_by_label(&label).is_some() && harness.query_by_label(THREAD_TEXT).is_some()
    });
    harness.get_by_label(&label).rect()
}

/// A right click at `position`, the pointer resting there first: the menu is the hovered line's.
fn secondary_click(harness: &mut Harness<'_, ReviewApp>, position: Pos2) {
    harness.hover_at(position);
    harness.step();
    harness.click_point(position, PointerButton::Secondary);
    harness.step();
}

/// Types into the text field whose hint starts with `placeholder` and returns once the field holds
/// the text: a field takes typed text only while it has the focus, so a step that follows (a
/// submit button that is enabled only with text) never runs on a field that missed it.
pub(super) fn type_into(harness: &mut Harness<'_, ReviewApp>, placeholder: &str, text: &str) {
    let is_field = |node: &AccessKitNode<'_>| {
        node.placeholder().is_some_and(|hint| hint.starts_with(placeholder))
    };
    wait_until(harness, placeholder, |harness| harness.query_by(is_field).is_some());
    harness.get_by(is_field).focus();
    harness.step();
    let field = harness.get_by(is_field);
    field.type_text(text);
    // Found by its id: a field with text may drop its hint.
    let id = field.accesskit_node().id();
    wait_until(harness, &format!("{placeholder} holding {text:?}"), |harness| {
        harness
            .query_by(|node| node.id() == id)
            .is_some_and(|field| field.value().is_some_and(|value| value.contains(text)))
    });
}

/// Scrolls the labelled widget into view, then clicks it like a pointer would (see
/// [`click_when_ready`]).
pub(super) fn press(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    click_when_ready(harness, label, None);
}

/// [`press`] for the button labelled `label`: for a submit button that is enabled only with text,
/// and where another widget carries the same label (a composer's text area is named after its
/// submit button).
pub(super) fn press_button(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    click_when_ready(harness, label, Some(Role::Button));
}

/// The widget labelled `label`, of `role` where one is given.
fn labelled<'h>(
    harness: &'h Harness<'_, ReviewApp>,
    label: &'h str,
    role: Option<Role>,
) -> Option<egui_kittest::Node<'h>> {
    match role {
        Some(role) => harness.query_by_role_and_label(role, label),
        None => harness.query_by_label(label),
    }
}

/// Scrolls the widget labelled `label` into view, waits until it is enabled with its centre inside
/// the window (scrolling again where late content pushed it out), then clicks it at its place in
/// that frame ([`Clicks::click_at`]). A fixed number of frames after the scroll proves none of
/// that: the click would land on a disabled button, or past the window's edge where nothing takes
/// it, and the test would wait for a request that never comes.
fn click_when_ready(harness: &mut Harness<'_, ReviewApp>, label: &str, role: Option<Role>) {
    wait_until(harness, label, |harness| labelled(harness, label, role).is_some());
    labelled(harness, label, role).expect("just found").scroll_to_me();
    settle(harness);
    let window = harness.ctx.content_rect();
    let rect = wait_for(
        harness,
        &format!("{label} to take a click"),
        |harness| {
            let node = labelled(harness, label, role)?;
            let rect = node.rect();
            let is_enabled = !node.accesskit_node().is_disabled();
            (is_enabled && window.contains(rect.center())).then_some(rect)
        },
        |harness| {
            if let Some(node) = labelled(harness, label, role)
                && !window.contains(node.rect().center())
            {
                node.scroll_to_me();
            }
        },
    );
    harness.click_at(rect);
    harness.step();
}

/// Waits until the text labelled `label` is a link where it is drawn: the pointer resting on it
/// turns into a hand, and the text stays in place. An answer's citations become links only once
/// the background checked them against the checkout, and their icons then move the text; a click
/// before that lands on plain text. Returns the point to click.
pub(super) fn wait_for_link(harness: &mut Harness<'_, ReviewApp>, label: &str) -> Pos2 {
    let _step = test_support::step(format!("waiting for {label} to become a link"));
    let started = Instant::now();
    loop {
        let center = harness.query_by_label(label).map(|node| node.rect().center());
        if let Some(center) = center {
            harness.hover_at(center);
        }
        harness.step();
        let is_link = harness.output().platform_output.cursor_icon == CursorIcon::PointingHand;
        let now = harness.query_by_label(label).map(|node| node.rect().center());
        if let Some(center) = center.filter(|center| is_link && now == Some(*center)) {
            return center;
        }
        assert!(started.elapsed() < TIMEOUT, "{label} did not become a link");
        test_support::let_threads_run(Duration::from_millis(20));
    }
}

/// Waits for the request and returns its JSON body.
fn wait_for_request(
    harness: &mut Harness<'_, ReviewApp>,
    host: &GitHubHost,
    verb: &str,
    request_path: &str,
) -> Value {
    let request = wait_until_found(harness, request_path, |_| host.find(verb, request_path));
    request.body_json().expect("a JSON body")
}

/// Whether an open pull request holds `path`'s diff in its cache.
pub(super) fn has_cached_diff(harness: &Harness<'_, ReviewApp>, path: &str) -> bool {
    harness.state().pull_requests.iter().any(|(_, page)| page.has_cached_diff(path))
}

/// Steps until no background work (git, the provider) is left for a few frames: what was on its
/// way has landed, and the frame after it took it in. A late answer can still change the layout
/// (a card above growing), so a test reads a place to click only after this. Each step moves
/// egui's clock on by a quarter second, past the dwell of a hover.
pub(super) fn settle_background(harness: &mut Harness<'_, ReviewApp>) {
    let _step = test_support::step("waiting for the background work to end over 3 frames");
    let started = Instant::now();
    let mut idle_frames = 0;
    while idle_frames < 3 {
        harness.step();
        let is_idle = harness.state().services.background.running() == 0;
        idle_frames = if is_idle { idle_frames + 1 } else { 0 };
        assert!(started.elapsed() < LIMIT, "background work never ended");
        test_support::let_threads_run(Duration::from_millis(5));
    }
}

pub(super) fn wait_until(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    wait_until_found(harness, what, |harness| is_done(harness).then_some(()));
}

fn wait_until_found<T>(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    find: impl Fn(&Harness<'_, ReviewApp>) -> Option<T>,
) -> T {
    let found = wait_for(harness, what, find, |_| {});
    settle(harness);
    found
}

/// Steps the app until `find` finds something and returns it on the frame it came, without
/// settling; `each` runs before every frame (a hover, a click). Fails after [`TIMEOUT`] with
/// nothing in flight in the background, or after [`LIMIT`].
pub(super) fn wait_for<T>(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    mut find: impl FnMut(&Harness<'_, ReviewApp>) -> Option<T>,
    each: impl Fn(&mut Harness<'_, ReviewApp>),
) -> T {
    let _step = test_support::step(format!("waiting for {what}"));
    let (started, mut idle) = (Instant::now(), Duration::ZERO);
    loop {
        let (is_busy, frame) = (harness.state().services.background.running() > 0, Instant::now());
        each(harness);
        harness.step();
        if let Some(found) = find(harness) {
            return found;
        }
        if idle > TIMEOUT || started.elapsed() > LIMIT {
            let labels: Vec<String> = harness
                .query_all_by(|_| true)
                .filter_map(|node| {
                    let node = node.accesskit_node();
                    let text = match (node.label(), node.value()) {
                        (Some(label), Some(value)) => Some(format!("{label} = {value:?}")),
                        (label, value) => label.or(value),
                    };
                    let state =
                        [(node.is_disabled(), " (disabled)"), (node.is_focused(), " (focused)")];
                    text.map(|text| {
                        state.iter().filter(|(is, _)| *is).fold(text, |text, (_, tag)| text + tag)
                    })
                })
                .collect();
            panic!("timed out waiting for {what}; screen: {labels:?}");
        }
        test_support::let_threads_run(Duration::from_millis(20));
        if !is_busy {
            idle += frame.elapsed();
        }
    }
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..4 {
        harness.step();
    }
}
