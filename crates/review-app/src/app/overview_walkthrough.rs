//! The pull request list (`list_*`) and the pull request page's Overview tab (`overview_tab_*`),
//! with stale-while-revalidate, driven headless (`egui_kittest`) against a mock GitLab that counts
//! every request. Time is simulated through egui's input time.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{Connection, ProviderKind};
use review_store::{AppPaths, AppSettings};
use serde_json::{Value, json};
use wiremock::{Request, Respond, ResponseTemplate};

use super::ReviewApp;
use super::gitlab_mock::{merge_request, merge_request_route};
use super::mock_host::MockHost;
use super::walkthrough::{
    app_with, has, press, pump, saved_paths, saved_settings, set_focus, settle_background,
    wait_until, wait_until_unsettled,
};
use cv_lists::pull_request_list::{GROUP_BY_REPOSITORY, LIST_INTERVAL, SEARCH_TOOLTIP};
use cv_runtime::cache::page_cache::REVISIT_FRESH;
use cv_runtime::route::{PullRequestKey, Route};
use cv_shell::attention::Attention;
use cv_testkit::{Clicks, ScratchDir, samples};

const TOKEN: &str = "glpat-mock-only";
const GROUP: &str = "team";
const PROJECT: &str = "team/app";
const LISTING: &str = "/api/v4/groups/team/merge_requests";

#[test]
fn list_revisit_shows_known_rows_at_once_then_updates() {
    let host = GitLabGroup::start(1);
    let scratch = ScratchDir::new("overview-revisit-list");
    let mut harness = host.harness(&scratch, "revisit-list");
    wait_until_unsettled(&mut harness, "the first rows", |harness| has(harness, "Change 1"));
    assert_eq!(host.count(LISTING), 1, "one listing on the first visit");
    assert!(
        has(&harness, "All") && has(&harness, "Created by me") && has(&harness, "Assigned to me")
    );

    // A revisit within the freshness window shows the rows and asks nothing. A revisit's refresh
    // would leave on its first frame, in front; the window goes to the background before the
    // settle, so the list's own refresh (`LIST_INTERVAL`) waits for the focus however many frames
    // a slow machine steps. Coming back is no return that refreshes the page shown
    // (`Attention::without_pauses`).
    harness.state_mut().attention = Attention::without_pauses();
    go(&mut harness, Route::Settings);
    pump(&mut harness, 2.0);
    go(&mut harness, Route::MyPullRequests);
    pump(&mut harness, 2.0);
    set_focus(&mut harness, false);
    settle_background(&mut harness);
    assert_eq!(host.count(LISTING), 1, "a fresh answer is not asked for again");
    set_focus(&mut harness, true);

    // Later the host has news, and answers slowly.
    go(&mut harness, Route::Settings);
    pump(&mut harness, REVISIT_FRESH.as_secs_f64() + 5.0);
    host.publish(2, Duration::from_millis(1500));
    harness.state_mut().navigator.go(Route::MyPullRequests);
    harness.step();
    let revisited_at = harness.ctx.input(|input| input.time);
    assert_eq!(harness.state().route, Route::MyPullRequests);
    // The first frame of the revisited page: the known row is there, no skeleton, a quiet note.
    harness.step();
    assert!(has(&harness, "Change 1"), "known rows paint in the first frame");
    assert!(!has(&harness, "Change 2"));
    assert!(has(&harness, "Updating…"), "the background refresh says so");

    wait_until_unsettled(&mut harness, "the refreshed rows", |harness| has(harness, "Change 2"));
    assert!(has(&harness, "Change 1"));
    wait_until(&mut harness, "the note to leave with the new answer", |harness| {
        !has(harness, "Updating…")
    });
    assert_eq!(host.count(LISTING), 2, "exactly one background refresh for the revisit");

    // Idle on the list sends nothing until its own refresh is due, one interval after the
    // revisit's; Refresh (F5) always asks.
    let idle = LIST_INTERVAL.as_secs_f64() - (harness.ctx.input(|input| input.time) - revisited_at);
    pump(&mut harness, idle - 5.0);
    assert_eq!(host.count(LISTING), 2, "an idle list sends nothing within its interval");
    pump(&mut harness, 10.0);
    wait_until_unsettled(&mut harness, "the list's own refresh", |_| host.count(LISTING) >= 3);
    harness.click_at(harness.get_by_label("Refresh (F5)").rect());
    // The request leaves on a background thread; under a loaded test run it can trail the frames.
    wait_until_unsettled(&mut harness, "the refresh request", |_| host.count(LISTING) >= 4);
    pump(&mut harness, 2.0);
    assert_eq!(host.count(LISTING), 4);
}

#[test]
fn overview_tab_revisit_paints_the_known_detail_even_after_it_was_dropped() {
    let host = GitLabGroup::start(5);
    let scratch = ScratchDir::new("overview-revisit-detail");
    let mut harness = host.harness(&scratch, "revisit-detail");
    wait_until_unsettled(&mut harness, "the rows", |harness| has(harness, "Change 5"));
    open(&mut harness, 1);
    wait_until_unsettled(&mut harness, "the detail", |harness| {
        has(harness, "Change 1") && has(harness, "Discussion")
    });
    // The live-update baseline probe follows each load and reads the merge request once more.
    pump(&mut harness, 2.0);
    let detail = format!("/api/v4/projects/{}/merge_requests/1", urlencoding::encode(PROJECT));
    let after_first = host.count(&detail);

    // Three other pull requests push the first one out of the open pages.
    for number in 2..=4 {
        open(&mut harness, number);
        wait_until_unsettled(&mut harness, "another detail", |harness| {
            has(harness, &format!("Change {number}"))
        });
    }
    pump(&mut harness, REVISIT_FRESH.as_secs_f64() + 5.0);
    host.publish(5, Duration::from_millis(1500));
    open(&mut harness, 1);
    harness.step();
    assert!(has(&harness, "Change 1"), "the dropped page starts from the page cache: no skeleton");
    assert!(has(&harness, "Updating…"));
    wait_until_unsettled(&mut harness, "the refreshed detail", |harness| {
        has(harness, "Change 1 (edited)")
    });
    pump(&mut harness, 2.0);
    assert_eq!(
        host.count(&detail),
        after_first * 2,
        "one refresh: the same requests as the first load, once"
    );
}

#[test]
fn list_collapsed_group_is_remembered_and_loads_nothing_until_expanded() {
    let host = GitLabGroup::start(2);
    let scratch = ScratchDir::new("overview-collapse");
    let paths = host.paths(&scratch, "collapse");
    let mut harness = start_app(&paths);
    wait_until_unsettled(&mut harness, "the rows", |harness| has(harness, "Change 2"));

    harness.click_at(harness.get_by_label("Mock GitLab, 2 pull requests, expanded").rect());
    pump(&mut harness, 1.0);
    assert!(
        has(&harness, "Mock GitLab, 2 pull requests, collapsed"),
        "a collapsed header keeps its count"
    );
    assert!(!has(&harness, "Change 2"), "the rows are gone once closed");
    let saved = saved_settings(&harness);
    assert!(saved.collapsed_connections.contains("collapse"), "the choice is saved at once");
    drop(harness);

    // A restart keeps the group closed and asks the host nothing for it; the saved answer counts.
    let listings = host.count(LISTING);
    let mut harness = start_app(&paths);
    wait_until(
        &mut harness,
        "the group closed after the restart, counted from the saved answer",
        |harness| has(harness, "Mock GitLab, 2 pull requests, collapsed"),
    );
    settle_background(&mut harness);
    assert_eq!(host.count(LISTING), listings, "a collapsed group loads nothing");
    harness.click_at(harness.get_by_label("Refresh (F5)").rect());
    pump(&mut harness, 2.0);
    settle_background(&mut harness);
    assert_eq!(host.count(LISTING), listings, "Refresh skips collapsed groups too");

    // Enter on the focused header expands it, which loads it.
    harness.get_by_label("Mock GitLab, 2 pull requests, collapsed").focus();
    harness.key_press(egui::Key::Enter);
    wait_until_unsettled(&mut harness, "the rows after expanding", |harness| {
        has(harness, "Change 2")
    });
    wait_until_unsettled(&mut harness, "the refresh after expanding", |_| {
        host.count(LISTING) == listings + 1
    });
    assert!(!saved_settings(&harness).collapsed_connections.contains("collapse"));
}

/// The list groups its rows under a header per repository by default. The header closes its
/// section, the books tool in the page's header turns the grouping off, and both are saved at once.
#[test]
fn list_groups_rows_by_repository_and_saves_the_choices() {
    let host = GitLabGroup::start(2);
    let scratch = ScratchDir::new("overview-sections");
    let paths = host.paths(&scratch, "sections");
    let mut harness = start_app(&paths);
    wait_until_unsettled(&mut harness, "the rows", |harness| has(harness, "Change 2"));
    let section = |state: &str| format!("{PROJECT}, 2 pull requests, {state}");

    press(&mut harness, &section("expanded"));
    assert!(has(&harness, &section("collapsed")), "a closed section keeps its header and count");
    assert!(!has(&harness, "Change 2"), "its rows are gone");
    let closed = |settings: &AppSettings| {
        settings.repository_sections.is_collapsed(
            review_store::SectionCard::Connection,
            "sections",
            PROJECT,
        )
    };
    assert!(closed(&saved_settings(&harness)), "the closed section is saved at once");
    press(&mut harness, &section("collapsed"));
    assert!(has(&harness, "Change 2") && !closed(&saved_settings(&harness)), "and opens again");

    press(&mut harness, GROUP_BY_REPOSITORY);
    assert!(!has(&harness, &section("expanded")), "without grouping no section headers");
    assert!(has(&harness, "Change 2"), "the rows stay");
    assert!(
        !saved_settings(&harness).repository_sections.is_grouped,
        "the choice is saved at once"
    );
    press(&mut harness, GROUP_BY_REPOSITORY);
    assert!(
        has(&harness, &section("expanded"))
            && saved_settings(&harness).repository_sections.is_grouped
    );
}

/// The overview's side column keeps the same right edge whether the page overflows (scroll bar
/// shown) or not, with the scroll gutter between the cards and the window edge, and card actions
/// sit in the title lines.
#[test]
fn overview_tab_side_column_keeps_its_right_edge_with_and_without_scroll_bar() {
    let host = GitLabGroup::start(4);
    let scratch = ScratchDir::new("overview-side-edge");
    let paths = host.paths(&scratch, "side-edge");
    let mut edges = Vec::new();
    for height in [1000.0, 520.0] {
        let mut harness = start_app_sized(&paths, height);
        open_side_column(&mut harness, "side-edge");
        let add = harness.get_by_label("Add reviewer").rect();
        let edit = harness.get_by_label("Edit labels").rect();
        assert!((add.right() - edit.right()).abs() < 0.5, "card actions line up: {add:?} {edit:?}");
        edges.push(add.right());
    }
    assert!(
        (edges[0] - edges[1]).abs() < 0.5,
        "no shift when the page starts to scroll: {edges:?}"
    );
    // Card padding 16 + page padding 16 + scroll gutter, from the window's right edge.
    let gap = 1440.0 - edges[0];
    assert!(
        gap >= 32.0 + cv_base::theme::style::SCROLL_GUTTER - 0.5,
        "the cards leave the scroll gutter free: {gap}"
    );
}

/// Label editing works on a draft: Cancel and Esc drop it without a request, Apply sends it only
/// when it changed something.
#[test]
fn overview_tab_label_editing_cancels_by_button_and_esc() {
    let host = GitLabGroup::start(4);
    let scratch = ScratchDir::new("overview-label-cancel");
    let paths = host.paths(&scratch, "label-cancel");
    let mut harness = start_app(&paths);
    open_side_column(&mut harness, "label-cancel");
    let detail = format!("/api/v4/projects/{}/merge_requests/4", urlencoding::encode(PROJECT));
    let before = host.count_writes(&detail);

    // Cancel button: an added label goes away again, nothing is sent.
    press(&mut harness, "Edit labels");
    wait_until_unsettled(&mut harness, "the label catalog", |harness| has(harness, "skip-deploy"));
    press(&mut harness, "skip-deploy");
    assert!(
        has(&harness, "Apply") && has(&harness, "Cancel (Esc)"),
        "editing offers Apply and Cancel"
    );
    press(&mut harness, "Cancel (Esc)");
    assert!(!has(&harness, "skip-deploy") && has(&harness, "backend"), "the labels are as before");
    assert!(has(&harness, "Edit labels"), "editing ended");

    // Esc: a removed label comes back, nothing is sent.
    press(&mut harness, "Edit labels");
    press(&mut harness, "backend");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(4);
    assert!(has(&harness, "Edit labels") && !has(&harness, "skip-deploy"), "Esc ends editing");
    assert!(has(&harness, "backend"), "Esc restores the removed label");
    settle_background(&mut harness);
    assert_eq!(host.count_writes(&detail), before, "Esc sends nothing");

    // Apply without a change sends nothing either.
    press(&mut harness, "Edit labels");
    press(&mut harness, "Apply");
    settle_background(&mut harness);
    assert_eq!(host.count_writes(&detail), before, "no request for cancelled or unchanged edits");

    // Apply with a change sends it.
    press(&mut harness, "Edit labels");
    press(&mut harness, "needs-docs");
    press(&mut harness, "Apply");
    wait_until_unsettled(&mut harness, "the label update", |_| host.count_writes(&detail) > before);
}

#[test]
fn list_files_button_opens_the_files_tab_and_not_the_row() {
    let host = GitLabGroup::start(3);
    let scratch = ScratchDir::new("overview-files-button");
    let mut harness = host.harness(&scratch, "files-button");
    wait_until_unsettled(&mut harness, "the rows", |harness| has(harness, "Change 3"));
    let connection_id = harness.state().services.connections.all()[0].id.clone();
    let key = |number: u64| PullRequestKey {
        connection_id: connection_id.clone(),
        repository_key: PROJECT.to_owned(),
        number,
    };

    // The button sits on top of the row: its click opens Files, the row's own click never fires.
    harness
        .click_at(harness.get_by_role_and_label(Role::Button, "Open Files (changes) of !1").rect());
    harness.step();
    assert_eq!(harness.state().route, Route::PullRequest(key(1)));
    assert_eq!(
        harness.state().history.back_routes(),
        [&Route::MyPullRequests],
        "exactly one navigation"
    );
    wait_until_unsettled(&mut harness, "the pull request", |harness| {
        harness.query_by_role_and_label(Role::Tab, "Overview").is_some()
    });
    assert_eq!(selected_tab(&harness), "Files");

    // Back is the list again; the row itself still opens the default tab.
    harness.state_mut().navigator.back();
    pump(&mut harness, 0.5);
    assert_eq!(harness.state().route, Route::MyPullRequests);
    harness.click_at(harness.get_by_role_and_label(Role::Link, "2 Change 2").rect());
    wait_until_unsettled(&mut harness, "the second pull request", |harness| {
        harness.query_by_role_and_label(Role::Tab, "Overview").is_some()
    });
    assert_eq!(harness.state().route, Route::PullRequest(key(2)));
    assert_eq!(selected_tab(&harness), "Overview");

    // Shift+Enter on a focused row opens Files too.
    harness.state_mut().navigator.back();
    pump(&mut harness, 0.5);
    harness
        .get_by_role_and_label(
            Role::Link,
            "3 Change 3: move the settings dialog into a page with sections",
        )
        .focus();
    harness.step();
    harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
    harness.step();
    assert_eq!(harness.state().route, Route::PullRequest(key(3)));
    wait_until_unsettled(&mut harness, "the third pull request", |harness| {
        harness.query_by_role_and_label(Role::Tab, "Overview").is_some()
    });
    assert_eq!(selected_tab(&harness), "Files");
}

/// The header's controls, left to right, as the accessibility tree names them.
const HEADER_CONTROLS: [&str; 6] = [
    "Pull requests",
    "All",
    "State: Active",
    "Repository: All repositories",
    SEARCH_TOOLTIP,
    "Refresh (F5)",
];

fn header_rects(harness: &Harness<'_, ReviewApp>) -> Vec<egui::Rect> {
    let roles = [
        Role::Label,
        Role::RadioButton,
        Role::ComboBox,
        Role::ComboBox,
        Role::Button,
        Role::Button,
    ];
    roles
        .iter()
        .zip(HEADER_CONTROLS)
        .map(|(role, label)| harness.get_by_role_and_label(*role, label).rect())
        .collect()
}

fn assert_apart(rects: &[egui::Rect], width: f32) {
    for (index, rect) in rects.iter().enumerate() {
        assert!(
            rect.left() >= 0.0 && rect.right() <= width,
            "{} stays inside the window at {width}: {rect:?}",
            HEADER_CONTROLS[index]
        );
        for (other, next) in rects.iter().enumerate().skip(index + 1) {
            assert!(
                !rect.shrink(0.5).intersects(next.shrink(0.5)),
                "{} and {} overlap at {width}: {rect:?} {next:?}",
                HEADER_CONTROLS[index],
                HEADER_CONTROLS[other]
            );
        }
    }
}

/// Title, role, state and repository pickers, search and refresh share one row from wide windows
/// down to the narrowest the app allows; only far narrower (a large UI scale) moves the filters
/// to a second row. Nothing overlaps, and the list below stays put while the header's note changes.
#[test]
fn list_header_is_one_row_at_wide_and_narrow_widths() {
    let host = GitLabGroup::start(4);
    let scratch = ScratchDir::new("overview-header-row");
    let paths = host.paths(&scratch, "header-row");
    let mut harness = start_app(&paths);
    // Whether the row fits at 960 is the fonts' width; the bundled ones are the same everywhere.
    cv_testkit::use_bundled_fonts(&harness.ctx);
    wait_until_unsettled(&mut harness, "the rows", |harness| has(harness, "Change 4"));

    for width in [1440.0, 960.0] {
        harness.set_size(egui::vec2(width, 900.0));
        pump(&mut harness, 0.5);
        let rects = header_rects(&harness);
        let row = rects[0].center().y;
        for (label, rect) in HEADER_CONTROLS.iter().zip(&rects) {
            assert!(
                (rect.center().y - row).abs() < 1.0,
                "{label} sits in the one header row at {width}: {rect:?}"
            );
        }
        for pair in rects.windows(2) {
            assert!(pair[0].right() <= pair[1].left() + 0.5, "left to right at {width}: {pair:?}");
        }
        assert_apart(&rects, width);
    }

    // Narrower than the window allows (the UI scaled up): the filters take a second row.
    harness.set_size(egui::vec2(700.0, 900.0));
    pump(&mut harness, 0.5);
    let rects = header_rects(&harness);
    assert!(rects[1].top() > rects[0].bottom(), "the filters wrap below the title: {rects:?}");
    assert!(
        (rects[4].center().y - rects[0].center().y).abs() < 1.0,
        "search and refresh stay beside the title"
    );
    assert_apart(&rects, 700.0);

    // A refresh shows its note in the header without moving the list.
    harness.set_size(egui::vec2(1440.0, 900.0));
    pump(&mut harness, 0.5);
    let card = || "Mock GitLab, 4 pull requests, expanded";
    let before = harness.get_by_label(card()).rect();
    host.publish(4, Duration::from_millis(1500));
    harness.click_at(harness.get_by_label("Refresh (F5)").rect());
    harness.run_steps(3);
    assert!(has(&harness, "Updating…"));
    assert_eq!(
        harness.get_by_label(card()).rect(),
        before,
        "the list does not move while updating"
    );
    wait_until_unsettled(&mut harness, "the refresh to end", |harness| !has(harness, "Updating…"));

    // A repository filter that hides every row offers the way back, not a dead end.
    harness.state_mut().services.settings.list_filters.repository =
        Some(("header-row".to_owned(), "team/other".to_owned()));
    pump(&mut harness, 0.5);
    assert!(has(&harness, "No pull requests in the chosen repository."));
    press(&mut harness, "Clear filters");
    wait_until_unsettled(&mut harness, "the rows to come back", |harness| has(harness, "Change 4"));
    assert_eq!(harness.state().services.settings.list_filters.repository, None);
}

/// Ctrl+K finds loaded pull requests by label and by author (what the list's own filter did), and
/// the header's search button opens the same search.
#[test]
fn list_search_finds_pull_requests_by_label_and_author() {
    let host = GitLabGroup::start(4);
    let scratch = ScratchDir::new("overview-search-label");
    let mut harness = host.harness(&scratch, "search-label");
    wait_until_unsettled(&mut harness, "the rows", |harness| has(harness, "Change 4"));

    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    harness.event(egui::Event::Text("backend".to_owned()));
    pump(&mut harness, 0.5);
    assert!(has(&harness, "!4 Change 4"), "the label finds its pull request");
    assert!(!has(&harness, "!3 Change 3"), "only that one");
    harness.key_press(egui::Key::Escape);
    pump(&mut harness, 0.5);
    assert!(!has(&harness, "!4 Change 4"), "Esc closes the results");

    press(&mut harness, SEARCH_TOOLTIP);
    harness.event(egui::Event::Text("grace".to_owned()));
    let found = |harness: &Harness<'_, ReviewApp>| {
        (1..=4).all(|number| has(harness, &format!("!{number} Change {number}")))
    };
    wait_until_unsettled(&mut harness, "the author's pull requests", found);
    harness.key_press(egui::Key::Enter);
    pump(&mut harness, 0.5);
    assert!(
        matches!(harness.state().route, Route::PullRequest(PullRequestKey { number: 4, .. })),
        "Enter opens the first match"
    );

    // Text that matches nothing loaded still leads somewhere.
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    harness.event(egui::Event::Text("no such thing".to_owned()));
    pump(&mut harness, 0.5);
    assert!(
        has(&harness, "Show all pull requests"),
        "a search without matches offers the full list"
    );
    harness.key_press(egui::Key::Enter);
    pump(&mut harness, 0.5);
    assert_eq!(harness.state().route, Route::MyPullRequests);
}

fn open_side_column(harness: &mut Harness<'_, ReviewApp>, connection_id: &str) {
    wait_until_unsettled(harness, "the rows", |harness| has(harness, "Change 4"));
    let key = PullRequestKey {
        connection_id: connection_id.to_owned(),
        repository_key: PROJECT.to_owned(),
        number: 4,
    };
    go(harness, Route::PullRequest(key));
    wait_until_unsettled(harness, "the side column", |harness| {
        has(harness, "backend") && has(harness, "Barbara Liskov")
    });
    pump(harness, 1.0);
}

/// The label of the pull request tab shown, without its count.
fn selected_tab(harness: &Harness<'_, ReviewApp>) -> String {
    let tab = harness
        .query_all_by_role(Role::Tab)
        .find(|node| node.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True))
        .expect("a selected tab");
    let label = tab.accesskit_node().label().unwrap_or_default();
    label.split(" (").next().unwrap_or_default().to_owned()
}

/// A GitLab group `team` with merge requests 1..=count in `team/app`, whose answers can change.
struct GitLabGroup {
    host: MockHost,
    state: Arc<HostState>,
}

#[derive(Default)]
struct HostState {
    /// Every request's method and path.
    hits: Mutex<Vec<(String, String)>>,
    count: AtomicUsize,
    edited: AtomicUsize,
    delay_ms: AtomicUsize,
}

impl GitLabGroup {
    fn start(count: usize) -> Self {
        let state = Arc::new(HostState::default());
        state.count.store(count, Ordering::SeqCst);
        let host = MockHost::start(|base_url| Responder {
            state: state.clone(),
            base_url: base_url.to_owned(),
        });
        Self { host, state }
    }

    fn connection(&self, id: &str, name: &str) -> Connection {
        Connection {
            name: name.to_owned(),
            organizations: vec![GROUP.to_owned()],
            token: Some(TOKEN.to_owned()),
            ..samples::connection(id, ProviderKind::GitLab, &self.host.uri())
        }
    }

    /// `scratch`'s data folders with one connection to the host, `id`.
    fn paths(&self, scratch: &ScratchDir, id: &str) -> AppPaths {
        saved_paths(scratch, &[self.connection(id, "Mock GitLab")])
    }

    /// The app on [`Self::paths`].
    fn harness(&self, scratch: &ScratchDir, id: &str) -> Harness<'static, ReviewApp> {
        start_app(&self.paths(scratch, id))
    }

    /// From now on the host lists `count` merge requests, titles edited, answering after `delay`.
    fn publish(&self, count: usize, delay: Duration) {
        self.state.count.store(count, Ordering::SeqCst);
        self.state.edited.store(1, Ordering::SeqCst);
        self.state.delay_ms.store(delay.as_millis() as usize, Ordering::SeqCst);
    }

    /// The requests to `request_path`, reads and writes.
    fn count(&self, request_path: &str) -> usize {
        self.count_where(|_, path| path == request_path)
    }

    /// The writes to `request_path`: what the app changed there, not what it read (the live probe
    /// reads a merge request on its own).
    fn count_writes(&self, request_path: &str) -> usize {
        self.count_where(|method, path| method != "GET" && path == request_path)
    }

    fn count_where(&self, is_counted: impl Fn(&str, &str) -> bool) -> usize {
        let hits = self.state.hits.lock().unwrap_or_else(PoisonError::into_inner);
        hits.iter().filter(|(method, path)| is_counted(method, path)).count()
    }
}

struct Responder {
    state: Arc<HostState>,
    base_url: String,
}

impl Respond for Responder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path().to_owned();
        let hit = (request.method.to_string(), path.clone());
        self.state.hits.lock().unwrap_or_else(PoisonError::into_inner).push(hit);
        let count = self.state.count.load(Ordering::SeqCst) as u64;
        let edited = self.state.edited.load(Ordering::SeqCst) == 1;
        let delay = Duration::from_millis(self.state.delay_ms.load(Ordering::SeqCst) as u64);
        let project = format!("/api/v4/projects/{}", urlencoding::encode(PROJECT));
        let body = if path == LISTING {
            Value::Array((1..=count).rev().map(|iid| self.merge_request(iid, false)).collect())
        } else if let Some((iid, tail)) = merge_request_route(&path, PROJECT) {
            match tail {
                "" => self.merge_request(iid, edited),
                "approvals" => json!({ "approved_by": [] }),
                _ => json!([]),
            }
        } else if path == format!("{project}/labels") {
            json!([
                { "id": 1, "name": "backend", "color": "#1f75cb" },
                { "id": 2, "name": "skip-deploy", "color": "#dc143c" },
                { "id": 3, "name": "needs-docs", "color": "#ed9121" },
            ])
        } else if path.ends_with("/statuses") {
            let status = |id: u64, name: &str| {
                json!({ "id": id, "name": name, "status": "success",
                    "started_at": "2026-10-17T10:00:00Z", "finished_at": "2026-10-17T10:03:12Z" })
            };
            json!([
                status(1, "build"),
                status(2, "integration tests on every supported database and platform"),
                status(3, "lint")
            ])
        } else {
            return ResponseTemplate::new(404).set_body_json(json!({ "message": "404 Not Found" }));
        };
        ResponseTemplate::new(200).set_body_json(body).set_delay(delay)
    }
}

impl Responder {
    fn merge_request(&self, iid: u64, edited: bool) -> Value {
        const NAMES: [&str; 3] = ["Linus Torvalds", "Ada Lovelace", "Barbara Liskov"];
        let title = match (iid, edited) {
            (1, true) => "Change 1 (edited)".to_owned(),
            (3, _) => format!("Change {iid}: move the settings dialog into a page with sections"),
            _ => format!("Change {iid}"),
        };
        let reviewers: Vec<Value> = (0..iid.min(3))
            .map(|index| {
                let name = NAMES[index as usize];
                json!({ "id": 10 + index, "username": format!("r{index}"), "name": name })
            })
            .collect();
        let mut value =
            merge_request(&self.base_url, PROJECT, iid, &"a".repeat(40), &"b".repeat(40));
        value["title"] = json!(title);
        value["description"] = json!("Measures requests.");
        value["draft"] = json!(iid == 2);
        value["created_at"] = json!(format!("2026-10-{iid:02}T10:00:00Z"));
        value["labels"] =
            if iid == 4 { json!([{ "name": "backend", "color": "#1f75cb" }]) } else { json!([]) };
        value["reviewers"] = json!(reviewers);
        value
    }
}

fn start_app(paths: &AppPaths) -> Harness<'static, ReviewApp> {
    start_app_sized(paths, 900.0)
}

fn start_app_sized(paths: &AppPaths, height: f32) -> Harness<'static, ReviewApp> {
    let builder = Harness::builder().with_size([1440.0, height]).with_step_dt(0.25);
    app_with(builder, paths.clone(), cv_providers::create_provider)
}

fn open(harness: &mut Harness<'_, ReviewApp>, number: u64) {
    let connection_id = harness.state().services.connections.all()[0].id.clone();
    let key = PullRequestKey { connection_id, repository_key: PROJECT.to_owned(), number };
    go(harness, Route::PullRequest(key));
}

fn go(harness: &mut Harness<'_, ReviewApp>, route: Route) {
    harness.state_mut().navigator.go(route);
    harness.step();
}
