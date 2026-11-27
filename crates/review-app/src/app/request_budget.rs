//! Request budget: drives the real app headless (`egui_kittest`) against a mock GitLab that counts
//! every request by method and path template, and a local bare git repository as the mirror's
//! remote. Time is simulated through egui's input time, so minutes pass in seconds.
//!
//! `cargo test -p review-app --bin CodeView app::request_budget -- --nocapture --test-threads=1`
//! prints a report per screen and endpoint. No network, no real token: the token below only ever
//! reaches the mock.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, HarnessBuilder};
use review_core::{Connection, ProviderKind};
use review_store::{AppPaths, AppSettings, ConnectionStore};
use serde_json::{Value, json};
use wiremock::{Request, Respond, ResponseTemplate};

use super::ReviewApp;
use super::mock_host::MockHost;
use super::recording::{Recorder, iids, template};
use super::startup;
use super::walkthrough::{
    self, LIMIT, click_first, has, pump, saved_paths, set_focus, wait_idle, wait_until_unsettled,
    wheel,
};
use cv_lists::pull_request_list::LIST_INTERVAL;
use cv_runtime::cache::page_cache::REVISIT_FRESH;
use cv_runtime::route::{PullRequestKey, Route};
use cv_shell::attention::{Attention, PAUSE};
use cv_testkit::{Clicks, ScratchDir, fast_import, file_url, git_in, samples};

const MOCK_TOKEN: &str = "glpat-mock-only";
const PROJECT: &str = "group/app";
const PROJECT_ID: u64 = 42;
const IID: u64 = 7;
const PIPELINE_ID: u64 = 1001;
/// The first of the older pipelines, numbered upwards from here.
const OLDER_PIPELINE_ID: u64 = 901;
const STEP_SECONDS: f32 = 0.5;

#[test]
fn browsing_300_files_costs_only_the_live_probe() {
    run_scenario(300);
}

#[test]
fn browsing_3000_files_fetches_blobs_in_chunks() {
    run_scenario(3000);
}

/// "First file" opens the Files tab on a file's diff, read from the local mirror like the folder
/// view's: no request beyond the live-update probe and no git fetch.
#[test]
fn files_tab_opening_on_the_first_file() {
    let fixture = Fixture::new("first-file", 300);
    let mut harness = fixture.harness();
    harness.state_mut().services.settings.files_start = review_store::FilesStart::FirstFile;
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key()));
    wait_until_unsettled(&mut harness, "the overview", |harness| has(harness, "Budget scenario"));
    wait_idle(&mut harness);
    let files_label = format!("Files ({})", cv_widgets::numbers::group_digits(300));
    wait_until_unsettled(&mut harness, "the file list", |harness| {
        harness.query_by_label(&files_label).is_some()
    });
    wait_idle(&mut harness);
    let fetches = fixture.git_fetches();
    // The probe goes out 30 s after the last one, so a window that opens right after a probe holds
    // exactly the next one, wherever the page load left the schedule.
    fixture.recorder.enter("Until a probe");
    wait_until_unsettled(&mut harness, "a live-update probe", |_| {
        fixture.recorder.endpoints("Until a probe").contains_key(PROBE_APPROVALS)
    });
    wait_background_idle_without_frames(&harness);
    fixture.recorder.enter("Files, first file (30 s)");
    let started = harness_time(&harness);
    harness.click_at(harness.get_by_label(&files_label).rect());
    wait_in_real_time(&mut harness, "the first file's diff", |harness| {
        assert!(!has(harness, "300 changed files"), "the folder view showed before the first file");
        has(harness, "Diff of ")
    });
    let shown_after = harness_time(&harness) - started;
    assert!(shown_after < 30.0, "the first file's diff took {shown_after} s of the 30 s window");
    pump(&mut harness, 30.0 - shown_after);
    wait_background_idle_without_frames(&harness);
    let hits = fixture.recorder.endpoints("Files, first file (30 s)");
    assert_eq!(
        hits.get(PROBE_APPROVALS).copied(),
        Some(1),
        "one live-update probe per 30 s: {hits:?}"
    );
    assert_eq!(
        fixture.recorder.count("Files, first file (30 s)"),
        3,
        "the diff comes from the git mirror; only the probe asks the API: {hits:?}"
    );
    assert_eq!(fixture.git_fetches(), fetches, "no git fetch for the first file");
}

/// Threads of the merge request; every second one sits on a line of a changed file.
const THREADS: usize = 12;

/// Each live-update probe asks for the approvals once (with the merge request and its reviewers).
const PROBE_APPROVALS: &str = "GET /api/v4/projects/:project/merge_requests/:id/approvals";

/// The coordinator's focus rules: nothing while inactive, one catch-up per due item, user actions pass.
#[test]
fn unfocused_window_is_silent_and_catches_up_once() {
    let fixture = Fixture::new("unfocused", 30);
    let mut harness = fixture.harness();
    open_pipelines(&mut harness, &fixture);

    // A full live interval passes first, so the pipeline refresh is due while the window is away.
    set_focus(&mut harness, false);
    fixture.recorder.enter("unfocused");
    pump(&mut harness, 300.0);
    wait_background_idle_without_frames(&harness);
    assert_eq!(fixture.recorder.count("unfocused"), 0, "an unfocused window sends nothing");

    fixture.recorder.enter("refocused");
    set_focus(&mut harness, true);
    pump(&mut harness, 2.0);
    wait_background_idle_without_frames(&harness);
    let hits = fixture.recorder.endpoints("refocused");
    let pipelines = "GET /api/v4/projects/:project/merge_requests/:id/pipelines".to_owned();
    let jobs = "GET /api/v4/projects/:id/pipelines/:id/jobs".to_owned();
    assert_eq!(
        hits.get(&pipelines).copied(),
        Some(1),
        "one catch-up refresh of the runs: {hits:?}"
    );
    assert_eq!(
        hits.get(&jobs).copied(),
        Some(1),
        "one catch-up refresh of the running run's jobs: {hits:?}"
    );

    // User actions are never paused: a Refresh clicked just before the window lost focus still goes
    // out, although its request starts in an unfocused frame.
    harness.state_mut().navigator.go(Route::MyPullRequests);
    pump(&mut harness, 1.0);
    // The revisit refreshes the list in the background; a Refresh while it runs joins it.
    wait_idle(&mut harness);
    fixture.recorder.enter("user refresh");
    harness.click_at(harness.get_by_label("Refresh (F5)").rect());
    harness.step();
    set_focus(&mut harness, false);
    pump(&mut harness, 1.0);
    wait_background_idle_without_frames(&harness);
    let hits = fixture.recorder.endpoints("user refresh");
    assert_eq!(hits.get("GET /api/v4/groups/group/merge_requests").copied(), Some(1), "{hits:?}");
}

/// Coming back after a pause on a merge request page: its probe goes out at once, and the overview,
/// where the user likely goes next, refreshes in the background, so going there asks nothing more.
#[test]
fn a_return_after_a_pause_refreshes_ahead() {
    let listing = "GET /api/v4/groups/group/merge_requests";
    let fixture = Fixture::new("return", 30);
    let mut harness = fixture.harness();
    harness.state_mut().attention = Attention::default();
    wait_until_unsettled(&mut harness, "the pull request list", |harness| {
        has(harness, "Change 27")
    });
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key()));
    wait_until_unsettled(&mut harness, "the overview", |harness| has(harness, "Budget scenario"));
    wait_idle(&mut harness);
    // No input for longer than a pause; the probe goes on meanwhile, the list asks nothing.
    pump(&mut harness, PAUSE.as_secs_f64() + 5.0);
    wait_background_idle_without_frames(&harness);

    fixture.recorder.enter("returned");
    harness.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(400.0, 300.0)));
    pump(&mut harness, 1.0);
    wait_background_idle_without_frames(&harness);
    let hits = fixture.recorder.endpoints("returned");
    assert_eq!(hits.get(PROBE_APPROVALS).copied(), Some(1), "the probe at once: {hits:?}");
    assert_eq!(hits.get(listing).copied(), Some(1), "the overview ahead: {hits:?}");

    fixture.recorder.enter("back to the list");
    harness.state_mut().navigator.go(Route::MyPullRequests);
    pump(&mut harness, 1.0);
    wait_background_idle_without_frames(&harness);
    let hits = fixture.recorder.endpoints("back to the list");
    assert_eq!(hits.get(listing), None, "the warmed rows are current: {hits:?}");
}

/// A 429 shows a calm banner with a countdown; "Retry now" sends the queued request at once.
#[test]
fn rate_limit_banner_retries_now() {
    let fixture = Fixture::new("banner", 30);
    fixture.limit_once("/api/v4/groups/group/merge_requests", 120);
    fixture.recorder.enter("limited");
    let mut harness = fixture.harness();
    wait_until_unsettled(&mut harness, "the rate-limit banner", |harness| {
        has(harness, "GitLab is rate-limiting this connection — retrying in")
    });
    assert!(has(&harness, "Open GitLab rate limit docs"));
    assert!(!has(&harness, "Change 27"), "the list waits for the host");

    harness.click_at(harness.get_by_label("Retry now").rect());
    wait_until_unsettled(&mut harness, "the list after Retry now", |harness| {
        has(harness, "Change 27")
    });
    pump(&mut harness, 1.0);
    assert!(!has(&harness, "is rate-limiting"), "the banner is gone once the host answers");
    let limited = fixture.recorder.endpoints("limited");
    assert_eq!(
        limited.get("GET /api/v4/groups/group/merge_requests").copied(),
        Some(1),
        "exactly one retry reached the host after the 429: {limited:?}"
    );

    // The request counter in Settings shows what this connection sent, rate limits included.
    harness.state_mut().navigator.go(Route::Settings);
    pump(&mut harness, 1.0);
    harness.click_at(harness.get_by_role_and_label(egui::Role::Tab, "Network activity").rect());
    pump(&mut harness, 1.0);
    // The listing twice (429, then the retry) and the GraphQL requests for its line counts and
    // its rows' checks.
    assert!(has(
        &harness,
        "4 per minute now, 4 sent, 0 unchanged (304), 0 shared with an identical request, 1 rate-limited"
    ));
    assert!(
        has(&harness, "GET /api/v4/groups/group/merge_requests")
            && has(&harness, "POST /api/graphql")
    );
}

/// gitlab.com limits anonymous GraphQL per IP far below REST and answers 429 with a `Retry-After`
/// up to the next full hour. That pause once covered the whole host, so the Pipelines tab (whose
/// stage loads ask GraphQL for the jobs' needs) stayed a skeleton until the hour was over. Now the
/// jobs come at once without their needs, and the banner names GraphQL with a way to a token.
#[test]
fn graphql_limit_does_not_hold_back_the_pipelines() {
    let fixture = Fixture::with_token("graphql-limit", 30, None);
    fixture.limit_once("/api/graphql", 2391);
    let mut harness = fixture.harness();
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key()));
    wait_until_unsettled(&mut harness, "the overview", |harness| has(harness, "Budget scenario"));
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_until_unsettled(&mut harness, "the jobs despite the GraphQL limit", |harness| {
        has(harness, "test-3-1")
    });
    wait_until_unsettled(&mut harness, "the GraphQL rate-limit banner", |harness| {
        has(
            harness,
            "GitLab is rate-limiting this connection's GraphQL requests — retrying in 40 min",
        )
    });
    assert!(has(&harness, "everything else loads as usual"));
    assert!(has(&harness, "Add a token"), "an anonymous connection is offered a token");
    harness.click_at(harness.get_by_label("Retry now").rect());
    wait_until_unsettled(&mut harness, "the banner gone after Retry now", |harness| {
        !has(harness, "is rate-limiting")
    });
    assert!(has(&harness, "test-3-1"), "the jobs stay on screen");
}

/// A GraphQL pause holds the list rows' checks back for as long as it lasts (gitlab.com's
/// anonymous one up to the full hour): the rows say they wait for GitLab, quietly, and the app
/// rests meanwhile. Before, their updating edge asked for every frame for the whole pause (83353
/// frames in 90 s in a walkthrough), and the banner's countdown woke the app every second although
/// its text changes once a minute.
#[test]
fn rows_held_by_a_rate_limit_let_the_app_rest() {
    let fixture = Fixture::new("held-rows", 30);
    fixture.limit_row_statuses_once(2391);
    let mut harness = fixture.loading_harness();
    wait_until_unsettled(&mut harness, "the GraphQL rate-limit banner", |harness| {
        has(harness, "rate-limiting this connection's GraphQL requests — retrying in 40 min")
    });
    wait_until_unsettled(&mut harness, "the pull request list", |harness| {
        has(harness, "Change 27")
    });
    // The banner and the rows fade in; then nothing changes until the countdown's next minute.
    pump(&mut harness, 5.0);
    let mut soon = Vec::new();
    for _ in 0..40 {
        harness.step();
        let delay = repaint_delay(&harness);
        if delay <= Duration::from_secs(1) {
            let causes: Vec<String> =
                harness.ctx.repaint_causes().iter().map(ToString::to_string).collect();
            soon.push(format!("{delay:?} for {causes:?}"));
        }
    }
    assert!(
        soon.is_empty(),
        "{} of 40 frames without input asked for the next within a second: {soon:?}",
        soon.len()
    );
    let held = "!27 waits for GitLab to accept requests again";
    assert!(has(&harness, held), "the rows say what they wait for");
    assert!(!has(&harness, "Updating the checks and approvals"), "nothing is under way");

    harness.click_at(harness.get_by_label("Retry now").rect());
    wait_until_unsettled(&mut harness, "the banner gone and the rows' checks in", |harness| {
        !has(harness, "is rate-limiting")
            && !has(harness, held)
            && !has(harness, "Updating the checks and approvals")
    });
}

/// The delay after which the last frame asked for the next one.
fn repaint_delay(harness: &Harness<'_, ReviewApp>) -> Duration {
    harness
        .output()
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map_or(Duration::MAX, |viewport| viewport.repaint_delay)
}

/// A click held while the rate-limit banner opens above the page still counts at its release:
/// the banner moves the page down but renames none of its widgets. Before, every widget under it
/// took another id in the frame it opened, egui dropped the press of a widget that was gone, and
/// the click on the Pipelines tab went to nobody (a busy macOS runner, GitHub run 45886166305).
#[test]
fn a_click_held_while_the_banner_opens_still_counts() {
    let fixture = Fixture::with_token("banner-click", 30, None);
    fixture.limit_once_after("/api/graphql", 2391, Duration::from_secs(2));
    let mut harness = fixture.harness();
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key()));
    wait_until_unsettled(&mut harness, "the overview", |harness| has(harness, "Budget scenario"));
    assert!(!has(&harness, "is rate-limiting"), "the limit is still on its way");
    // egui's clock stands still while the button is down, so the press stays a click.
    let time = harness_time(&harness);
    let at = harness.get_by_label("Pipelines").rect().center();
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    harness.event(button(true));
    let started = Instant::now();
    while !has(&harness, "is rate-limiting") {
        assert!(started.elapsed() < LIMIT, "the banner never opened");
        harness.input_mut().time = Some(time);
        harness.step();
        cv_testkit::let_threads_run(Duration::from_millis(10));
    }
    harness.event(button(false));
    harness.input_mut().time = Some(time);
    harness.step();
    harness.input_mut().time = None;
    wait_until_unsettled(&mut harness, "the Pipelines tab opened by the click", |harness| {
        let pipelines = harness.get_by_role_and_label(egui::Role::Tab, "Pipelines");
        pipelines.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
    });
}

/// gitlab.com gives an IP without a token 60 requests an hour; past that its CI endpoints answer
/// 429 until the full hour. The Pipelines tab then says what it waits for, with a token, Retry now
/// and the merge request on GitLab, instead of a skeleton that lasts up to an hour.
#[test]
fn rate_limited_jobs_show_a_waiting_state_with_ways_out() {
    let fixture = Fixture::with_token("jobs-limit", 30, None);
    fixture
        .limit_once(&format!("/api/v4/projects/{PROJECT_ID}/pipelines/{PIPELINE_ID}/jobs"), 2391);
    let mut harness = fixture.harness();
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key()));
    wait_until_unsettled(&mut harness, "the overview", |harness| has(harness, "Budget scenario"));
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_until_unsettled(&mut harness, "the waiting state", |harness| {
        has(harness, "Waiting for GitLab")
    });
    assert!(has(&harness, "The pipelines load on their own in 40 min; a token raises the limit."));
    assert!(has(&harness, "Add a token") && has(&harness, "Open on GitLab"), "the ways forward");
    assert!(!has(&harness, "test-3-1"));
    // The banner above offers Retry now as well; either ends the pause.
    harness.click_at(harness.query_all_by_label("Retry now").next().expect("Retry now").rect());
    wait_until_unsettled(&mut harness, "the jobs after Retry now", |harness| {
        has(harness, "test-3-1")
    });
    assert!(!has(&harness, "Waiting for GitLab"));
}

/// gitlab.com lists every pipeline of a merge request, and an IP without a token gets about 60 CI
/// and GraphQL requests an hour. The Pipelines tab of a merge request with 10 pipelines loads the
/// jobs of the head commit's pipeline only (its jobs, bridges and needs), not those of the nine
/// older ones, which are rows of the cheap pipeline list. An older pipeline loads its jobs when it
/// is opened, once: opening it again, or the tab again, costs nothing.
#[test]
fn pipelines_tab_loads_jobs_of_the_head_run_only() {
    let fixture = Fixture::with_older_pipelines("lazy-pipelines", 9);
    let mut harness = fixture.harness();
    fixture.recorder.enter("open the tab");
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key()));
    wait_until_unsettled(&mut harness, "the overview", |harness| has(harness, "Budget scenario"));
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_until_unsettled(&mut harness, "the head run's jobs", |harness| has(harness, "test-3-1"));
    // Less than the live refresh's first 15 s, so the running head run is asked for once.
    pump(&mut harness, 2.0);
    // The pipelines whose jobs were asked for, and how many GraphQL requests (the jobs' needs) went out.
    let job_requests = |phase: &str| {
        let paths = fixture.recorder.details(|hit, _| hit == phase);
        // Jobs and bridges load concurrently.
        let mut pipelines: Vec<String> = paths
            .iter()
            .filter_map(|path| {
                path.strip_prefix(&format!("/api/v4/projects/{PROJECT_ID}/pipelines/"))
            })
            .map(str::to_owned)
            .collect();
        pipelines.sort();
        (pipelines, paths.iter().filter(|path| *path == "/api/graphql").count())
    };
    let head = vec![format!("{PIPELINE_ID}/bridges"), format!("{PIPELINE_ID}/jobs")];
    let opened = fixture.recorder.endpoints("open the tab");
    assert_eq!(
        job_requests("open the tab"),
        (head.clone(), 2),
        "jobs, bridges and needs of the head run only, and the pipelines' names once although the runs loaded twice: {opened:?}"
    );
    assert!(
        has(&harness, "Budget pipeline, Running"),
        "the head pipeline carries the name GraphQL gave it"
    );
    // 10 pipelines and the head run's 40 jobs: the filter brings the oldest pipeline into view.
    filter_checks(&mut harness, &format!("#{OLDER_PIPELINE_ID} ·"));
    // Without a `workflow:name` a pipeline is named by its kind; its number is on the line below.
    let older = "Merge request pipeline, Succeeded, older run";
    assert!(
        harness.query_by_label(older).is_some(),
        "older pipelines are rows of the list, named as such"
    );
    filter_checks(&mut harness, "old-check");
    assert!(!has(&harness, "old-check-1"), "without their jobs");
    filter_checks(&mut harness, &format!("#{OLDER_PIPELINE_ID} ·"));

    fixture.recorder.enter("open an older run");
    harness.click_at(harness.get_by_label(older).rect());
    wait_until_unsettled(&mut harness, "the older run's stages", |harness| {
        harness.query_by_label("Stage check").is_some()
    });
    let (pipelines, needs) = job_requests("open an older run");
    let opened_run = [format!("{OLDER_PIPELINE_ID}/bridges"), format!("{OLDER_PIPELINE_ID}/jobs")];
    assert_eq!(
        pipelines.iter().filter(|path| !head.contains(path)).collect::<Vec<_>>(),
        opened_run.iter().collect::<Vec<_>>(),
        "the opened run loads its own jobs"
    );
    assert!(needs >= 1, "and its needs");

    fixture.recorder.enter("back and again");
    harness.click_at(harness.get_by_role_and_label(egui::Role::Link, "Pipelines").rect());
    wait_until_unsettled(&mut harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });
    filter_checks(&mut harness, "old-check");
    assert!(has(&harness, "old-check-1 in Merge request pipeline"), "its jobs are rows now");
    filter_checks(&mut harness, &format!("#{OLDER_PIPELINE_ID} ·"));
    harness.click_at(harness.get_by_label(older).rect());
    wait_until_unsettled(&mut harness, "the older run again", |harness| {
        harness.query_by_label("Stage check").is_some()
    });
    harness.click_at(harness.get_by_role_and_label(egui::Role::Link, "Pipelines").rect());
    wait_until_unsettled(&mut harness, "the checks table again", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });
    let (pipelines, _) = job_requests("back and again");
    assert!(
        pipelines.iter().all(|path| head.contains(path)),
        "a finished run's jobs stay loaded; only the running head run refreshes: {pipelines:?}"
    );
}

/// The Favorites card asks nothing while it is collapsed; expanding it (a user action) loads the
/// starred project once; a revisit's background refresh waits while the window is inactive and
/// goes out once when it is active again. Idle sends nothing.
#[test]
fn favorites_load_only_while_open_and_focused() {
    let fixture = Fixture::new("favorites", 30);
    let mut settings = AppSettings::load(&fixture.paths.settings_file()).expect("no settings yet");
    settings.favorites.toggle(&fixture.connection_id, PROJECT);
    settings.favorites.is_collapsed = true;
    settings.save(&fixture.paths.settings_file()).expect("save the favorite");
    let favorites = "GET /api/v4/projects/:project/merge_requests";
    let hits = |phase: &str| fixture.recorder.endpoints(phase).get(favorites).copied().unwrap_or(0);

    fixture.recorder.enter("collapsed");
    let mut harness = fixture.harness();
    wait_until_unsettled(&mut harness, "the pull request list", |harness| {
        has(harness, "Change 27")
    });
    pump(&mut harness, 30.0);
    assert!(has(&harness, "Favorites (1 repository), collapsed"));
    assert_eq!(hits("collapsed"), 0, "a collapsed card asks nothing");

    fixture.recorder.enter("expanded");
    harness.click_at(harness.get_by_label("Favorites (1 repository), collapsed").rect());
    wait_until_unsettled(&mut harness, "the favorite's rows", |harness| {
        has(harness, "Favorites (1 repository), 30 pull requests, expanded")
    });
    pump(&mut harness, 60.0);
    assert_eq!(hits("expanded"), 1, "one listing of the starred project, no polling");

    harness.state_mut().navigator.go(Route::Settings);
    pump(&mut harness, REVISIT_FRESH.as_secs_f64() + 5.0);
    set_focus(&mut harness, false);
    fixture.recorder.enter("unfocused revisit");
    harness.state_mut().navigator.go(Route::MyPullRequests);
    pump(&mut harness, 60.0);
    assert!(
        has(&harness, "Favorites (1 repository), 30 pull requests, expanded"),
        "the known rows stay"
    );
    assert_eq!(hits("unfocused revisit"), 0, "an inactive window holds the refresh back");

    fixture.recorder.enter("refocused");
    set_focus(&mut harness, true);
    pump(&mut harness, 60.0);
    assert_eq!(hits("refocused"), 1, "the owed refresh goes out once");
}

/// The overview's listings: the fixture's connection card's, and the Favorites card's of the
/// starred project.
const GROUP_LISTING: &str = "GET /api/v4/groups/group/merge_requests";
const PROJECT_LISTING: &str = "GET /api/v4/projects/:project/merge_requests";

/// Only the cards on screen load. The Favorites card's 30 rows fill the window, so the cards of
/// the three connections below it are off screen: Refresh, a revisit, a warm and the periodic
/// refresh ask the starred project only (before this was measured: the project and all three
/// connections on Refresh and on a warm, 4 each). A card that scrolls in loads then; Refresh down
/// there asks only the cards on screen (before: 4 again).
/// `--nocapture` prints the listings of each phase.
#[test]
fn the_overview_asks_only_for_cards_on_screen() {
    let fixture = Fixture::new("on-screen", 30);
    fixture.add_connection("2");
    fixture.add_connection("3");
    let mut settings = AppSettings::load(&fixture.paths.settings_file()).expect("no settings yet");
    settings.favorites.toggle(&fixture.connection_id, PROJECT);
    settings.save(&fixture.paths.settings_file()).expect("save the favorite");
    // (the starred project's listings, the connections' group listings)
    let hits = |phase: &str| {
        let endpoints = fixture.recorder.endpoints(phase);
        let groups = endpoints.iter().filter(|(name, _)| {
            name.starts_with("GET /api/v4/groups/") && name.ends_with("/merge_requests")
        });
        (
            endpoints.get(PROJECT_LISTING).copied().unwrap_or(0),
            groups.map(|(_, hits)| hits).sum::<usize>(),
        )
    };

    fixture.recorder.enter("opened");
    let mut harness = fixture.harness();
    wait_until_unsettled(&mut harness, "the Favorites card", |harness| {
        has(harness, "Favorites (1 repository), 30 pull requests, expanded")
    });
    wait_idle(&mut harness);
    wait_background_idle_without_frames(&harness);

    fixture.recorder.enter("refresh");
    harness.click_at(harness.get_by_label("Refresh (F5)").rect());
    pump(&mut harness, 2.0);
    wait_background_idle_without_frames(&harness);

    harness.state_mut().navigator.go(Route::Settings);
    pump(&mut harness, REVISIT_FRESH.as_secs_f64() + 5.0);
    fixture.recorder.enter("revisit");
    harness.state_mut().navigator.go(Route::MyPullRequests);
    pump(&mut harness, 5.0);
    wait_background_idle_without_frames(&harness);

    fixture.recorder.enter("periodic");
    pump(&mut harness, LIST_INTERVAL.as_secs_f64());
    wait_background_idle_without_frames(&harness);

    // A return on a merge request page warms the overview ahead.
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key()));
    wait_until_unsettled(&mut harness, "the merge request", |harness| {
        has(harness, "Budget scenario")
    });
    harness.state_mut().attention = Attention::default();
    pump(&mut harness, PAUSE.as_secs_f64() + 5.0);
    wait_background_idle_without_frames(&harness);
    fixture.recorder.enter("warm");
    harness.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(400.0, 300.0)));
    pump(&mut harness, 2.0);
    wait_background_idle_without_frames(&harness);
    harness.state_mut().attention = Attention::without_pauses();

    harness.state_mut().navigator.go(Route::MyPullRequests);
    pump(&mut harness, 2.0);
    fixture.recorder.enter("scrolled");
    scroll_list(&mut harness, 20_000.0);
    pump(&mut harness, 2.0);
    wait_background_idle_without_frames(&harness);

    fixture.recorder.enter("refresh below");
    harness.click_at(harness.get_by_label("Refresh (F5)").rect());
    pump(&mut harness, 2.0);
    wait_background_idle_without_frames(&harness);

    let phases = ["opened", "refresh", "revisit", "periodic", "warm", "scrolled", "refresh below"];
    let mut report = String::from("\n=== Overview listings (starred project, connections) ===\n");
    for phase in phases {
        let (lane, cards) = hits(phase);
        writeln!(report, "{phase}: {lane} + {cards}").unwrap();
    }
    println!("{report}");
    let (_, opened) = hits("opened");
    assert!(opened < 3, "the third card was below the skeletons from the start: {report}");
    assert_eq!(hits("refresh"), (1, 0), "Refresh asks the card on screen: {report}");
    assert_eq!(hits("revisit"), (1, 0), "a revisit refreshes the card on screen: {report}");
    assert_eq!(hits("periodic"), (1, 0), "the card on screen refreshes on its own: {report}");
    assert_eq!(hits("warm"), (1, 0), "the warm asks the card that was on screen: {report}");
    // The scroll passes the cards above the last one: each loads its stale answer as it comes on
    // screen, once.
    let (lane, scrolled) = hits("scrolled");
    assert_eq!(lane, 0, "the Favorites card is off screen now: {report}");
    assert!((1..=3).contains(&scrolled), "the cards that scrolled in load: {report}");
    let (lane, below) = hits("refresh below");
    assert_eq!(lane, 0, "Refresh skips the Favorites card off screen: {report}");
    assert!((1..3).contains(&below), "Refresh asks the cards on screen, not all three: {report}");
}

/// A list's answer that arrives while another page shows ages from its arrival, not from the
/// revisit that shows it: Refresh, Settings before the answer came, and a revisit after the
/// freshness window asks again. Before, the list took the answer in only at the revisit, counted
/// it as new there and asked nothing; on a busy runner Refresh's own answer came that late.
#[test]
fn an_answer_that_lands_on_another_page_ages_from_its_arrival() {
    let fixture = Fixture::new("hidden-answer", 30);
    let mut harness = fixture.harness();
    wait_until_unsettled(&mut harness, "the pull request list", |harness| {
        has(harness, "Change 27")
    });
    wait_idle(&mut harness);
    wait_background_idle_without_frames(&harness);

    // Long enough for the three frames below on a busy machine.
    fixture.delay_listing(Duration::from_secs(2));
    fixture.recorder.enter("refresh");
    harness.click_at(harness.get_by_label("Refresh (F5)").rect());
    harness.step();
    harness.state_mut().navigator.go(Route::Settings);
    harness.step();
    assert!(harness.state().services.background.running() > 0, "Refresh's answer is on its way");
    // It arrives while Settings shows, between two frames.
    wait_background_idle_without_frames(&harness);
    let refresh = fixture.recorder.endpoints("refresh");
    assert_eq!(refresh.get(GROUP_LISTING).copied(), Some(1), "Refresh asked: {refresh:?}");
    fixture.delay_listing(Duration::ZERO);
    pump(&mut harness, REVISIT_FRESH.as_secs_f64() + 5.0);

    fixture.recorder.enter("revisit");
    harness.state_mut().navigator.go(Route::MyPullRequests);
    pump(&mut harness, 5.0);
    wait_background_idle_without_frames(&harness);
    let revisit = fixture.recorder.endpoints("revisit");
    assert_eq!(revisit.get(GROUP_LISTING).copied(), Some(1), "the revisit asks again: {revisit:?}");
}

/// Coming back to the window after a minimize, on the overview: its card on screen asks again at
/// once, however short the minimize, and its rows ask for their checks and votes again, which a
/// GitLab approval may not show in the listing.
#[test]
fn a_return_to_the_window_refreshes_the_overview() {
    let fixture = Fixture::new("overview-return", 30);
    let mut harness = fixture.harness();
    wait_until_unsettled(&mut harness, "the pull request list", |harness| {
        has(harness, "Change 27")
    });
    wait_idle(&mut harness);
    wait_background_idle_without_frames(&harness);
    harness.state_mut().attention = Attention::default();
    pump(&mut harness, 5.0);
    set_focus(&mut harness, false);
    // eframe runs no frame while minimized: simulated time jumps.
    harness.input_mut().time = Some(harness_time(&harness) + 20.0);
    fixture.recorder.enter("restored");
    set_focus(&mut harness, true);
    harness.input_mut().time = None;
    pump(&mut harness, 2.0);
    wait_background_idle_without_frames(&harness);
    pump(&mut harness, 1.0);
    wait_background_idle_without_frames(&harness);
    let hits = fixture.recorder.endpoints("restored");
    assert_eq!(hits.get(GROUP_LISTING).copied(), Some(1), "the listing at once: {hits:?}");
    assert_eq!(hits.get(STATUSES).copied(), Some(1), "the rows' checks and votes: {hits:?}");
}

const GRAPHQL: &str = "POST /api/graphql";
/// The GraphQL request for the list rows' checks, counted apart from the line counts.
const STATUSES: &str = "POST /api/graphql statuses";

/// The size column's line counts: one GraphQL request for every listed merge request of the
/// project, held back while the window is inactive. Scrolling and a revisit ask nothing more for
/// merge requests whose head did not move.
#[test]
fn line_counts_are_one_graphql_request_per_project() {
    let fixture = Fixture::new("line-counts", 30);
    let graphql =
        |phase: &str| fixture.recorder.endpoints(phase).get(GRAPHQL).copied().unwrap_or(0);
    // The rows arrive once the window lost focus.
    fixture.delay_listing(Duration::from_secs(2));
    fixture.recorder.enter("unfocused");
    let mut harness = fixture.harness();
    set_focus(&mut harness, false);
    wait_until_unsettled(&mut harness, "the pull request list", |harness| {
        has(harness, "Change 27")
    });
    fixture.delay_listing(Duration::ZERO);
    pump(&mut harness, 60.0);
    assert_eq!(graphql("unfocused"), 0, "an inactive window counts nothing");
    assert!(has(&harness, "Counting the lines of !27"), "the rows wait, reserving their place");

    fixture.recorder.enter("refocused");
    set_focus(&mut harness, true);
    // !27: 270 added, 27 deleted (see `diff_stats_summaries`).
    wait_until_unsettled(&mut harness, "the counted lines", |harness| {
        has(harness, "297 lines changed in 2 files: 270 added, 27 deleted")
    });
    pump(&mut harness, 30.0);
    wait_background_idle_without_frames(&harness);
    let refocused = fixture.recorder.endpoints("refocused");
    assert_eq!(
        graphql("refocused"),
        1,
        "one request for all 30 merge requests of the project: {refocused:?}"
    );

    fixture.recorder.enter("scrolled");
    scroll_list(&mut harness, 1600.0);
    pump(&mut harness, 30.0);
    assert!(
        has(&harness, "11 lines changed in 2 files"),
        "the lowest row came counted with the others"
    );
    assert_eq!(
        graphql("scrolled"),
        0,
        "scrolling asks nothing: {:?}",
        fixture.recorder.endpoints("scrolled")
    );

    harness.state_mut().navigator.go(Route::Settings);
    pump(&mut harness, REVISIT_FRESH.as_secs_f64() + 5.0);
    fixture.recorder.enter("revisit");
    harness.state_mut().navigator.go(Route::MyPullRequests);
    pump(&mut harness, 60.0);
    wait_background_idle_without_frames(&harness);
    let revisit = fixture.recorder.endpoints("revisit");
    assert_eq!(
        revisit.get("GET /api/v4/groups/group/merge_requests").copied(),
        Some(1),
        "{revisit:?}"
    );
    assert_eq!(graphql("revisit"), 0, "unchanged heads keep their counts: {revisit:?}");
}

/// Azure DevOps lists no sizes, so the size column asks each pull request's iterations and the
/// latest one's changes: for the rows on screen only, once per head commit, never while the window
/// is inactive. 25 changed files are more than are counted from git on their own, so the rows
/// show their files and "Count lines", and no mirror is made.
#[test]
fn azure_devops_sizes_are_asked_for_the_rows_on_screen() {
    let fixture = AzureFixture::new("azure-sizes");
    fixture.recorder.enter("listed");
    let mut harness = harness_for(fixture.paths.clone());
    wait_until_unsettled(&mut harness, "the pull request list", |harness| {
        has(harness, "Change 40")
    });
    pump(&mut harness, 30.0);
    wait_background_idle_without_frames(&harness);
    let listed = asked_pull_requests(&fixture.recorder, "listed");
    let on_screen = uncounted_rows(&harness);
    let endpoints = fixture.recorder.endpoints("listed");
    assert!(
        !listed.is_empty() && listed.len() < AZURE_ROWS as usize,
        "the rows on screen, not all {AZURE_ROWS}: {endpoints:?}"
    );
    assert_eq!(
        listed.keys().copied().collect::<BTreeSet<_>>(),
        on_screen,
        "every row on screen got its files, nothing else"
    );
    assert!(listed.values().all(|&asked| asked == 1), "once each: {listed:?}");
    let changes =
        "GET /north/Web/_apis/git/repositories/site/pullrequests/:id/iterations/:id/changes";
    assert_eq!(
        fixture.recorder.endpoints("listed").get(changes).copied(),
        Some(listed.len()),
        "and the latest iteration's changes"
    );
    assert!(
        !fixture.paths.mirrors_dir().exists()
            || std::fs::read_dir(fixture.paths.mirrors_dir())
                .is_ok_and(|mut dir| dir.next().is_none()),
        "no mirror for large pull requests"
    );

    set_focus(&mut harness, false);
    fixture.recorder.enter("unfocused");
    scroll_list(&mut harness, 1200.0);
    pump(&mut harness, 30.0);
    assert_eq!(
        fixture.recorder.count("unfocused"),
        0,
        "an inactive window asks nothing: {:?}",
        fixture.recorder.endpoints("unfocused")
    );
    assert!(has(&harness, "Counting the lines of !"), "rows that scrolled in wait in place");

    fixture.recorder.enter("refocused");
    set_focus(&mut harness, true);
    pump(&mut harness, 30.0);
    wait_background_idle_without_frames(&harness);
    let refocused = asked_pull_requests(&fixture.recorder, "refocused");
    assert!(!refocused.is_empty(), "the rows that scrolled in are asked once the window is active");
    assert!(
        refocused.keys().all(|number| !listed.contains_key(number)),
        "counted rows are not asked again: {refocused:?}"
    );
    assert!(
        refocused.keys().copied().collect::<BTreeSet<_>>().is_subset(&uncounted_rows(&harness)),
        "only rows on screen: {refocused:?}"
    );

    harness.state_mut().navigator.go(Route::Settings);
    pump(&mut harness, REVISIT_FRESH.as_secs_f64() + 5.0);
    fixture.recorder.enter("revisit");
    harness.state_mut().navigator.go(Route::MyPullRequests);
    pump(&mut harness, 30.0);
    wait_background_idle_without_frames(&harness);
    let revisit = fixture.recorder.endpoints("revisit");
    assert_eq!(
        revisit.get("GET /north/_apis/git/pullrequests").copied(),
        Some(1),
        "the revisit refreshes the list: {revisit:?}"
    );
    assert_eq!(
        fixture.recorder.count("revisit"),
        1,
        "unchanged pull requests keep their files: {revisit:?}"
    );
}

/// Favorites count their lines whatever their size. The starred repository's pull requests change
/// 25 files, more than are counted on their own (the test above keeps "Count lines" for them while
/// nothing is starred), and are counted from git all the same, without asking for their iterations:
/// the mirror's file list counts the files too. Only rows on screen are counted.
#[test]
fn favorites_always_count_their_lines() {
    let fixture = AzureFixture::with_git("azure-favorites");
    fixture.star_the_repository();
    fixture.recorder.enter("listed");
    let mut harness = harness_for(fixture.paths.clone());
    wait_until_unsettled(&mut harness, "the Favorites card", |harness| {
        has(harness, "Favorites (1 repository), 40 pull requests, expanded")
    });
    // Each file of the fixture's head changes three lines and adds one.
    wait_until_unsettled(&mut harness, "the counted lines of a large favorite", |harness| {
        has(harness, "175 lines changed in 25 files: 100 added, 75 deleted")
    });
    wait_until_unsettled(&mut harness, "every favorite on screen counted", |harness| {
        !has(harness, "Counting the lines of !")
    });
    wait_background_idle_without_frames(&harness);
    assert!(!has(&harness, "not counted"), "no favorite waits for Count lines");
    let endpoints = fixture.recorder.endpoints("listed");
    assert!(
        !endpoints
            .contains_key("GET /north/Web/_apis/git/repositories/site/pullrequests/:id/iterations"),
        "{endpoints:?}"
    );
    let details = endpoints
        .get("GET /north/Web/_apis/git/repositories/site/pullrequests/:id")
        .copied()
        .unwrap_or(0);
    assert!(
        details >= 1 && details < AZURE_ROWS as usize,
        "the rows on screen prepare their page, not all {AZURE_ROWS}: {endpoints:?}"
    );
}

/// Wheels the list down by `points` from over its rows.
pub(super) fn scroll_list(harness: &mut Harness<'_, ReviewApp>, points: f32) {
    harness.hover_at(egui::pos2(720.0, 600.0));
    let mouse = |delta: f32, phase| wheel(delta, phase, egui::MouseWheelSource::Wheel);
    harness.event(mouse(0.0, egui::TouchPhase::Start));
    for _ in 0..4 {
        harness.event(mouse(-points / 4.0, egui::TouchPhase::Move));
        harness.step();
    }
    harness.event(mouse(0.0, egui::TouchPhase::End));
    pump(harness, 2.0);
}

/// How often each Azure DevOps pull request's iterations were asked for in `phase`.
fn asked_pull_requests(recorder: &Recorder, phase: &str) -> BTreeMap<u64, usize> {
    let mut asked = BTreeMap::new();
    for path in recorder.details(|hit, _| hit == phase) {
        let number = path
            .strip_prefix(AZURE_PULL_REQUESTS)
            .and_then(|rest| rest.strip_suffix("/iterations"))
            .and_then(|number| number.parse().ok());
        if let Some(number) = number {
            *asked.entry(number).or_default() += 1;
        }
    }
    asked
}

/// The Azure DevOps rows on screen whose size column shows files without lines.
fn uncounted_rows(harness: &Harness<'_, ReviewApp>) -> BTreeSet<u64> {
    (1..=AZURE_ROWS)
        .filter(|number| {
            harness.query_by_label(&format!("Lines of !{number} not counted")).is_some()
        })
        .collect()
}

/// Replaces the checks table's filter text.
fn filter_checks(harness: &mut Harness<'_, ReviewApp>, text: &str) {
    harness.get_by_label("Filter checks").focus();
    harness.step();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get_by_label("Filter checks").type_text(text);
    harness.step();
    harness.step();
}

/// The git fetches of [`drive_scenario`]'s page steps.
struct ScenarioFetches {
    /// The objects of each fetch until the Files tab listed the files.
    before_files: Vec<usize>,
    /// Fetches while 20 diffs were browsed.
    while_browsing: usize,
}

fn run_scenario(file_count: usize) {
    let (fixture, fetches) = drive_scenario(file_count);
    assert_list_phases(&fixture);
    assert_browsing_reads_the_mirror(&fixture, &fetches);
    assert_pipeline_rounds(&fixture);
    assert_blob_chunks(file_count, &fetches);
}

/// The list, a merge request's overview, its Files tab with 20 diffs, its running pipelines, a job
/// log and the list again, each phase recorded; prints the report.
fn drive_scenario(file_count: usize) -> (Fixture, ScenarioFetches) {
    let fixture = Fixture::new(&format!("scenario-{file_count}"), file_count);
    let mut phases: Vec<(&'static str, f64)> = Vec::new();

    fixture.recorder.enter("My pull requests (60 s)");
    let mut harness = fixture.harness();
    wait_until_unsettled(&mut harness, "the pull request list", |harness| {
        has(harness, "Change 27")
    });
    pump(&mut harness, 60.0);
    phases.push(("My pull requests (60 s)", 60.0));

    enter_phase(&fixture.recorder, &harness, "Open MR + Overview (20 s)");
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key()));
    wait_until_unsettled(&mut harness, "the overview", |harness| has(harness, "Budget scenario"));
    pump(&mut harness, 20.0);
    // Simulated seconds pass faster than git works; a person reading the overview gives the
    // chunked download of the changed blobs (line counts) this time.
    wait_idle(&mut harness);
    // The changed files' grammar compiles while the overview shows, from the mirror's file list:
    // the phase's request counts below hold with it.
    wait_until_unsettled(&mut harness, "the files' grammar warmed on the overview", |harness| {
        held_languages(harness, &fixture).iter().any(|language| language == "Rust")
    });
    phases.push(("Open MR + Overview (20 s)", 20.0));
    let fetched_before_files = fixture.git_fetch_objects();
    let fetches_before_files = fetched_before_files.len();

    let files_label = format!("Files ({})", cv_widgets::numbers::group_digits(file_count));
    wait_until_unsettled(&mut harness, "the file list", |harness| {
        harness.query_by_label(&files_label).is_some()
    });
    // The phase starts once the Files tab shows: a probe while the Overview still shows refreshes
    // its running checks too (the click lands over a few frames), and belongs to the Overview,
    // also when its requests are still on the way.
    harness.click_at(harness.get_by_label(&files_label).rect());
    wait_until_unsettled(&mut harness, "the Files tab", |harness| {
        harness.query_by_label(&files_label).is_some_and(|tab| {
            tab.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
        })
    });
    wait_idle(&mut harness);
    enter_phase(&fixture.recorder, &harness, "Files + 20 diffs (60 s)");
    let started = harness_time(&harness);
    for _ in 0..20 {
        harness.key_press(egui::Key::J);
        pump(&mut harness, 2.0);
    }
    let browsed = harness_time(&harness) - started;
    pump(&mut harness, (60.0 - browsed).max(0.0));
    phases.push(("Files + 20 diffs (60 s)", 60.0));
    let fetches_while_browsing = fixture.git_fetches() - fetches_before_files;

    enter_phase(&fixture.recorder, &harness, "Pipelines, running (3 min)");
    let started = harness_time(&harness);
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_until_unsettled(&mut harness, "the jobs", |harness| has(harness, "test-3-1"));
    // A loaded machine stretches the wait over simulated seconds; the phase stays 3 min.
    let waited = harness_time(&harness) - started;
    pump(&mut harness, (180.0 - waited).max(0.0));
    phases.push(("Pipelines, running (3 min)", 180.0));

    enter_phase(&fixture.recorder, &harness, "Job log (10 s)");
    // 40 job rows: the filter brings the job into view, its row opens the log.
    harness.get_by_label("Filter checks").focus();
    harness.step();
    harness.get_by_label("Filter checks").type_text("test-3-2");
    harness.step();
    click_first(&mut harness, "test-3-2 in");
    pump(&mut harness, 10.0);
    phases.push(("Job log (10 s)", 10.0));

    enter_phase(&fixture.recorder, &harness, "Back to list + idle (5 min)");
    harness.state_mut().navigator.go(Route::MyPullRequests);
    pump(&mut harness, 300.0);
    phases.push(("Back to list + idle (5 min)", 300.0));

    let report = fixture.report(&phases);
    println!(
        "{report}git: {fetches_before_files} fetches until the files were listed, {fetches_while_browsing} while browsing 20 diffs\n"
    );
    let fetches = ScenarioFetches {
        before_files: fetched_before_files,
        while_browsing: fetches_while_browsing,
    };
    (fixture, fetches)
}

/// The list asks once and its rows' checks once; back on it, the card on screen refreshes every
/// interval, with its rows' checks.
fn assert_list_phases(fixture: &Fixture) {
    let count = |phase: &str| fixture.recorder.count(phase);
    let listed = fixture.recorder.endpoints("My pull requests (60 s)");
    assert_eq!(
        count("My pull requests (60 s)"),
        3,
        "one listing, one GraphQL request for its line counts and one for its rows' checks, no refresh within its interval: {listed:?}"
    );
    assert_eq!(listed.get(GRAPHQL).copied(), Some(1), "{listed:?}");
    assert_eq!(listed.get(STATUSES).copied(), Some(1), "{listed:?}");
    // The revisit refreshes the list once; then the card on screen refreshes every interval, each
    // time with its rows' checks and votes (one GraphQL request for the project's rows).
    let revisit = fixture.recorder.endpoints("Back to list + idle (5 min)");
    let beats = usize::try_from(300 / LIST_INTERVAL.as_secs()).expect("a few beats");
    assert_eq!(revisit.get(GROUP_LISTING).copied(), Some(1 + beats), "{revisit:?}");
    assert_eq!(revisit.get(STATUSES).copied(), Some(beats), "{revisit:?}");
    assert_eq!(count("Back to list + idle (5 min)"), 1 + 2 * beats, "{revisit:?}");
}

/// The overview loads the merge request once; the Files tab reads its diffs from the mirror and
/// asks the API only for the live-update probe: the merge request, its approvals and reviewers
/// every 30 s, never the discussions.
fn assert_browsing_reads_the_mirror(fixture: &Fixture, fetches: &ScenarioFetches) {
    let endpoint = |endpoints: &BTreeMap<String, usize>, name: &str| {
        endpoints
            .get(&format!("GET /api/v4/projects/:project/merge_requests/:id{name}"))
            .copied()
            .unwrap_or(0)
    };
    let browsing = fixture.recorder.endpoints("Files + 20 diffs (60 s)");
    let probes = endpoint(&browsing, "/approvals");
    assert!(probes <= 2, "one live-update probe per 30 s: {browsing:?}");
    assert_eq!(
        fixture.recorder.count("Files + 20 diffs (60 s)"),
        3 * probes,
        "diffs come from the git mirror; only the probe asks the API: {browsing:?}"
    );
    let open = fixture.recorder.endpoints("Open MR + Overview (20 s)");
    // Every merge request read is the detail or a probe (each with its approvals) or a pipelines load.
    assert_eq!(
        endpoint(&open, ""),
        endpoint(&open, "/approvals") + endpoint(&open, "/pipelines"),
        "{open:?}"
    );
    assert_eq!(
        endpoint(&open, "/discussions"),
        1,
        "an unchanged merge request is not reloaded: {open:?}"
    );
    assert_eq!(fetches.while_browsing, 0, "diffs and syntax colors read the local mirror");
}

/// The running pipelines refresh once per 15 s.
fn assert_pipeline_rounds(fixture: &Fixture) {
    let live = fixture.recorder.endpoints("Pipelines, running (3 min)");
    let rounds = live
        .get("GET /api/v4/projects/:project/merge_requests/:id/pipelines")
        .copied()
        .unwrap_or(0);
    assert!(
        rounds <= 13,
        "the first load, then one round per 15 s while a run is going: {rounds} rounds"
    );
}

/// The mirror fetches commits and trees, then the changed blobs in chunks. The thread excerpts'
/// diffs wait in the mirror for the count's chunks: no file is fetched on its own.
fn assert_blob_chunks(file_count: usize, fetches: &ScenarioFetches) {
    let chunks = review_git::download_chunk_sizes(file_count);
    assert_eq!(
        fetches.before_files.len(),
        1 + chunks.len(),
        "commits and trees, then the changed blobs in chunks of {chunks:?} files: objects per fetch {:?}",
        fetches.before_files
    );
}

fn open_pipelines(harness: &mut Harness<'_, ReviewApp>, fixture: &Fixture) {
    wait_until_unsettled(harness, "the pull request list", |harness| has(harness, "Change 27"));
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key()));
    wait_until_unsettled(harness, "the overview", |harness| has(harness, "Budget scenario"));
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_until_unsettled(harness, "the jobs", |harness| has(harness, "test-3-1"));
    pump(harness, 20.0);
}

/// Starts `phase` once the background work in flight has finished
/// ([`wait_background_idle_without_frames`]): what the last phase started (a job log's download, a
/// probe) reaches the host in that phase, however long a loaded machine takes to send it.
fn enter_phase(recorder: &Recorder, harness: &Harness<'_, ReviewApp>, phase: &str) {
    wait_background_idle_without_frames(harness);
    recorder.enter(phase);
}

/// Waits in real time, without a frame, until the background work in flight has finished, so the
/// requests it sends land in the current phase and no simulated time passes.
fn wait_background_idle_without_frames(harness: &Harness<'_, ReviewApp>) {
    let _step = cv_testkit::step("waiting without a frame for the background work to end");
    let started = Instant::now();
    while harness.state().services.background.running() > 0 {
        assert!(started.elapsed() < LIMIT, "background work did not finish");
        cv_testkit::let_threads_run(Duration::from_millis(2));
    }
}

/// The languages whose compiled grammars the fixture's pull request page keeps.
fn held_languages(harness: &Harness<'_, ReviewApp>, fixture: &Fixture) -> Vec<String> {
    let key = fixture.key();
    harness
        .state()
        .pull_requests
        .iter()
        .find(|(open, _)| *open == key)
        .map(|(_, page)| page.held_languages())
        .unwrap_or_default()
}

fn harness_time(harness: &Harness<'_, ReviewApp>) -> f64 {
    harness.ctx.input(|input| input.time)
}

/// [`wait_until_unsettled`] for work that takes real time, such as git reading a diff: egui's time
/// follows the wall clock meanwhile instead of going on by [`STEP_SECONDS`] a frame, so a loaded
/// machine does not stretch the work over simulated minutes. Afterwards it goes on by the step
/// again.
fn wait_in_real_time(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    let _step = cv_testkit::step(format!("waiting in real time for {what}"));
    let (started, time) = (Instant::now(), harness_time(harness));
    loop {
        harness.input_mut().time = Some(time + started.elapsed().as_secs_f64());
        harness.step();
        if is_done(harness) {
            break;
        }
        assert!(started.elapsed() < LIMIT, "timed out waiting for {what}");
        cv_testkit::let_threads_run(Duration::from_millis(10));
    }
    harness.input_mut().time = None;
}

struct Fixture {
    host: MockHost,
    recorder: Arc<Recorder>,
    paths: AppPaths,
    /// Unique per test: request counters and rate-limit notices are kept per connection id.
    connection_id: String,
    /// Real time the group listing takes to answer.
    listing_delay: Arc<Mutex<Duration>>,
    _scratch: ScratchDir,
}

impl Fixture {
    fn new(name: &str, file_count: usize) -> Self {
        Self::with_token(name, file_count, Some(MOCK_TOKEN))
    }

    fn with_token(name: &str, file_count: usize, token: Option<&str>) -> Self {
        Self::build(name, file_count, token, 0)
    }

    /// An anonymous connection whose merge request lists `older` finished pipelines of earlier
    /// commits after the head commit's running one.
    fn with_older_pipelines(name: &str, older: u64) -> Self {
        Self::build(name, 30, None, older)
    }

    fn build(name: &str, file_count: usize, token: Option<&str>, older_pipelines: u64) -> Self {
        let scratch = ScratchDir::new(&format!("budget-{name}"));
        let (repository, base, head) =
            build_repository(&scratch.path().join("remote.git"), file_count);
        let recorder = Arc::new(Recorder::default());
        let listing_delay = Arc::new(Mutex::new(Duration::ZERO));
        let host = MockHost::start(|base_url| MockGitLab {
            recorder: recorder.clone(),
            base_url: base_url.to_owned(),
            clone_url: file_url(&repository),
            base,
            head,
            older_pipelines,
            listing_delay: listing_delay.clone(),
        });
        let connection_id = format!("budget-{name}");
        let connection = Connection {
            name: "Mock GitLab".to_owned(),
            organizations: vec!["group".to_owned()],
            token: token.map(str::to_owned),
            ..samples::connection(&connection_id, ProviderKind::GitLab, &host.uri())
        };
        let paths = saved_paths(&scratch, &[connection]);
        Self { host, recorder, paths, connection_id, listing_delay, _scratch: scratch }
    }

    /// The app on the fixture. Its phases let simulated minutes pass without a pointer, which
    /// would count as the user's pauses; the return after one is measured on its own.
    fn harness(&self) -> Harness<'static, ReviewApp> {
        let mut harness = harness_for(self.paths.clone());
        harness.state_mut().attention = Attention::without_pauses();
        harness
    }

    /// [`Self::harness`] after its first frames only ([`startup::first_frames`]): for a test that
    /// checks a state while the app still loads. A settled start runs frames until nothing asks
    /// for a repaint, and the spinner of a running load asks every frame: a slow host's answer
    /// would land before the test sees the first frame.
    fn loading_harness(&self) -> Harness<'static, ReviewApp> {
        let paths = self.paths.clone();
        let mut harness = startup::first_frames(harness_builder(), |creation| {
            ReviewApp::new(creation, paths, cv_providers::create_provider)
        });
        harness.state_mut().attention = Attention::without_pauses();
        harness
    }

    /// Another connection with a token to the same mock, `{connection_id}-{suffix}`: its card lists
    /// group `group-{suffix}`, with the same merge requests, below the fixture's.
    fn add_connection(&self, suffix: &str) {
        let mut store = ConnectionStore::open(&self.paths).expect("the store");
        store
            .upsert(Connection {
                name: format!("Mock GitLab {suffix}"),
                organizations: vec![format!("group-{suffix}")],
                token: Some(MOCK_TOKEN.to_owned()),
                ..samples::connection(
                    &format!("{}-{suffix}", self.connection_id),
                    ProviderKind::GitLab,
                    &self.host.uri(),
                )
            })
            .expect("store the connection");
    }

    /// Lets the group listing answer after `delay` of real time from now on.
    fn delay_listing(&self, delay: Duration) {
        *self.listing_delay.lock().unwrap_or_else(PoisonError::into_inner) = delay;
    }

    /// The next request to `path` gets a 429 with `Retry-After` (not counted by the recorder).
    fn limit_once(&self, path: &str, retry_after_secs: u64) {
        self.limit_once_after(path, retry_after_secs, Duration::ZERO);
    }

    /// The next GraphQL request for the list rows' checks gets a 429 with `Retry-After`.
    fn limit_row_statuses_once(&self, retry_after_secs: u64) {
        let limited = ResponseTemplate::new(429)
            .insert_header("Retry-After", retry_after_secs.to_string().as_str())
            .set_body_string("Retry later");
        let mock = wiremock::Mock::given(wiremock::matchers::path("/api/graphql"))
            .and(wiremock::matchers::body_string_contains("headPipeline"))
            .respond_with(limited)
            .up_to_n_times(1)
            .with_priority(1);
        self.host.mount(mock);
    }

    /// [`Self::limit_once`] with the 429 arriving `delay` after the request.
    fn limit_once_after(&self, path: &str, retry_after_secs: u64, delay: Duration) {
        let limited = ResponseTemplate::new(429)
            .insert_header("Retry-After", retry_after_secs.to_string().as_str())
            .insert_header("RateLimit-Remaining", "0")
            .set_body_string("Retry later")
            .set_delay(delay);
        let mock = wiremock::Mock::given(wiremock::matchers::path(path))
            .respond_with(limited)
            .up_to_n_times(1)
            .with_priority(1);
        self.host.mount(mock);
    }

    fn key(&self) -> PullRequestKey {
        PullRequestKey {
            connection_id: self.connection_id.clone(),
            repository_key: PROJECT.to_owned(),
            number: IID,
        }
    }

    /// Fetches from the remote into the partial clone: each leaves one promisor pack.
    fn git_fetches(&self) -> usize {
        self.git_fetch_objects().len()
    }

    /// The objects each fetch from the remote brought, from its pack's index (version 2: the
    /// object count is the last entry of the fan-out table after the 8-byte header).
    fn git_fetch_objects(&self) -> Vec<usize> {
        let Ok(mirrors) = std::fs::read_dir(self.paths.mirrors_dir()) else {
            return Vec::new();
        };
        mirrors
            .flatten()
            .filter_map(|mirror| std::fs::read_dir(mirror.path().join("objects").join("pack")).ok())
            .flat_map(Iterator::flatten)
            .filter(|pack| pack.path().extension().is_some_and(|extension| extension == "promisor"))
            .map(|pack| {
                let index = std::fs::read(pack.path().with_extension("idx")).expect("pack index");
                let count = index.get(1028..1032).and_then(|bytes| bytes.try_into().ok());
                u32::from_be_bytes(count.expect("a version 2 pack index")) as usize
            })
            .collect()
    }

    fn report(&self, phases: &[(&str, f64)]) -> String {
        let mut report = format!("\n=== Request budget against {} ===\n", self.host.uri());
        for (phase, seconds) in phases {
            let count = self.recorder.count(phase);
            let per_minute = count as f64 / (seconds / 60.0);
            writeln!(report, "{phase}: {count} requests, {per_minute:.1}/min").unwrap();
            for (endpoint, hits) in self.recorder.endpoints(phase) {
                writeln!(report, "    {hits:>4}  {endpoint}").unwrap();
            }
        }
        report
    }
}

/// The app on `paths`, in a 1440 × 900 window, once its start settled.
fn harness_for(paths: AppPaths) -> Harness<'static, ReviewApp> {
    walkthrough::app_with(harness_builder(), paths, cv_providers::create_provider)
}

/// The app's harness with the window and step of [`harness_for`], before the renderer is picked.
pub(super) fn harness_builder() -> HarnessBuilder<ReviewApp> {
    Harness::builder().with_size([1440.0, 900.0]).with_step_dt(STEP_SECONDS)
}

const AZURE_ROWS: u64 = 40;
/// Changed files of each Azure DevOps pull request: more than are counted from git on their own.
const AZURE_FILES: usize = 25;
const AZURE_PULL_REQUESTS: &str = "/north/Web/_apis/git/repositories/site/pullrequests/";

/// An Azure DevOps connection (organization `north`) against [`MockAzure`].
struct AzureFixture {
    _host: MockHost,
    recorder: Arc<Recorder>,
    paths: AppPaths,
    connection_id: String,
    _scratch: ScratchDir,
}

impl AzureFixture {
    fn new(name: &str) -> Self {
        Self::build(name, false)
    }

    /// [`Self::new`] whose repository `north/Web/site` is served over git's smart HTTP protocol,
    /// every pull request changing [`AZURE_FILES`] files of the same two commits.
    fn with_git(name: &str) -> Self {
        Self::build(name, true)
    }

    fn build(name: &str, has_git: bool) -> Self {
        let scratch = ScratchDir::new(&format!("budget-{name}"));
        let git = has_git.then(|| {
            let project_root = scratch.path().join("git");
            let (_, base, head) =
                build_repository(&project_root.join("north/Web/_git/site"), AZURE_FILES);
            AzureGit { project_root, base, head }
        });
        let recorder = Arc::new(Recorder::default());
        let host = MockHost::start(|_| MockAzure { recorder: recorder.clone(), git });
        let connection_id = format!("budget-{name}");
        let connection = Connection {
            name: "Mock Azure DevOps".to_owned(),
            organizations: vec!["north".to_owned()],
            token: Some("ado-mock-only".to_owned()),
            ..samples::connection(&connection_id, ProviderKind::AzureDevOps, &host.uri())
        };
        let paths = saved_paths(&scratch, &[connection]);
        Self { _host: host, recorder, paths, connection_id, _scratch: scratch }
    }

    /// Stars `north/Web/site` before the app starts.
    fn star_the_repository(&self) {
        let mut settings = AppSettings::load(&self.paths.settings_file()).expect("no settings yet");
        settings.favorites.toggle(&self.connection_id, AZURE_REPOSITORY);
        settings.save(&self.paths.settings_file()).expect("save the favorite");
    }
}

const AZURE_REPOSITORY: &str = "north/Web/site";

/// The repository behind [`AzureFixture::with_git`]: `{project_root}/north/Web/_git/site`, and the
/// two commits every pull request compares.
struct AzureGit {
    project_root: PathBuf,
    base: String,
    head: String,
}

/// A scripted Azure DevOps REST API 7.1: [`AZURE_ROWS`] active pull requests of `north/Web/site`
/// (paged by `$top`/`$skip`, in the organization's listing and the repository's), each with one
/// iteration of [`AZURE_FILES`] changed files and a passed build policy, and the repository; with
/// `git`, also their details.
struct MockAzure {
    recorder: Arc<Recorder>,
    git: Option<AzureGit>,
}

impl Respond for MockAzure {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path().to_owned();
        self.recorder.record(format!("{} {}", request.method, template(&path)), path.clone());
        if let Some(git) = self.git.as_ref().filter(|_| path.starts_with("/north/Web/_git/")) {
            return super::github_mock::SmartGit { project_root: git.project_root.clone() }
                .respond(request);
        }
        let param = |name: &str| {
            request
                .url
                .query_pairs()
                .find(|(key, _)| key == name)
                .and_then(|(_, value)| value.parse::<usize>().ok())
        };
        let pull_request = path
            .strip_prefix(AZURE_PULL_REQUESTS)
            .and_then(|rest| rest.split_once('/'))
            .map(|(_, rest)| rest);
        let row = |id: u64| azure_pull_request(id, self.git.as_ref());
        // A pull request's detail: what preparing its page (and counting it from git) reads first.
        if let Some(number) = path
            .strip_prefix(AZURE_PULL_REQUESTS)
            .and_then(|number| number.parse::<u64>().ok())
            .filter(|_| self.git.is_some())
        {
            return ResponseTemplate::new(200).set_body_json(row(number));
        }
        let body = match (path.as_str(), pull_request) {
            ("/north/_apis/connectionData", _) => {
                json!({ "authenticatedUser": { "id": "me-id", "providerDisplayName": "Ada" } })
            }
            (
                "/north/_apis/git/pullrequests"
                | "/north/Web/_apis/git/repositories/site/pullrequests",
                _,
            ) => {
                let (skip, top) = (param("$skip").unwrap_or(0), param("$top").unwrap_or(100));
                let rows: Vec<Value> =
                    (1..=AZURE_ROWS).rev().map(row).skip(skip).take(top).collect();
                json!({ "count": rows.len(), "value": rows })
            }
            ("/north/Web/_apis/git/repositories/site", _) => {
                json!({ "id": "site-id", "name": "site", "project": { "id": "web-id", "name": "Web" } })
            }
            // The list rows' checks: one passed build policy each.
            ("/north/Web/_apis/policy/evaluations", _) => json!({ "count": 1, "value": [{
                "status": "approved",
                "configuration": { "isBlocking": true, "isEnabled": true, "type": { "displayName": "Build" } }
            }] }),
            (_, Some("iterations")) => json!({ "count": 1, "value": [{ "id": 1 }] }),
            (_, Some("iterations/1/changes")) => {
                let entries: Vec<Value> = (1..=AZURE_FILES)
                    .map(|index| json!({ "changeTrackingId": index, "changeId": index, "item": { "path": format!("/src/f{index}.rs") }, "changeType": "edit" }))
                    .collect();
                json!({ "changeEntries": entries })
            }
            _ => {
                return ResponseTemplate::new(404).set_body_json(json!({ "message": "Not found" }));
            }
        };
        ResponseTemplate::new(200).set_body_json(body)
    }
}

/// An active pull request of `north/Web/site`; higher numbers are newer. With `git` it compares
/// that repository's two commits, else made-up ones.
fn azure_pull_request(id: u64, git: Option<&AzureGit>) -> Value {
    let (base, head) = git.map_or_else(
        || (format!("{:040x}", 1_000_000), format!("{id:040x}")),
        |git| (git.base.clone(), git.head.clone()),
    );
    json!({
        "pullRequestId": id, "status": "active", "title": format!("Change {id}"),
        "repository": { "id": "site-id", "name": "site", "project": { "id": "web-id", "name": "Web" } },
        "createdBy": { "id": "author", "displayName": "Grace" },
        "creationDate": format!("2026-10-10T{:02}:{:02}:00Z", id / 60, id % 60),
        "sourceRefName": format!("refs/heads/feature/{id}"), "targetRefName": "refs/heads/main",
        "lastMergeSourceCommit": { "commitId": head },
        "lastMergeTargetCommit": { "commitId": base },
        "reviewers": [],
    })
}

/// GraphQL's `diffStatsSummary` of the merge requests a request names: iid × 10 lines added, iid
/// deleted, in 2 files.
pub(super) fn diff_stats_summaries(request: &Request) -> Value {
    let nodes: Vec<Value> = iids(&request.body).into_iter().map(|iid| json!({ "iid": iid.to_string(), "diffStatsSummary": { "additions": iid * 10, "deletions": iid, "fileCount": 2 } })).collect();
    json!({ "data": { "project": { "mergeRequests": { "nodes": nodes } } } })
}

/// The merge request's pipelines as GraphQL names them: the head pipeline has a `workflow:name`.
fn pipeline_names() -> Value {
    let pipelines = json!([
        { "id": format!("gid://gitlab/Ci::Pipeline/{PIPELINE_ID}"), "name": "Budget pipeline", "mergeRequestEventType": "DETACHED" }
    ]);
    json!({ "data": { "projects": { "nodes": [{ "mergeRequest": { "pipelines": { "nodes": pipelines } } }] } } })
}

/// The list rows' checks of the merge requests a request names: a passed pipeline, but [`IID`]'s
/// running one.
fn row_statuses(request: &Request) -> Value {
    let nodes: Vec<Value> = iids(&request.body)
        .into_iter()
        .map(|iid| {
            let status = if iid == IID { "RUNNING" } else { "SUCCESS" };
            json!({ "iid": iid.to_string(), "headPipeline": { "status": status, "stages": { "nodes": [] } } })
        })
        .collect();
    json!({ "data": { "project": { "mergeRequests": { "nodes": nodes } } } })
}

/// A scripted GitLab REST API v4: 30 merge requests, one with a running pipeline (5 stages, 40 jobs)
/// on its head commit and `older_pipelines` finished ones of earlier commits. GraphQL answers the
/// list's line counts (`diffStatsSummary`), its rows' checks (`headPipeline`) and the pipelines'
/// names (`mergeRequestEventType`) only, anything else 404.
struct MockGitLab {
    recorder: Arc<Recorder>,
    base_url: String,
    clone_url: String,
    base: String,
    head: String,
    older_pipelines: u64,
    listing_delay: Arc<Mutex<Duration>>,
}

impl Respond for MockGitLab {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path().to_owned();
        let asks_statuses = String::from_utf8_lossy(&request.body).contains("headPipeline");
        let suffix = if asks_statuses { " statuses" } else { "" };
        self.recorder
            .record(format!("{} {}{suffix}", request.method, template(&path)), path.clone());
        let project = format!("/api/v4/projects/{}", urlencoding::encode(PROJECT));
        let merge_request = format!("{project}/merge_requests/{IID}");
        let pipeline = |id: u64| format!("/api/v4/projects/{PROJECT_ID}/pipelines/{id}");
        let older = || OLDER_PIPELINE_ID..OLDER_PIPELINE_ID + self.older_pipelines;
        let body = match path.as_str() {
            "/api/v4/user" => user(1, "ada"),
            "/api/graphql"
                if String::from_utf8_lossy(&request.body).contains("diffStatsSummary") =>
            {
                diff_stats_summaries(request)
            }
            "/api/graphql" if asks_statuses => row_statuses(request),
            "/api/graphql"
                if String::from_utf8_lossy(&request.body).contains("mergeRequestEventType") =>
            {
                pipeline_names()
            }
            // The fixture's group, and those of connections added to it.
            p if p.starts_with("/api/v4/groups/") && p.ends_with("/merge_requests") => {
                let rows =
                    Value::Array((1..=30).rev().map(|iid| self.merge_request(iid)).collect());
                let delay = *self.listing_delay.lock().unwrap_or_else(PoisonError::into_inner);
                return ResponseTemplate::new(200).set_body_json(rows).set_delay(delay);
            }
            p if p == project => json!({
                "id": PROJECT_ID, "name": "app", "path_with_namespace": PROJECT, "web_url": format!("{}/{PROJECT}", self.base_url),
                "default_branch": "main", "http_url_to_repo": self.clone_url,
            }),
            // The project's own list: what the repository page and the Favorites card ask.
            p if p == format!("{project}/merge_requests") => {
                Value::Array((1..=30).rev().map(|iid| self.merge_request(iid)).collect())
            }
            p if p == merge_request => self.merge_request(IID),
            p if p == format!("{merge_request}/reviewers") => json!([]),
            p if p == format!("{merge_request}/approvals") => json!({ "approved_by": [] }),
            p if p == format!("{merge_request}/discussions") => self.discussions(),
            p if p == format!("{merge_request}/pipelines") => Value::Array(
                std::iter::once(self.pipeline()).chain(older().rev().map(older_pipeline)).collect(),
            ),
            p if p.ends_with("/statuses") => json!([]),
            p if p == format!("{}/jobs", pipeline(PIPELINE_ID)) => jobs(),
            p if older().any(|id| p == format!("{}/jobs", pipeline(id))) => older_jobs(),
            p if p.ends_with("/bridges") => json!([]),
            p if p.starts_with(&format!("/api/v4/projects/{PROJECT_ID}/jobs/"))
                && p.ends_with("/trace") =>
            {
                return ResponseTemplate::new(200).set_body_string(
                    "Running with gitlab-runner\n$ cargo test\ntest result: ok\n".repeat(200),
                );
            }
            _ => {
                return ResponseTemplate::new(404)
                    .set_body_json(json!({ "message": "404 Not Found" }));
            }
        };
        ResponseTemplate::new(200).set_body_json(body)
    }
}

impl MockGitLab {
    fn merge_request(&self, iid: u64) -> Value {
        let title = if iid == IID { "Budget scenario".to_owned() } else { format!("Change {iid}") };
        json!({
            "id": 1000 + iid, "iid": iid, "project_id": PROJECT_ID, "title": title, "description": "Measures requests.",
            "state": "opened", "author": user(2, "grace"), "source_branch": format!("feature-{iid}"), "target_branch": "main",
            "created_at": format!("2026-10-{:02}T10:00:00Z", 1 + iid % 28), "updated_at": "2026-10-17T10:00:00Z",
            "web_url": format!("{}/{PROJECT}/-/merge_requests/{iid}", self.base_url),
            "references": { "full": format!("{PROJECT}!{iid}") }, "labels": [], "reviewers": [], "sha": self.head,
            "diff_refs": { "base_sha": self.base, "start_sha": self.base, "head_sha": self.head },
            "detailed_merge_status": "mergeable", "head_pipeline": self.pipeline(),
        })
    }

    fn pipeline(&self) -> Value {
        json!({
            "id": PIPELINE_ID, "project_id": PROJECT_ID, "sha": self.head, "ref": "refs/merge-requests/7/head",
            "status": "running", "source": "merge_request_event", "created_at": "2026-10-18T09:00:00Z",
            "updated_at": "2026-10-18T09:05:00Z", "started_at": "2026-10-18T09:00:10Z",
            "web_url": format!("{}/{PROJECT}/-/pipelines/{PIPELINE_ID}", self.base_url),
        })
    }

    fn discussions(&self) -> Value {
        let threads: Vec<Value> = (0..THREADS)
            .map(|index| {
                let position = (index % 2 == 0).then(|| {
                    json!({ "position_type": "text", "head_sha": self.head, "new_path": file_path(index), "old_path": file_path(index), "new_line": 5 })
                });
                json!({ "id": format!("d{index}"), "notes": [{
                    "id": index + 1, "body": format!("Comment {index}"), "author": user(3, "linus"),
                    "created_at": "2026-10-17T12:00:00Z", "system": false, "resolvable": true, "resolved": false, "position": position,
                }]})
            })
            .collect();
        Value::Array(threads)
    }
}

/// A finished pipeline of an earlier commit.
fn older_pipeline(id: u64) -> Value {
    json!({
        "id": id, "project_id": PROJECT_ID, "sha": format!("{id:040x}"), "ref": "refs/merge-requests/7/head",
        "status": "success", "source": "merge_request_event", "created_at": "2026-10-17T09:00:00Z",
        "updated_at": "2026-10-17T09:05:00Z", "started_at": "2026-10-17T09:00:10Z", "finished_at": "2026-10-17T09:05:00Z",
        "web_url": format!("https://gitlab.example/{PROJECT}/-/pipelines/{id}"),
    })
}

/// The jobs of an older pipeline: one stage, all passed.
fn older_jobs() -> Value {
    let jobs: Vec<Value> = (1..=3)
        .map(|job| {
            json!({
                "id": 4000 + job, "name": format!("old-check-{job}"), "stage": "check", "status": "success", "allow_failure": false,
                "started_at": "2026-10-17T09:00:20Z", "finished_at": "2026-10-17T09:04:00Z",
            })
        })
        .collect();
    Value::Array(jobs)
}

fn jobs() -> Value {
    let stages = ["build", "lint", "test", "package", "deploy"];
    let mut jobs = Vec::new();
    for (stage_index, stage) in stages.iter().enumerate() {
        for job in 1..=8 {
            let status = match stage_index {
                2 if job == 1 => "running",
                0..=2 => "success",
                _ => "created",
            };
            jobs.push(json!({
                "id": 5000 + stage_index * 10 + job, "name": format!("{stage}-{}-{job}", stage_index + 1), "stage": stage,
                "status": status, "allow_failure": false, "started_at": "2026-10-18T09:00:20Z",
            }));
        }
    }
    Value::Array(jobs)
}

pub(super) fn user(id: u64, username: &str) -> Value {
    json!({ "id": id, "username": username, "name": username })
}

fn file_path(index: usize) -> String {
    format!("src/m{:02}/f{index:04}.rs", index / 30)
}

/// A bare repository with `main` (base) and `refs/merge-requests/7/head` changing `file_count` files.
fn build_repository(git_dir: &Path, file_count: usize) -> (PathBuf, String, String) {
    git_in(None, &["init", "--bare", "--quiet", &git_dir.display().to_string()]);
    let mut stream = Vec::new();
    let mut commit = |reference: &str,
                      mark: u32,
                      parent: Option<u32>,
                      contents: &dyn Fn(usize) -> String| {
        write!(
            stream,
            "commit {reference}\nmark :{mark}\ncommitter Test <test@example.com> {} +0000\ndata 6\nchange\n",
            1_700_000_000 + mark
        ).unwrap();
        if let Some(parent) = parent {
            writeln!(stream, "from :{parent}").unwrap();
        }
        for index in 0..file_count {
            let content = contents(index);
            write!(
                stream,
                "M 100644 inline {}\ndata {}\n{content}\n",
                file_path(index),
                content.len()
            )
            .unwrap();
        }
    };
    commit("refs/heads/main", 1, None, &|index| body(index, false));
    commit(&format!("refs/merge-requests/{IID}/head"), 2, Some(1), &|index| body(index, true));
    fast_import(git_dir, &stream);
    for (key, value) in
        [("uploadpack.allowFilter", "true"), ("uploadpack.allowAnySHA1InWant", "true")]
    {
        git_in(Some(git_dir), &["config", key, value]);
    }
    let base = git_in(Some(git_dir), &["rev-parse", "refs/heads/main"]);
    let head = git_in(Some(git_dir), &["rev-parse", &format!("refs/merge-requests/{IID}/head")]);
    (git_dir.to_path_buf(), base, head)
}

fn body(index: usize, is_head: bool) -> String {
    let mut lines: Vec<String> =
        (0..40).map(|line| format!("pub fn item_{index}_{line}() -> u32 {{ {line} }}")).collect();
    if is_head {
        lines[4] = format!("pub fn item_{index}_4() -> u32 {{ 400 }} // changed");
        lines[20] = format!("pub fn item_{index}_20() -> u32 {{ 2000 }} // changed");
        lines.push(format!("pub fn added_{index}() -> u32 {{ 1 }}"));
    }
    lines.join("\n")
}
