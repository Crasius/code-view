//! Walks the pipelines views on a real pull request page, offline: a scripted GitLab provider
//! reports a merge request with pipelines and status checks. The overview's merge-status card
//! leads to a run, the breadcrumb back to the checks table, whose filters, keyboard and reruns
//! work as the toolbar says. No network, no git. Runs with `cargo test`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{
    Capabilities, Connection, GitRemote, Label, MergeOptions, MergeStatus, MergeStrategy,
    NewThread, PipelineJob, PipelineRun, Provider, ProviderError, ProviderKind, PullRequestDetail,
    PullRequestQuery, PullRequestState, PullRequestSummary, RepositoryInfo, RerunAction, RerunKind,
    Result, ReviewThread, ReviewVote, Reviewer, RunStatus, RunningLogs, ThreadComment,
    ThreadStatus, Timestamp, UserIdentity,
};
use review_store::{AppPaths, AppSettings, CheckFilter, ConnectionStore};
use time::macros::datetime;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::ReviewApp;
use super::clicks::Clicks;
use cv_runtime::route::{PullRequestKey, Route};
use cv_settings::onboarding::CliTools;
use cv_testkit as test_support;

const REPOSITORY: &str = "fabrikam-logistics/fabrikam-logistics";
const NUMBER: u64 = 17;
const HEAD_SHA: &str = "3f9a2c1d8e7b6a5f4e3d2c1b0a9f8e7d6c5b4a39";
const TIMEOUT: Duration = Duration::from_secs(60);

/// Which pipelines the scripted provider reports, per connection id.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    /// Everything passed, as the user's merge request !17.
    Green,
    /// A failed pipeline, a failed status check and a running pipeline among passing ones.
    Mixed,
    /// An Azure DevOps run in the shape of a real public timeline: a running job without its own
    /// log, a job waiting for an agent and a skipped one, neither with any log.
    Azure,
    /// Azure DevOps build 1639558 in the shape of its public timeline: stage "Build" retried, its
    /// job "Source-Build (Managed)" failed in both attempts; earlier attempts are listed by id only.
    Attempts,
    /// The head commit passed; the failed pipeline of the push before stays listed as an older run.
    Superseded,
    /// A GitHub workflow run whose job "Backend / Lint and test" runs step "Pytest": GitHub serves
    /// the job's log only once the job finished (before, its log URL answers 404 `BlobNotFound`).
    GitHubRunning,
    /// A GitLab pipeline whose job "rspec" runs: its trace grows by 50 lines with every request.
    GitLabLive,
}

#[derive(Default)]
struct Calls {
    reruns: Vec<(String, RerunKind)>,
    /// Earlier attempts loaded, by target.
    attempts: Vec<String>,
    /// Logs downloaded, by log id.
    logs: Vec<String>,
    /// The running job of [`Scenario::GitHubRunning`] or [`Scenario::GitLabLive`] finished.
    is_job_finished: bool,
}

/// The scenario and the call log of each connection id; the provider factory is a plain function.
type Scripts = HashMap<String, (Scenario, Arc<Mutex<Calls>>)>;

static SCENARIOS: Mutex<Option<Scripts>> = Mutex::new(None);

#[test]
#[expect(clippy::too_many_lines, reason = "one walkthrough scenario, told step by step")]
fn pipelines_overview_card_opens_a_run_and_the_breadcrumb_leads_back() {
    let app = App::start("pipelines-green", Scenario::Green, [1500.0, 900.0]);
    let mut harness = app.harness;
    wait_for(&mut harness, "the merge status", |harness| has(harness, "All pipelines succeeded"));
    assert!(has(&harness, "2 pipelines · 2 succeeded"), "status checks are no pipelines");
    assert!(has(&harness, "No merge conflicts"));
    assert!(
        harness.query_by_role_and_label(Role::Link, "Pipeline #2279, Succeeded").is_none(),
        "collapsed by default"
    );

    harness.click_at(harness.get_by_label("Show pipelines").rect());
    wait_for(&mut harness, "the pipelines", |harness| {
        harness.query_by_role_and_label(Role::Link, "Pipeline #2279, Succeeded").is_some()
    });
    assert!(saved(&app.paths).pipelines.is_overview_expanded, "the open card is remembered");
    assert!(
        harness.query_by_role_and_label(Role::Link, "Security scan #88, Succeeded").is_none(),
        "status checks stay in the Pipelines tab"
    );
    assert!(harness.query_by_label("Detect changes").is_none(), "jobs stay in the Pipelines tab");

    // "Open" shows the run full width with its jobs inside the stage cards.
    harness.click_at(harness.get_by_role_and_label(Role::Link, "Pipeline #2279, Succeeded").rect());
    wait_for(&mut harness, "the stage graph", |harness| {
        harness.query_by_label("Stage Analyze").is_some()
    });
    assert!(has(&harness, "Pipeline #2279 (current page)"), "the breadcrumb names the run");
    assert!(harness.query_by_label("Detect changes").is_some(), "the card lists its jobs");
    // 3 runs and the 11 jobs of the two pipelines.
    assert!(
        harness.query_by_label_contains("Pipeline #2279, 1 of 14").is_some(),
        "the switcher says where the run stands"
    );
    assert!(
        harness
            .query_by_label("Previous check (K)")
            .is_some_and(|node| node.accesskit_node().is_disabled())
    );

    // J steps to the next check, the run's first job, and opens its log; K goes back to the run.
    harness.key_press(egui::Key::J);
    wait_for(&mut harness, "the first job's log", |harness| {
        has(harness, "Detect changes (current page)")
    });
    assert!(
        harness.query_by_label_contains("Detect changes, 2 of 14").is_some(),
        "the switcher follows"
    );
    harness.key_press(egui::Key::K);
    wait_for(&mut harness, "the run again", |harness| {
        harness.query_by_label("Stage Analyze").is_some()
    });
    assert!(harness.query_by_label_contains("Pipeline #2279, 1 of 14").is_some(), "K moves back");

    // The switcher lists every check; picking one opens it.
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::ComboBox
                    && node.label().is_some_and(|label| label.contains("Switch check"))
            })
            .rect(),
    );
    settle(&mut harness);
    assert!(
        harness.query_by_label("Security scan #88, Succeeded").is_some(),
        "status checks are options"
    );
    harness.click_at(harness.get_by_label("Docs preview #412, Succeeded").rect());
    wait_for(&mut harness, "the docs pipeline", |harness| {
        harness.query_by_label("Stage deploy").is_some()
    });
    assert!(harness.query_by_label_contains("Docs preview #412, 12 of 14").is_some());
    // The Jobs pivot shown last does not stick: the next pipeline opens at its stages.
    harness.click_at(harness.get_by_label("Jobs").rect());
    settle(&mut harness);
    assert!(harness.query_by_label("Stage deploy").is_none(), "the flat job list");
    harness.click_at(harness.get_by_role_and_label(Role::Link, "Pipelines").rect());
    wait_for(&mut harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });
    harness.click_at(harness.get_by_label("Pipeline #2279, Succeeded").rect());
    wait_for(&mut harness, "the run again", |harness| {
        harness.query_by_label("Stage Analyze").is_some()
    });

    // A job opens its log; the breadcrumb grows by the job, Esc goes back step by step.
    harness.click_at(harness.get_by_label("Quality").rect());
    wait_for(&mut harness, "the log", |harness| has(harness, "Quality (current page)"));
    assert!(
        harness.query_by_label_contains("Quality, 4 of 14").is_some(),
        "the header names the job"
    );
    harness.key_press(egui::Key::Escape);
    settle(&mut harness);
    assert!(
        harness.query_by_label("Stage Analyze").is_some(),
        "Esc on the log goes back to the stages"
    );
    assert!(
        harness.query_by_label_contains("Pipeline #2279, 1 of 14").is_some(),
        "and the header to the run"
    );

    // The breadcrumb's "Pipelines" leads to the checks table.
    harness.click_at(harness.get_by_role_and_label(Role::Link, "Pipelines").rect());
    wait_for(&mut harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });
    assert!(has(&harness, "Checks for 3f9a2c1"));
    assert!(!has(&harness, "Pipeline #2279 (current page)"));
}

/// The card's pipelines start collapsed; a click anywhere on the pipelines line (not only its
/// button) opens them without moving the line's summary, and the choice comes back after a
/// restart, for every pull request.
#[test]
fn pipelines_overview_card_remembers_open_or_closed_across_restarts() {
    const ID: &str = "pipelines-remembered";
    const SIZE: [f32; 2] = [1500.0, 900.0];
    let is_listed = |harness: &Harness<'_, ReviewApp>| {
        harness.query_by_role_and_label(Role::Link, "Pipeline #2279, Succeeded").is_some()
    };
    let app = App::start(ID, Scenario::Green, SIZE);
    let paths = app.paths.clone();
    let mut harness = app.harness;
    wait_for(&mut harness, "the merge status", |harness| has(harness, "All pipelines succeeded"));
    assert!(!is_listed(&harness) && has(&harness, "Show pipelines"), "collapsed by default");
    let summary = harness.get_by_label("2 pipelines · 2 succeeded").rect();

    // The headline is no button of its own; the line under it takes the click.
    harness.click_at(harness.get_by_label("All pipelines succeeded").rect());
    wait_for(&mut harness, "the pipelines", is_listed);
    assert!(has(&harness, "Hide pipelines"));
    assert!(saved(&paths).pipelines.is_overview_expanded, "the open card is saved");
    assert_eq!(
        harness.get_by_label("2 pipelines · 2 succeeded").rect(),
        summary,
        "the summary stays where it was"
    );
    drop(harness);

    let mut harness = App::reopen(ID, paths.clone(), SIZE).harness;
    wait_for(&mut harness, "the pipelines, open after the restart", is_listed);
    harness.click_at(harness.get_by_label("Hide pipelines").rect());
    wait_for(&mut harness, "the collapsed card", |harness| {
        !is_listed(harness) && has(harness, "Show pipelines")
    });
    assert!(!saved(&paths).pipelines.is_overview_expanded, "the closed card is saved");
    drop(harness);

    let mut harness = App::reopen(ID, paths, SIZE).harness;
    wait_for(&mut harness, "the merge status", |harness| has(harness, "All pipelines succeeded"));
    settle_long(&mut harness);
    assert!(!is_listed(&harness), "closed after the restart");
}

#[test]
fn pipelines_checks_table_sorts_filters_steps_and_reruns_failed() {
    let app = App::start("pipelines-mixed", Scenario::Mixed, [1500.0, 900.0]);
    let mut harness = app.harness;
    wait_for(&mut harness, "the merge status", |harness| has(harness, "1 pipeline failed"));
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_for(&mut harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });

    // 6 runs and the 14 jobs of the three pipelines; the rows below the fold are not drawn.
    let rows = row_labels(&harness);
    assert_eq!(
        rows[..7],
        [
            "Pipeline #2281, Failed",
            "Build images in Pipeline #2281, Failed",
            "License compliance, Failed",
            "Docs preview #412, Running",
            "pages in Docs preview #412, Running",
            "Pipeline #2279, Succeeded",
            "Detect changes in Pipeline #2279, Succeeded",
        ],
        "failed first, then running, then the platform's order with each run's jobs after it"
    );
    assert!(
        harness.query_by_label("Failed, 3 checks").is_some()
            && harness.query_by_label("All, 20 checks").is_some()
    );

    // Status chips filter, combine and are remembered.
    harness.click_at(harness.get_by_label("Failed, 3 checks").rect());
    settle(&mut harness);
    assert_eq!(row_labels(&harness).len(), 3);
    harness.click_at(harness.get_by_label("Running, 2 checks").rect());
    settle(&mut harness);
    assert_eq!(row_labels(&harness).len(), 5, "chips add up");
    assert_eq!(
        saved(&app.paths).pipelines.status_filter,
        [CheckFilter::Failed, CheckFilter::Running].into()
    );
    harness.click_at(harness.get_by_label("All, 20 checks").rect());
    settle(&mut harness);
    assert_eq!(row_labels(&harness)[..7], rows[..7]);
    assert!(saved(&app.paths).pipelines.status_filter.is_empty());

    // ↓ ↓ ↓ Enter opens the third row.
    harness.key_press(egui::Key::ArrowDown);
    harness.key_press(egui::Key::ArrowDown);
    harness.key_press(egui::Key::ArrowDown);
    harness.key_press(egui::Key::Enter);
    wait_for(&mut harness, "the third check", |harness| {
        harness.query_by_label_contains("License compliance, 3 of 20").is_some()
    });
    assert!(has(&harness, "No job details"), "a status check explains where its details are");
    harness.key_press(egui::Key::Escape);
    wait_for(&mut harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });

    // A job's row opens its log, its run's stages a step back.
    harness.click_at(harness.get_by_label("Build images in Pipeline #2281, Failed").rect());
    wait_for(&mut harness, "the job's log", |harness| has(harness, "Build images (current page)"));
    assert!(harness.query_by_label_contains("Build images, 2 of 20").is_some());
    harness.key_press(egui::Key::Escape);
    wait_for(&mut harness, "the run", |harness| harness.query_by_label("Stage Build").is_some());
    harness.key_press(egui::Key::Escape);
    wait_for(&mut harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });

    // "Rerun failed" reruns the failed jobs of the failed pipeline.
    harness.click_at(harness.get_by_label("Rerun failed").rect());
    wait_for(&mut harness, "the rerun", |harness| has(harness, "Rerun started"));
    assert_eq!(
        app.calls.lock().unwrap_or_else(PoisonError::into_inner).reruns,
        [("pipeline:2281".to_owned(), RerunKind::RunFailed)]
    );

    // "Logs" goes straight to the first failed log.
    let logs =
        harness.query_all_by_label("Logs").next().expect("the failed pipeline offers its logs");
    harness.click_at(logs.rect());
    wait_for(&mut harness, "the failed log", |harness| has(harness, "Build images (current page)"));
}

/// A failed pipeline of an older push does not fail the merge request: the summary, the chips and
/// "Rerun failed" speak for the head commit; the overview shows the latest run only, the checks
/// table keeps the older run, last, as an older run.
#[test]
fn pipelines_older_runs_stay_listed_but_never_fail_the_summary() {
    // Tall enough to draw every row of the checks table, the older run last.
    let app = App::start("pipelines-superseded", Scenario::Superseded, [1500.0, 1400.0]);
    let mut harness = app.harness;
    wait_for(&mut harness, "the merge status", |harness| has(harness, "All pipelines succeeded"));
    assert!(has(&harness, "1 pipeline · 1 succeeded"));
    harness.click_at(harness.get_by_label("Show pipelines").rect());
    wait_for(&mut harness, "the pipelines", |harness| {
        harness.query_by_role_and_label(Role::Link, "Pipeline #2279, Succeeded").is_some()
    });
    assert!(
        harness.query_by_role_and_label(Role::Link, "Pipeline #2275, Failed, older run").is_none(),
        "the overview shows the latest run only"
    );

    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_for(&mut harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });
    assert!(
        harness.query_by_label("Failed, 0 checks").is_some(),
        "the older failure is not counted"
    );
    assert!(harness.query_by_label("Rerun failed").is_none(), "nothing of the head commit failed");
    let rows = row_labels(&harness);
    assert_eq!(rows.first().map(String::as_str), Some("Pipeline #2279, Succeeded"));
    assert_eq!(
        rows.last().map(String::as_str),
        Some("Pipeline #2275, Failed, older run"),
        "listed after the head commit's checks: {rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("in Pipeline #2275")),
        "its jobs load once it is opened"
    );
}

/// A job click always opens the job in the app, never the browser: a running job without its own
/// log opens its first step's log, a job without any log its state with the way to Azure DevOps;
/// only that explicit action opens the platform.
#[test]
fn pipelines_job_clicks_stay_in_the_app() {
    let app = App::start("pipelines-azure", Scenario::Azure, [1500.0, 900.0]);
    let mut harness = app.harness;
    let mut opened = Vec::new();
    wait_for(&mut harness, "the tabs", |harness| harness.query_by_label("Pipelines").is_some());
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_for(&mut harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });
    harness.click_at(harness.get_by_label(&format!("{ADO_RUN}, Running")).rect());
    wait_for(&mut harness, "the stage graph", |harness| {
        harness.query_by_role_and_label(Role::Link, ADO_RUNNING_JOB).is_some()
    });

    // The running job has no log of its own yet; its first finished step's log opens.
    harness.click_at(harness.get_by_role_and_label(Role::Link, ADO_RUNNING_JOB).rect());
    collect_until(&mut harness, &mut opened, "the step's log", |harness| {
        has(harness, "Initialize job (current page)")
    });
    collect_until(&mut harness, &mut opened, "the downloaded log", |harness| {
        harness.query_by_label("Copy log").is_some()
    });

    // A job waiting for an agent has no log at all: its state, in the app.
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, ADO_WAITING_JOB).rect());
    collect_until(&mut harness, &mut opened, "the waiting job", |harness| {
        has(harness, "Waiting for an agent")
    });
    assert!(
        has(&harness, &format!("{ADO_WAITING_JOB} (current page)")),
        "the breadcrumb names the job"
    );
    assert!(opened.is_empty(), "no click on a job opened the browser: {opened:?}");

    // Back in the stage graph, the skipped job opens its state as well.
    harness.key_press(egui::Key::Escape);
    collect_until(&mut harness, &mut opened, "the stages", |harness| {
        harness.query_by_role_and_label(Role::Link, ADO_SKIPPED_JOB).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::Link, ADO_SKIPPED_JOB).rect());
    collect_until(&mut harness, &mut opened, "the skipped job", |harness| {
        has(harness, "was skipped due to conditions")
    });
    assert!(opened.is_empty(), "no click on a job opened the browser: {opened:?}");

    // Only the explicit action opens Azure DevOps, at the job: the one in the log pane, below the
    // header's own.
    let actions: Vec<_> = harness.query_all_by_label("Open in Azure DevOps").collect();
    assert_eq!(actions.len(), 2, "the header and the log pane offer the platform");
    let pane_action = actions
        .into_iter()
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("the log pane's action");
    harness.click_at(pane_action.rect());
    collect_frames(&mut harness, &mut opened, 10);
    assert_eq!(opened, [format!("{ADO_RESULTS}&view=logs&j=skipped")]);
}

/// GitHub serves a job's log only once the job finished: while it runs the pane says so with the
/// way to watch it live (at the step), never asks for the log and shows no error; the step list
/// stays live. When the stages refresh reports the job done, its log loads on its own, the step
/// still selected.
#[test]
fn pipelines_github_running_job_waits_for_its_log_and_loads_it_when_done() {
    let app = App::start("pipelines-github-running", Scenario::GitHubRunning, [1500.0, 900.0]);
    let mut harness = app.harness;
    let mut opened = Vec::new();
    let calls = app.calls.clone();
    let logs = || calls.lock().unwrap_or_else(PoisonError::into_inner).logs.clone();
    wait_for(&mut harness, "the tabs", |harness| harness.query_by_label("Pipelines").is_some());
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_for(&mut harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });
    harness.click_at(harness.get_by_label(&format!("{GH_RUN}, Running")).rect());
    wait_for(&mut harness, "the stage graph", |harness| {
        harness.query_by_role_and_label(Role::Link, GH_JOB).is_some()
    });

    harness.click_at(harness.get_by_role_and_label(Role::Link, GH_JOB).rect());
    collect_until(&mut harness, &mut opened, "the live state", |harness| {
        has(harness, "Live log only on GitHub")
    });
    assert!(has(&harness, "its log loads here on its own when it is done"));
    assert!(!has(&harness, "BlobNotFound") && !has(&harness, "404"), "no error while the job runs");

    // The running step: the pane names it, and watching live opens GitHub at the step.
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, "Pytest").rect());
    collect_until(&mut harness, &mut opened, "the step", |harness| {
        has(harness, "so the log of Pytest loads here on its own")
    });
    assert!(has(&harness, "Pytest (current page)"), "the breadcrumb names the step");
    harness.click_at(harness.get_by_label("Watch live on GitHub").rect());
    collect_frames(&mut harness, &mut opened, 10);
    assert_eq!(opened, [format!("{GH_JOB_URL}#step:5:1")]);
    assert_eq!(logs(), Vec::<String>::new(), "no request for a log GitHub does not serve yet");

    // The job finishes; the next stages refresh (15 s while focused) brings it and the log loads.
    calls.lock().unwrap_or_else(PoisonError::into_inner).is_job_finished = true;
    step_until(&mut harness, "the finished job's log", 30 * 60, |harness| {
        harness.query_by_label("Copy log").is_some()
    });
    assert_eq!(logs(), ["501"], "fetched once, when the job finished");
    assert!(has(&harness, "Pytest (current page)"), "the step stays selected");
    step_frames(&mut harness, 10 * 60);
    assert_eq!(logs(), ["501"], "a finished job's log is not fetched again");
}

/// A running GitLab job's trace follows the job: it opens at its end and grows every 4 s while
/// the view stays there; scrolled up, it keeps its place. Once the job finished, its end is
/// fetched once and the fetching stops.
#[test]
fn pipelines_gitlab_running_job_log_follows_the_job() {
    let app = App::start("pipelines-gitlab-live", Scenario::GitLabLive, [1500.0, 900.0]);
    let mut harness = app.harness;
    let calls = app.calls.clone();
    let requests = || {
        calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .logs
            .iter()
            .filter(|log| *log == "7:77")
            .count()
    };
    let top_line = |harness: &Harness<'_, ReviewApp>| {
        cv_shell::focus_mode::top_line(&harness.ctx, cv_shell::focus_mode::Surface::Log)
            .map_or(0, |(line, _)| line)
    };
    wait_for(&mut harness, "the tabs", |harness| harness.query_by_label("Pipelines").is_some());
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_for(&mut harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });
    harness.click_at(harness.get_by_label(&format!("{GL_RUN}, Running")).rect());
    wait_for(&mut harness, "the stage graph", |harness| {
        harness.query_by_role_and_label(Role::Link, GL_JOB).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::Link, GL_JOB).rect());

    wait_for(&mut harness, "the live log", |harness| has(harness, "200 lines · live"));
    let at_open = top_line(&harness);
    assert!(at_open > 150, "a live log opens at its end, top line {at_open}");
    step_until(&mut harness, "what the job added", 10 * 60, |harness| {
        has(harness, "250 lines · live")
    });
    let followed = top_line(&harness);
    assert!(followed >= at_open + 50, "the view follows the end: {at_open} -> {followed}");

    // Scrolled up, the log keeps its place while it grows.
    harness.get_by_role(Role::Document).hover();
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 2000.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Wheel,
    });
    // The wheel scrolls smoothly; once it settled the view stays where it went.
    step_frames(&mut harness, 60);
    let scrolled = top_line(&harness);
    assert!(scrolled < followed, "scrolled up: {followed} -> {scrolled}");
    step_until(&mut harness, "more of the job", 10 * 60, |harness| {
        has(harness, "300 lines · live")
    });
    assert_eq!(top_line(&harness), scrolled, "no jump while scrolled up");

    // The job finishes: its end comes once, then nothing more is fetched. A fetch while the log
    // was still live may already bring the end; the fetch for the end starts the frame the job
    // shows finished and counts only once it ran, so the count is taken with nothing in flight.
    calls.lock().unwrap_or_else(PoisonError::into_inner).is_job_finished = true;
    step_until(&mut harness, "the finished job", 30 * 60, |harness| line_count(harness).is_some());
    step_until(&mut harness, "the job's end", 5 * 60, |harness| {
        line_count(harness).is_some_and(|lines| (lines - 200) % 50 == 2)
            && harness.state().services.background.running() == 0
    });
    let after_end = requests();
    step_frames(&mut harness, 10 * 60);
    assert_eq!(requests(), after_end, "a finished job's log is not fetched again");
}

/// The log toolbar's line count once the log is no longer live.
fn line_count(harness: &Harness<'_, ReviewApp>) -> Option<usize> {
    harness.query_all_by_label_contains(" lines").find_map(|node| {
        let node = node.accesskit_node();
        // A plain label carries its text as the value.
        let text = node.label().or_else(|| node.value())?;
        text.strip_suffix(" lines")?.replace(',', "").parse().ok()
    })
}

/// Steps frames (each 1/60 s of app time) until `is_done`, at most `frames`; background loads get
/// a moment of real time between frames.
fn step_until(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    frames: usize,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    for _ in 0..frames {
        harness.step();
        if is_done(harness) {
            settle(harness);
            return;
        }
        test_support::let_threads_run(Duration::from_millis(1));
    }
    panic!("{what} did not come within {frames} frames");
}

fn step_frames(harness: &mut Harness<'_, ReviewApp>, frames: usize) {
    for _ in 0..frames {
        harness.step();
        test_support::let_threads_run(Duration::from_millis(1));
    }
}

/// A retried job shows its attempts; picking attempt 1 loads it (only then) and opens its failed
/// log in the app, with the attempt in the breadcrumb and a switcher back to the latest.
#[test]
#[expect(clippy::too_many_lines, reason = "one walkthrough scenario, told step by step")]
fn pipelines_attempts_open_earlier_attempts_in_the_app() {
    let app = App::start("pipelines-attempts", Scenario::Attempts, [1500.0, 900.0]);
    let mut harness = app.harness;
    let mut opened = Vec::new();
    let calls = app.calls.clone();
    let attempts_loaded = || calls.lock().unwrap_or_else(PoisonError::into_inner).attempts.clone();
    wait_for(&mut harness, "the tabs", |harness| harness.query_by_label("Pipelines").is_some());
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_for(&mut harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });
    let rows = row_labels(&harness);
    assert!(
        rows.contains(&format!("{RETRIED_JOB} in {RETRIED_RUN}, Failed, failed after 2 attempts")),
        "the checks table sums the retry up: {rows:?}"
    );
    assert!(
        rows.contains(&format!(
            "Test: Windows Server x64 in {RETRIED_RUN}, Succeeded, passed on retry"
        )),
        "{rows:?}"
    );
    assert!(
        rows.contains(&format!("{RETRIED_RUN}, Failed")),
        "the run's own status stays the latest attempt's"
    );
    harness.click_at(harness.get_by_label(&format!("{RETRIED_RUN}, Failed")).rect());
    collect_until(&mut harness, &mut opened, "the stage graph", |harness| {
        harness.query_by_label("Stage Build").is_some()
    });

    // Badges on the retried stage and job; nothing about earlier attempts loaded yet.
    assert!(
        harness.query_by_label("Build, 2 attempts. Switch attempt").is_some(),
        "the stage card's badge"
    );
    let badge = format!("{RETRIED_JOB}, 2 attempts. Switch attempt");
    assert!(harness.query_by_label(&badge).is_some(), "the job row's badge");
    assert!(attempts_loaded().is_empty(), "no request for attempts before one is opened");

    // The badge lists the attempts; attempt 1 loads and its failed log opens in the app.
    harness.click_at(harness.get_by_label(&badge).rect());
    collect_until(&mut harness, &mut opened, "the attempt list", |harness| {
        harness.query_by_label_contains("Attempt 1, opens on demand").is_some()
    });
    assert!(harness.query_by_label("Attempt 2, Failed, latest").is_some());
    harness.click_at(harness.get_by_label_contains("Attempt 1, opens on demand").rect());
    collect_until(&mut harness, &mut opened, "attempt 1's log", |harness| {
        has(harness, &format!("{RETRIED_JOB} · Attempt 1 (current page)"))
    });
    collect_until(&mut harness, &mut opened, "the downloaded log", |harness| {
        harness.query_by_label("Copy log").is_some()
    });
    assert_eq!(attempts_loaded(), [JOB_ATTEMPT], "the attempt loads once it is opened");
    assert!(
        calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .logs
            .contains(&"1639558:95".to_owned()),
        "attempt 1's own log"
    );
    assert!(
        harness.query_by_label_contains("Attempt 1 of 2. Switch attempt").is_some(),
        "the switcher names the attempt"
    );

    // A step of attempt 1 opens its own log and stays in attempt 1.
    harness.click_at(
        harness
            .get_by_role_and_label(
                Role::TreeItem,
                "Checkout dotnet/aspnetcore@refs/pull/69916/merge to s",
            )
            .rect(),
    );
    collect_until(&mut harness, &mut opened, "the step's log", |_| {
        calls.lock().unwrap_or_else(PoisonError::into_inner).logs.contains(&"1639558:87".to_owned())
    });
    assert!(
        harness.query_by_label_contains("Attempt 1 of 2. Switch attempt").is_some(),
        "still attempt 1"
    );

    // The switcher goes back to the latest attempt.
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::ComboBox
                    && node.label().is_some_and(|label| label.contains("Switch attempt"))
            })
            .rect(),
    );
    collect_until(&mut harness, &mut opened, "the switcher's list", |harness| {
        harness.query_by_label("Attempt 2, Failed, latest").is_some()
    });
    harness.click_at(harness.get_by_label("Attempt 2, Failed, latest").rect());
    collect_until(&mut harness, &mut opened, "the latest attempt", |harness| {
        has(harness, &format!("{RETRIED_JOB} (current page)"))
    });
    assert!(harness.query_by_label_contains("Attempt 2 of 2. Switch attempt").is_some());
    assert_eq!(attempts_loaded(), [JOB_ATTEMPT], "a loaded attempt is not loaded again");
    assert!(opened.is_empty(), "attempts open in the app, never the browser: {opened:?}");
}

/// Steps until `is_done` and 10 frames more, keeping every URL the app asked the platform to open.
fn collect_until(
    harness: &mut Harness<'_, ReviewApp>,
    opened: &mut Vec<String>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    let started = Instant::now();
    loop {
        collect_frames(harness, opened, 1);
        if is_done(harness) {
            collect_frames(harness, opened, 10);
            return;
        }
        assert!(started.elapsed() < TIMEOUT, "timed out waiting for {what}");
        test_support::let_threads_run(Duration::from_millis(10));
    }
}

fn collect_frames(harness: &mut Harness<'_, ReviewApp>, opened: &mut Vec<String>, frames: usize) {
    for _ in 0..frames {
        harness.step();
        opened.extend(harness.output().platform_output.commands.iter().filter_map(|command| {
            match command {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                _ => None,
            }
        }));
    }
}

struct App {
    harness: Harness<'static, ReviewApp>,
    paths: AppPaths,
    calls: Arc<Mutex<Calls>>,
}

impl App {
    fn start(id: &str, scenario: Scenario, size: [f32; 2]) -> Self {
        let calls = Arc::new(Mutex::new(Calls::default()));
        SCENARIOS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(id.to_owned(), (scenario, calls.clone()));
        let root = std::env::temp_dir().join(format!("review-{id}-{}", std::process::id()));
        test_support::remove_dir(&root);
        let paths = AppPaths::new(root.join("config"), root.join("cache"));
        let mut store = ConnectionStore::open(&paths).expect("empty store");
        store
            .upsert(Connection {
                id: id.to_owned(),
                name: "example.com".to_owned(),
                kind: platform(scenario),
                base_url: "https://gitlab.example.com".to_owned(),
                organizations: Vec::new(),
                token: Some("glpat-walkthrough".to_owned()),
            })
            .expect("store the connection");
        Self::open(id, paths, size, calls)
    }

    /// The app again on the files an earlier [`App::start`] left, as after a restart.
    fn reopen(id: &str, paths: AppPaths, size: [f32; 2]) -> Self {
        let calls = SCENARIOS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|scenarios| scenarios.get(id))
            .expect("started before")
            .1
            .clone();
        Self::open(id, paths, size, calls)
    }

    fn open(id: &str, paths: AppPaths, size: [f32; 2], calls: Arc<Mutex<Calls>>) -> Self {
        let builder = Harness::builder()
            .with_size(size)
            .with_step_dt(1.0 / 60.0)
            .with_max_steps(100_000)
            .allow_missing_glyphs();
        let app_paths = paths.clone();
        let mut harness = builder
            .build_eframe(move |creation| ReviewApp::new(creation, app_paths, scripted_provider));
        harness.state_mut().settings.cli_tools = Some(CliTools::default());
        harness.state_mut().navigator.go(Route::PullRequest(PullRequestKey {
            connection_id: id.to_owned(),
            repository_key: REPOSITORY.to_owned(),
            number: NUMBER,
        }));
        Self { harness, paths, calls }
    }
}

fn saved(paths: &AppPaths) -> AppSettings {
    AppSettings::load(&paths.settings_file()).expect("settings file")
}

fn has(harness: &Harness<'_, ReviewApp>, text: &str) -> bool {
    harness.query_by_label_contains(text).is_some()
}

/// The checks table's rows, top to bottom.
fn row_labels(harness: &Harness<'_, ReviewApp>) -> Vec<String> {
    let mut rows: Vec<(f32, String)> = harness
        .query_all_by_role(Role::Row)
        .map(|node| (node.rect().top(), node.accesskit_node().label().unwrap_or_default()))
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, label)| label).collect()
}

fn wait_for(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    let started = Instant::now();
    loop {
        harness.step();
        if is_done(harness) {
            settle(harness);
            return;
        }
        assert!(started.elapsed() < TIMEOUT, "timed out waiting for {what}");
        test_support::let_threads_run(Duration::from_millis(10));
    }
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..10 {
        harness.step();
    }
}

/// Long enough for loads in flight and every eased transition to finish.
fn settle_long(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..30 {
        test_support::let_threads_run(Duration::from_millis(10));
        harness.step();
    }
}

fn at(minute: i64, second: i64) -> Timestamp {
    datetime!(2026-10-19 09:00 UTC)
        + time::Duration::minutes(minute)
        + time::Duration::seconds(second)
}

fn job(
    id: &str,
    name: &str,
    status: RunStatus,
    start: (i64, i64),
    end: Option<(i64, i64)>,
) -> PipelineJob {
    let ran = !matches!(status, RunStatus::Skipped | RunStatus::Neutral);
    PipelineJob {
        id: format!("job:{id}"),
        name: name.to_owned(),
        status,
        started_at: match status {
            RunStatus::Running => Some(running_since(40)),
            _ if ran => Some(at(start.0, start.1)),
            _ => None,
        },
        finished_at: end.filter(|_| ran).map(|(minute, second)| at(minute, second)),
        log_id: ran.then(|| format!("7:{id}")),
        web_url: Some(format!("https://gitlab.example.com/{REPOSITORY}/-/jobs/{id}")),
        reruns: if ran {
            vec![RerunAction::new(RerunKind::Job, format!("7:{id}"))]
        } else {
            Vec::new()
        },
        ..PipelineJob::default()
    }
}

fn stage(pipeline: u64, name: &str, after: Option<&str>, jobs: Vec<PipelineJob>) -> PipelineJob {
    PipelineJob {
        id: format!("stage:{pipeline}:{name}"),
        name: name.to_owned(),
        status: RunStatus::combined(jobs.iter().map(|job| job.status)),
        started_at: jobs.iter().filter_map(|job| job.started_at).min(),
        finished_at: jobs.iter().filter_map(|job| job.finished_at).max(),
        depends_on: after.map(|before| format!("stage:{pipeline}:{before}")).into_iter().collect(),
        children: jobs,
        ..PipelineJob::default()
    }
}

/// The user's pipeline #2279: five stages, the cleanup jobs manual.
fn pipeline_2279() -> Vec<PipelineJob> {
    use RunStatus::*;
    vec![
        stage(
            2279,
            "Analyze",
            None,
            vec![
                job("1", "Detect changes", Succeeded, (0, 0), Some((0, 17))),
                job("2", "Check platform access: review", Succeeded, (0, 0), Some((0, 14))),
                job("3", "Quality", Succeeded, (0, 0), Some((14, 25))),
            ],
        ),
        stage(
            2279,
            "Build",
            Some("Analyze"),
            vec![job("4", "Build images", Succeeded, (0, 20), Some((7, 33)))],
        ),
        stage(
            2279,
            "Infra Apps",
            Some("Build"),
            vec![job("5", "Infra Apps: review", Succeeded, (7, 40), Some((9, 40)))],
        ),
        stage(
            2279,
            "Deploy",
            Some("Infra Apps"),
            vec![job("6", "Deploy review", Succeeded, (9, 45), Some((12, 31)))],
        ),
        stage(
            2279,
            "Cleanup",
            Some("Deploy"),
            vec![
                job("7", "Reset review data", Neutral, (0, 0), None),
                job("8", "Repair review", Neutral, (0, 0), None),
                job("9", "Stop review", Neutral, (0, 0), None),
            ],
        ),
    ]
}

fn pipeline_2281() -> Vec<PipelineJob> {
    use RunStatus::*;
    vec![
        stage(
            2281,
            "Analyze",
            None,
            vec![job("21", "Detect changes", Succeeded, (0, 0), Some((0, 15)))],
        ),
        stage(
            2281,
            "Build",
            Some("Analyze"),
            vec![job("22", "Build images", Failed, (0, 20), Some((4, 2)))],
        ),
        stage(
            2281,
            "Deploy",
            Some("Build"),
            vec![job("23", "Deploy review", Skipped, (0, 0), None)],
        ),
    ]
}

const ADO_RUN: &str = "dotnet-android #1639760";
const ADO_RESULTS: &str = "https://dev.azure.com/erika/public/_build/results?buildId=1639760";
const ADO_RUNNING_JOB: &str = "macOS > Tests > MSBuild 9";
const ADO_WAITING_JOB: &str = "macOS > Tests > MSBuild 10";
const ADO_SKIPPED_JOB: &str = "MAUI Integration";

/// As `build_job_tree` maps the public timeline of `dnceng-public` build 1639760 while it ran:
/// jobs get their own log only once they finished, their finished tasks have theirs.
fn azure_1639760() -> Vec<PipelineJob> {
    use RunStatus::*;
    let ado =
        |id: &str, name: &str, status: RunStatus, log: Option<&str>, children: Vec<PipelineJob>| {
            PipelineJob {
                id: id.to_owned(),
                name: name.to_owned(),
                status,
                started_at: (status != Skipped && status != Queued).then(|| running_since(300)),
                log_id: log.map(|log| format!("1639760:{log}")),
                web_url: Some(format!("{ADO_RESULTS}&view=logs&j={id}")),
                children,
                ..PipelineJob::default()
            }
        };
    let tasks = vec![
        ado("init", "Initialize job", Succeeded, Some("227"), Vec::new()),
        ado("tests", "run Xamarin.Android.Build.Tests - macOS-9", Running, None, Vec::new()),
        ado("diagnose", "diagnose hanging processes (macOS/Linux)", Queued, None, Vec::new()),
    ];
    let running = ado("running", ADO_RUNNING_JOB, Running, None, tasks);
    let waiting = ado(
        "waiting",
        ADO_WAITING_JOB,
        Queued,
        None,
        vec![ado("pending", "Initialize job", Queued, None, Vec::new())],
    );
    let mut msbuild = stage(1_639_760, "MSBuild Tests", None, vec![running, waiting]);
    let mut maui = stage(
        1_639_760,
        "MAUI Tests",
        None,
        vec![ado("skipped", ADO_SKIPPED_JOB, Skipped, None, Vec::new())],
    );
    for stage in [&mut msbuild, &mut maui] {
        stage.web_url = Some(ADO_RESULTS.to_owned());
    }
    vec![msbuild, maui]
}

const RETRIED_RUN: &str = "aspnetcore-ci #1639558";
const RETRIED_RESULTS: &str = "https://dev.azure.com/erika/public/_build/results?buildId=1639558";
const RETRIED_JOB: &str = "Source-Build (Managed)";
const STAGE_ATTEMPT: &str = "1639558:6884a131:6884a131";
const JOB_ATTEMPT: &str = "1639558:6884a131:2f0d093c";

/// A node of build 1639558 as `build_job_tree` maps its timeline.
fn retried_node(
    id: &str,
    name: &str,
    status: RunStatus,
    log: Option<&str>,
    minutes: (i64, i64),
    children: Vec<PipelineJob>,
) -> PipelineJob {
    PipelineJob {
        id: id.to_owned(),
        name: name.to_owned(),
        status,
        started_at: Some(at(minutes.0, 0)),
        finished_at: Some(at(minutes.1, 30)),
        log_id: log.map(|log| format!("1639558:{log}")),
        web_url: Some(format!("{RETRIED_RESULTS}&view=logs&j={id}")),
        children,
        attempt: 1,
        ..PipelineJob::default()
    }
}

fn earlier(number: u32, target: &str) -> review_core::PipelineAttempt {
    review_core::PipelineAttempt {
        number,
        source: review_core::AttemptSource::Lazy(target.to_owned()),
    }
}

/// The latest attempts of build 1639558: "Build" in attempt 2 with the job that failed again,
/// "Code check" from attempt 1; "Platform tests" passed on its retry.
fn azure_1639558() -> Vec<PipelineJob> {
    use RunStatus::*;
    let tasks = vec![
        retried_node("t-init-2", "Initialize job", Succeeded, Some("257"), (10, 10), Vec::new()),
        retried_node(
            "t-checkout-2",
            "Checkout dotnet/aspnetcore@refs/pull/69916/merge to s",
            Failed,
            Some("259"),
            (10, 12),
            Vec::new(),
        ),
    ];
    let source_build = PipelineJob {
        attempt: 2,
        previous_attempts: vec![earlier(1, JOB_ATTEMPT)],
        ..retried_node("42c44170", RETRIED_JOB, Failed, Some("266"), (10, 13), tasks)
    };
    let code_check =
        retried_node("efda9e5f", "Code check", Succeeded, Some("85"), (2, 7), Vec::new());
    let build = PipelineJob {
        attempt: 2,
        previous_attempts: vec![earlier(1, STAGE_ATTEMPT)],
        web_url: Some(RETRIED_RESULTS.to_owned()),
        ..retried_node("ff05ad62", "Build", Failed, None, (2, 13), vec![code_check, source_build])
    };
    let windows = PipelineJob {
        attempt: 2,
        previous_attempts: vec![earlier(1, "1639558:6e9adb6a:78d63fcb")],
        ..retried_node(
            "71aac945",
            "Test: Windows Server x64",
            Succeeded,
            Some("280"),
            (7, 59),
            Vec::new(),
        )
    };
    let platform = PipelineJob {
        attempt: 2,
        previous_attempts: vec![earlier(1, "1639558:6e9adb6a:6e9adb6a")],
        web_url: Some(RETRIED_RESULTS.to_owned()),
        ..retried_node("8728e293", "Platform tests", Succeeded, None, (7, 59), vec![windows])
    };
    vec![build, platform]
}

/// The first attempts, as `Provider::pipeline_attempt` loads them from their own timeline.
fn attempt_1639558(target: &str) -> Option<PipelineJob> {
    use RunStatus::*;
    let job = || {
        let tasks = vec![
            retried_node("t-init-1", "Initialize job", Succeeded, Some("75"), (5, 5), Vec::new()),
            retried_node(
                "t-checkout-1",
                "Checkout dotnet/aspnetcore@refs/pull/69916/merge to s",
                Failed,
                Some("87"),
                (5, 8),
                Vec::new(),
            ),
        ];
        retried_node("2f0d093c", RETRIED_JOB, Failed, Some("95"), (5, 8), tasks)
    };
    match target {
        JOB_ATTEMPT => Some(job()),
        STAGE_ATTEMPT => Some(retried_node(
            "6884a131",
            "Build",
            Failed,
            None,
            (2, 38),
            vec![
                retried_node("efda9e5f", "Code check", Succeeded, Some("85"), (2, 7), Vec::new()),
                job(),
            ],
        )),
        _ => None,
    }
}

fn docs_412(is_running: bool) -> Vec<PipelineJob> {
    use RunStatus::*;
    let deploy = if is_running {
        job("32", "pages", Running, (0, 50), None)
    } else {
        job("32", "pages", Succeeded, (0, 50), Some((1, 10)))
    };
    vec![
        stage(
            412,
            "build",
            None,
            vec![job("31", "mkdocs build", Succeeded, (0, 0), Some((0, 45)))],
        ),
        stage(412, "deploy", Some("build"), vec![deploy]),
    ]
}

/// Running items started a moment ago, so their durations count up from there.
fn running_since(seconds: i64) -> Timestamp {
    time::OffsetDateTime::now_utc() - time::Duration::seconds(seconds)
}

fn run(
    id: &str,
    name: &str,
    status: RunStatus,
    description: &str,
    minutes: (i64, i64),
    has_jobs: bool,
) -> PipelineRun {
    let reruns = if has_jobs && status == RunStatus::Failed {
        vec![RerunAction::new(RerunKind::RunFailed, id)]
    } else {
        Vec::new()
    };
    PipelineRun {
        id: id.to_owned(),
        name: name.to_owned(),
        status,
        description: Some(description.to_owned()),
        web_url: Some(format!("https://gitlab.example.com/{REPOSITORY}/-/pipelines/{id}")),
        started_at: Some(if status.is_finished() { at(0, 0) } else { running_since(80) }),
        finished_at: status.is_finished().then(|| at(minutes.0, minutes.1)),
        has_jobs,
        is_head: true,
        reruns,
    }
}

fn runs(scenario: Scenario) -> Vec<PipelineRun> {
    use RunStatus::*;
    let event = "merge_request_event on refs/merge-requests/17/head";
    let green = run("pipeline:2279", "Pipeline #2279", Succeeded, event, (14, 25), true);
    let scan =
        run("status:88", "Security scan #88", Succeeded, "external status check", (3, 2), false);
    match scenario {
        Scenario::Green => vec![
            green,
            scan,
            run("pipeline:412", "Docs preview #412", Succeeded, "pages deployment", (1, 10), true),
        ],
        Scenario::Mixed => vec![
            green,
            run("pipeline:412", "Docs preview #412", Running, "pages deployment", (0, 0), true),
            run("pipeline:2281", "Pipeline #2281", Failed, event, (4, 2), true),
            scan,
            run(
                "status:91",
                "License compliance",
                Failed,
                "2 dependencies need a review",
                (0, 41),
                false,
            ),
            run("status:93", "Coverage report", Neutral, "coverage unchanged", (0, 12), false),
        ],
        Scenario::Azure => vec![PipelineRun {
            web_url: Some(ADO_RESULTS.to_owned()),
            ..run("pipeline:1639760", ADO_RUN, Running, "refs/pull/13176/merge", (0, 0), true)
        }],
        Scenario::Attempts => vec![PipelineRun {
            web_url: Some(RETRIED_RESULTS.to_owned()),
            ..run("pipeline:1639558", RETRIED_RUN, Failed, "refs/pull/69916/merge", (57, 35), true)
        }],
        Scenario::Superseded => vec![
            green,
            scan,
            PipelineRun {
                is_head: false,
                ..run("pipeline:2275", "Pipeline #2275", Failed, event, (6, 30), true)
            },
        ],
        // Another job keeps the run running, so the stages refresh after the job finished.
        Scenario::GitHubRunning => vec![PipelineRun {
            web_url: Some(GH_RUN_URL.to_owned()),
            ..run("run:900", GH_RUN, Running, "pull_request", (0, 0), true)
        }],
        Scenario::GitLabLive => vec![run("pipeline:3001", GL_RUN, Running, event, (0, 0), true)],
    }
}

fn platform(scenario: Scenario) -> ProviderKind {
    match scenario {
        Scenario::Azure | Scenario::Attempts => ProviderKind::AzureDevOps,
        Scenario::GitHubRunning => ProviderKind::GitHub,
        Scenario::Green | Scenario::Mixed | Scenario::Superseded | Scenario::GitLabLive => {
            ProviderKind::GitLab
        }
    }
}

const GH_RUN: &str = "CI #900";
const GH_JOB: &str = "Backend / Lint and test";
const GH_RUN_URL: &str = "https://github.com/erika/fabrikam-logistics/actions/runs/900";
const GH_JOB_URL: &str = "https://github.com/erika/fabrikam-logistics/actions/runs/900/job/501";

/// As the jobs API reports run 900: "Backend / Lint and test" in step "Pytest" (or done), "Backend
/// / Build image" still running.
fn github_900(is_finished: bool) -> Vec<PipelineJob> {
    use RunStatus::*;
    let step = |number: u32, name: &str, status: RunStatus| PipelineJob {
        id: format!("501:{number}"),
        name: name.to_owned(),
        status,
        started_at: Some(running_since(500 - i64::from(number) * 60)),
        finished_at: status.is_finished().then(|| running_since(0)),
        web_url: Some(format!("{GH_JOB_URL}#step:{number}:1")),
        ..PipelineJob::default()
    };
    let (job_status, pytest) =
        if is_finished { (Succeeded, Succeeded) } else { (Running, Running) };
    let lint = PipelineJob {
        id: "501".to_owned(),
        name: GH_JOB.to_owned(),
        status: job_status,
        started_at: Some(running_since(500)),
        finished_at: is_finished.then(|| running_since(0)),
        log_id: Some("501".to_owned()),
        web_url: Some(GH_JOB_URL.to_owned()),
        children: vec![
            step(1, "Set up job", Succeeded),
            step(4, "Ruff", Succeeded),
            step(5, "Pytest", pytest),
        ],
        ..PipelineJob::default()
    };
    let image = PipelineJob {
        id: "502".to_owned(),
        name: "Backend / Build image".to_owned(),
        status: Running,
        started_at: Some(running_since(300)),
        log_id: Some("502".to_owned()),
        web_url: Some(format!("{GH_RUN_URL}/job/502")),
        ..PipelineJob::default()
    };
    vec![PipelineJob {
        id: "group:backend".to_owned(),
        web_url: Some(GH_JOB_URL.to_owned()),
        ..stage(900, "Backend", None, vec![lint, image])
    }]
}

const GL_RUN: &str = "Pipeline #3001";
const GL_JOB: &str = "rspec";

fn gitlab_3001(is_finished: bool) -> Vec<PipelineJob> {
    use RunStatus::*;
    let rspec = PipelineJob {
        status: if is_finished { Succeeded } else { Running },
        finished_at: is_finished.then(|| running_since(0)),
        ..job("77", GL_JOB, Running, (0, 0), None)
    };
    vec![stage(3001, "test", None, vec![rspec, job("78", "rubocop", Running, (0, 0), None)])]
}

/// The trace of "rspec" at the `request`-th request (from 1): 200 lines, 50 more each time, the
/// job's last lines once it finished.
fn rspec_trace(request: usize, is_finished: bool) -> String {
    let lines = 200 + 50 * (request.max(1) - 1);
    let mut trace = (1..=lines).fold(String::new(), |mut trace, line| {
        writeln!(trace, "example {line} passed").unwrap();
        trace
    });
    if is_finished {
        trace.push_str("Finished in 2 minutes\nJob succeeded\n");
    }
    trace
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "a ProviderFactory, whose signature hands the connection and the client over"
)]
fn scripted_provider(connection: Connection, _http: reqwest::Client) -> Arc<dyn Provider> {
    let (scenario, calls) = SCENARIOS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|scenarios| scenarios.get(&connection.id).cloned())
        .unwrap_or((Scenario::Green, Arc::new(Mutex::new(Calls::default()))));
    let capabilities = Capabilities {
        votes: vec![ReviewVote::Approved, ReviewVote::None],
        merge_strategies: vec![MergeStrategy::Merge],
        has_label_catalog: true,
        running_logs: if platform(scenario) == ProviderKind::GitHub {
            RunningLogs::WhenFinished
        } else {
            RunningLogs::Growing
        },
    };
    Arc::new(ScriptedProvider { scenario, calls, capabilities })
}

struct ScriptedProvider {
    scenario: Scenario,
    calls: Arc<Mutex<Calls>>,
    capabilities: Capabilities,
}

impl ScriptedProvider {
    fn unused<T>() -> Result<T> {
        Err(ProviderError::Unsupported("Not part of the pipelines walkthrough.".to_owned()))
    }
}

fn person(id: &str, name: &str, login: &str) -> UserIdentity {
    UserIdentity {
        id: id.to_owned(),
        display_name: name.to_owned(),
        login: Some(login.to_owned()),
        avatar_url: None,
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(person("1", "Erika Hart", "erika"))
    }

    async fn list_repositories(&self, _search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        Ok(Vec::new())
    }

    async fn git_remote(&self, _repo: &str) -> Result<GitRemote> {
        Err(ProviderError::Local("The pipelines walkthrough has no git remote.".to_owned()))
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        Ok(Vec::new())
    }

    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        Ok(PullRequestDetail {
            summary: PullRequestSummary {
                repository_key: repo.to_owned(),
                repository_name: "fabrikam-logistics".to_owned(),
                number,
                title: "Issue an invoice for every charge".to_owned(),
                author: person("1", "Erika Hart", "erika"),
                state: PullRequestState::Open,
                is_draft: false,
                source_branch: "feat/invoicing".to_owned(),
                target_branch: "main".to_owned(),
                created_at: datetime!(2026-10-17 08:30 UTC),
                updated_at: None,
                web_url: format!("https://gitlab.example.com/{repo}/-/merge_requests/{number}"),
                labels: vec![Label { name: "skip-deploy".to_owned(), color: Some("3b4a5c".to_owned()), description: None }],
                reviewers: vec![Reviewer { identity: person("2", "Sam Okafor", "sam"), vote: ReviewVote::None, is_required: false, is_group: false }],
                checks: None,
                diff_stat: review_core::DiffStat::default(),
                head_sha: None,
                target_sha: None,
            },
            description: "Every successful charge now issues an invoice. Composition is keyed by (PackageId, DepotId) end to end.".to_owned(),
            target_sha: "0".repeat(40),
            head_sha: HEAD_SHA.to_owned(),
            merge_status: MergeStatus::Mergeable,
            git_fetch_refs: Vec::new(),
            closed_at: None,
            auto_complete: None,
        })
    }

    async fn merge(&self, _repo: &str, _number: u64, _options: &MergeOptions) -> Result<()> {
        Self::unused()
    }

    async fn close(&self, _repo: &str, _number: u64) -> Result<()> {
        Self::unused()
    }

    async fn reopen(&self, _repo: &str, _number: u64) -> Result<()> {
        Self::unused()
    }

    async fn set_draft(&self, _repo: &str, _number: u64, _is_draft: bool) -> Result<()> {
        Self::unused()
    }

    async fn update_title_description(
        &self,
        _repo: &str,
        _number: u64,
        _title: &str,
        _description: &str,
    ) -> Result<()> {
        Self::unused()
    }

    async fn set_vote(
        &self,
        _repo: &str,
        _number: u64,
        _vote: ReviewVote,
        _comment: Option<&str>,
    ) -> Result<()> {
        Self::unused()
    }

    async fn add_reviewer(&self, _repo: &str, _number: u64, _user: &str) -> Result<()> {
        Self::unused()
    }

    async fn remove_reviewer(&self, _repo: &str, _number: u64, _reviewer_id: &str) -> Result<()> {
        Self::unused()
    }

    async fn threads(&self, _repo: &str, _number: u64) -> Result<Vec<ReviewThread>> {
        Ok(Vec::new())
    }

    async fn create_thread(
        &self,
        _repo: &str,
        _number: u64,
        _thread: &NewThread,
    ) -> Result<ReviewThread> {
        Self::unused()
    }

    async fn reply(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _body: &str,
    ) -> Result<ThreadComment> {
        Self::unused()
    }

    async fn set_thread_status(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _status: ThreadStatus,
    ) -> Result<()> {
        Self::unused()
    }

    async fn available_labels(&self, _repo: &str) -> Result<Vec<Label>> {
        Ok(Vec::new())
    }

    async fn set_labels(&self, _repo: &str, _number: u64, _labels: &[String]) -> Result<()> {
        Self::unused()
    }

    async fn pipeline_runs(&self, _repo: &str, _number: u64) -> Result<Vec<PipelineRun>> {
        Ok(runs(self.scenario))
    }

    async fn pipeline_jobs(
        &self,
        _repo: &str,
        _number: u64,
        run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        Ok(match run_id {
            "pipeline:2279" => pipeline_2279(),
            "pipeline:2281" | "pipeline:2275" => pipeline_2281(),
            "pipeline:412" => docs_412(self.scenario == Scenario::Mixed),
            "pipeline:1639760" => azure_1639760(),
            "pipeline:1639558" => azure_1639558(),
            "run:900" => github_900(
                self.calls.lock().unwrap_or_else(PoisonError::into_inner).is_job_finished,
            ),
            "pipeline:3001" => gitlab_3001(
                self.calls.lock().unwrap_or_else(PoisonError::into_inner).is_job_finished,
            ),
            _ => Vec::new(),
        })
    }

    async fn pipeline_attempt(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
        target: &str,
    ) -> Result<PipelineJob> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).attempts.push(target.to_owned());
        attempt_1639558(target).ok_or_else(|| ProviderError::Local(format!("No attempt {target}.")))
    }

    async fn download_log(
        &self,
        _repo: &str,
        _number: u64,
        log_id: &str,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()> {
        let (requests, is_finished) = {
            let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
            calls.logs.push(log_id.to_owned());
            (calls.logs.iter().filter(|logged| *logged == log_id).count(), calls.is_job_finished)
        };
        let text = match (self.scenario, log_id) {
            // What GitHub's log URL leads to while the job runs.
            (Scenario::GitHubRunning, "501") if !is_finished => {
                return Err(ProviderError::Status {
                    status: 404,
                    path: "/actions-results/0d1f/workflow-job-run-501/logs/job/job-logs.txt".to_owned(),
                    body: "<?xml version=\"1.0\" encoding=\"utf-8\"?><Error><Code>BlobNotFound</Code><Message>The specified blob does not exist.</Message></Error>".to_owned(),
                });
            }
            (Scenario::GitLabLive, "7:77") => rspec_trace(requests, is_finished),
            _ => format!("$ ./ci/run {log_id}\nRunning on runner-7 (docker)\nJob succeeded\n"),
        };
        sink.write_all(text.as_bytes())
            .await
            .map_err(|error| ProviderError::Local(error.to_string()))
    }

    async fn rerun(
        &self,
        _repo: &str,
        _number: u64,
        run_id: &str,
        action: &RerunAction,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .reruns
            .push((run_id.to_owned(), action.kind));
        Ok(())
    }
}
