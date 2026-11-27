//! Running pipelines follow live on a real pull request page, offline: a scripted provider reports
//! one running pipeline whose jobs move on while the runs' answer stays the same, until the run
//! finishes. The Overview's merge-status card and both pages of the Pipelines tab show each step
//! within one poll interval plus one frame; the window inactive asks nothing, focus catches up
//! once, anonymous connections poll more slowly, and a finished run is not asked again. The clock
//! is the harness's simulated frames; between two frames the mock's answers get the real time they
//! need, so a frame stands for any network round trip.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use egui_kittest::kittest::Queryable;
use review_core::{
    PipelineJob, PipelineRun, ProviderError, PullRequestDetail, PullRequestRevision,
    PullRequestSummary, Result, RunStatus, Timestamp,
};
use review_store::PullRequestTab as Tab;
use time::macros::datetime;

use crate::page_harness::PageApp;
use cv_pr_pipelines::stage_bars::Stages;
use cv_pr_pipelines::stages_of;
use cv_runtime::load::Live;
use cv_runtime::refresh::{ANONYMOUS_PIPELINE_INTERVAL, CATCH_UP_STAGGER, PIPELINE_INTERVAL};
use cv_runtime::route::PullRequestKey;
use cv_testkit::provider::{Script, Scripted};
use cv_testkit::samples;

const REPOSITORY: &str = "erika/fabrikam-logistics";
const NUMBER: u64 = 42;
const RUN: &str = "pipeline:50";
const RUN_NAME: &str = "Pipeline #50";
/// One frame of simulated time. The bounds below are "interval plus one frame" whatever its length;
/// a tenth of a second keeps the minutes these scenarios span quick to step through.
const FRAME: f32 = 0.1;

/// How far the scripted pipeline got. The runs' answer is the same for every phase before
/// [`Phase::Finished`]: only its jobs move on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    /// "build" passed; "test" runs rspec and lint; "deploy" waits.
    Testing,
    /// lint passed, rspec still runs.
    LintDone,
    /// "test" passed; "deploy" runs.
    Deploying,
    /// Everything passed, the run with it.
    Finished,
}

/// The scripted pipeline and how often the page asked for it.
#[derive(Debug)]
struct Pipeline {
    phase: Phase,
    runs_asked: usize,
    jobs_asked: usize,
}

struct App {
    kit: PageApp,
    pipeline: Arc<Mutex<Pipeline>>,
}

impl App {
    fn start(id: &str, token: Option<&str>) -> Self {
        let pipeline =
            Arc::new(Mutex::new(Pipeline { phase: Phase::Testing, runs_asked: 0, jobs_asked: 0 }));
        let key = PullRequestKey {
            connection_id: id.to_owned(),
            repository_key: REPOSITORY.to_owned(),
            number: NUMBER,
        };
        let platform = pipeline.clone();
        let kit = PageApp::start(
            key,
            FRAME,
            token,
            // The card lists the pipelines with their stage bars.
            |services| services.settings.pipelines.is_overview_expanded = true,
            move |_, _| Arc::new(Scripted(Platform(platform.clone()))),
        );
        Self { kit, pipeline }
    }

    fn pipeline(&self) -> MutexGuard<'_, Pipeline> {
        self.pipeline.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn asked(&self) -> (usize, usize) {
        let pipeline = self.pipeline();
        (pipeline.runs_asked, pipeline.jobs_asked)
    }

    fn set_phase(&self, phase: Phase) {
        self.pipeline().phase = phase;
    }

    /// Frames for `seconds` of simulated time.
    fn run_for(&mut self, seconds: f64) {
        let end = self.kit.now() + seconds;
        while self.kit.now() < end {
            self.kit.frame();
        }
    }

    /// Frames until `is_done`, at most `seconds` of simulated time; returns the time it took.
    fn until(&mut self, what: &str, seconds: f64, is_done: impl Fn(&Self) -> bool) -> f64 {
        let start = self.kit.now();
        while !is_done(self) {
            assert!(self.kit.now() - start <= seconds, "{what} did not come within {seconds} s");
            self.kit.frame();
        }
        self.kit.now() - start
    }

    /// Frames until the next poll went out and its answers are in, so a change made right after
    /// it waits the longest: a whole interval.
    fn after_a_poll(&mut self, interval: Duration) {
        let (runs, _) = self.asked();
        self.until("a poll", interval.as_secs_f64() + 1.0, |app| app.asked().0 > runs);
    }

    /// The statuses of the run's stages as the page holds them, which its stage bars and graph
    /// paint: `None` while they are not loaded.
    fn stage_states(&self) -> Option<Vec<(String, RunStatus)>> {
        let session = self.kit.page().session.as_ref().ok()?;
        let stages = session.stages(RUN).and_then(Live::ready)?;
        Some(stages.iter().map(|stage| (stage.name.clone(), stage.status)).collect())
    }

    /// The status of `job` (in any stage) as the page holds it.
    fn job_state(&self, job: &str) -> Option<RunStatus> {
        let session = self.kit.page().session.as_ref().ok()?;
        let stages = session.stages(RUN).and_then(Live::ready)?;
        stages
            .iter()
            .flat_map(|stage| &stage.children)
            .find(|node| node.name == job)
            .map(|node| node.status)
    }

    /// What the Overview's stage bars of the run draw from: its stages once loaded.
    fn overview_bars(&self) -> Option<Vec<RunStatus>> {
        let session = self.kit.page().session.as_ref().ok()?;
        let run = session.runs.ready()?.iter().find(|run| run.id == RUN)?;
        match stages_of(run, session) {
            Stages::Ready(stages) => Some(stages.iter().map(|stage| stage.status).collect()),
            _ => None,
        }
    }

    fn has(&self, label: &str) -> bool {
        self.kit.harness.query_by_label(label).is_some()
    }
}

/// Simulated time a change made right after a poll may take to show: the interval and one frame.
fn bound(interval: Duration) -> f64 {
    interval.as_secs_f64() + f64::from(FRAME) + 1e-6
}

/// The Overview's card follows the running run every 15 s on its own (the live probe fails
/// throughout, so nothing of it helps), then the finished run's stages load once more and nothing
/// is asked after that.
#[test]
fn overview_follows_running_stages_and_stops_when_the_run_finished() {
    use RunStatus::*;
    let mut app = App::start("ci-live-overview", Some("ghp_walkthrough"));
    app.kit.page_mut().show_tab(Tab::Overview);
    app.until("the stage bars", 10.0, |app| app.overview_bars().is_some());
    assert_eq!(app.overview_bars(), Some(vec![Succeeded, Running, Queued]));
    assert!(app.has("Merge status"), "the Overview's card shows the run");

    // A stage finishes while the run's own answer stays the same.
    app.after_a_poll(PIPELINE_INTERVAL);
    app.set_phase(Phase::Deploying);
    let took = app.until("the new stage bar", 20.0, |app| {
        app.overview_bars() == Some(vec![Succeeded, Succeeded, Running])
    });
    assert!(took <= bound(PIPELINE_INTERVAL), "the stage bar took {took} s");
    assert_eq!(app.job_state("rspec"), Some(Succeeded), "the job's state came along");

    // The run finishes: its state and its final stages show, then nothing more is asked.
    app.after_a_poll(PIPELINE_INTERVAL);
    app.set_phase(Phase::Finished);
    let took = app.until("the finished run", 20.0, |app| {
        app.overview_bars() == Some(vec![Succeeded, Succeeded, Succeeded])
    });
    assert!(took <= bound(PIPELINE_INTERVAL) + f64::from(FRAME), "the finish took {took} s");
    assert!(app.has(&format!("{RUN_NAME}, Succeeded")), "the run's row says it passed");
    app.run_for(1.0);
    let after_finish = app.asked();
    app.run_for(3.0 * PIPELINE_INTERVAL.as_secs_f64());
    assert_eq!(app.asked(), after_finish, "a finished run is not polled");
}

/// The checks table follows each job within 15 s, also after several polls whose runs' answer did
/// not change (no backoff to 30 or 60 s).
#[test]
fn checks_table_follows_jobs_every_interval() {
    let mut app = App::start("ci-live-checks", Some("ghp_walkthrough"));
    app.kit.page_mut().show_tab(Tab::Pipelines);
    let row = |job: &str, status: &str| format!("{job} in {RUN_NAME}, {status}");
    app.until("the checks table", 10.0, |app| app.has(&row("lint", "Running")));

    app.after_a_poll(PIPELINE_INTERVAL);
    app.set_phase(Phase::LintDone);
    let took = app.until("lint passed", 20.0, |app| app.has(&row("lint", "Succeeded")));
    assert!(took <= bound(PIPELINE_INTERVAL), "lint took {took} s");

    // Polls whose runs' answer repeats; a backoff would now wait 60 s.
    app.run_for(2.0 * PIPELINE_INTERVAL.as_secs_f64());
    app.after_a_poll(PIPELINE_INTERVAL);
    app.set_phase(Phase::Deploying);
    let took = app.until("rspec passed", 20.0, |app| app.has(&row("rspec", "Succeeded")));
    assert!(took <= bound(PIPELINE_INTERVAL), "rspec took {took} s");
    assert!(app.has(&row("release", "Running")), "the next stage's job runs");
}

/// The run's own page (its stage graph) follows each job within 15 s the same way.
#[test]
fn run_page_follows_jobs_every_interval() {
    use RunStatus::*;
    let mut app = App::start("ci-live-run", Some("ghp_walkthrough"));
    app.kit.page_mut().show_tab(Tab::Pipelines);
    app.kit.page_mut().pipelines.open_run(RUN);
    app.until("the stage graph", 10.0, |app| app.has("Stage test"));
    assert_eq!(app.job_state("lint"), Some(Running));

    app.after_a_poll(PIPELINE_INTERVAL);
    app.set_phase(Phase::LintDone);
    let took = app.until("lint passed", 20.0, |app| app.job_state("lint") == Some(Succeeded));
    assert!(took <= bound(PIPELINE_INTERVAL), "lint took {took} s");

    app.run_for(2.0 * PIPELINE_INTERVAL.as_secs_f64());
    app.after_a_poll(PIPELINE_INTERVAL);
    app.set_phase(Phase::Deploying);
    let took = app.until("deploy runs", 20.0, |app| {
        app.stage_states().is_some_and(|stages| stages[2].1 == Running)
    });
    assert!(took <= bound(PIPELINE_INTERVAL), "the stage took {took} s");
    assert!(app.has("Stage deploy"), "the graph still shows the run");
}

/// With the window inactive nothing is asked; focus brings one catch-up refresh at once, then the
/// normal interval.
#[test]
fn inactive_window_polls_nothing_and_focus_catches_up_once() {
    let mut app = App::start("ci-live-focus", Some("ghp_walkthrough"));
    app.kit.page_mut().show_tab(Tab::Overview);
    app.until("the stage bars", 10.0, |app| app.overview_bars().is_some());
    app.after_a_poll(PIPELINE_INTERVAL);

    app.kit.set_focus(false);
    app.kit.frame();
    let before = app.asked();
    app.run_for(4.0 * PIPELINE_INTERVAL.as_secs_f64());
    assert_eq!(app.asked(), before, "nothing is polled while the window is inactive");

    app.kit.set_focus(true);
    // The live probe may take the first catch-up slot; the pipelines follow one stagger later.
    let catch_up = CATCH_UP_STAGGER.as_secs_f64() + 2.0 * f64::from(FRAME);
    let took = app.until("the catch-up", catch_up, |app| app.asked().0 > before.0);
    assert!(took <= catch_up, "the catch-up took {took} s");
    assert_eq!(app.asked().0, before.0 + 1, "one catch-up, not one per missed interval");
    assert!(app.asked().1 > before.1, "the shown stages came along");
    let caught_up = app.asked();
    app.run_for(PIPELINE_INTERVAL.as_secs_f64() - 1.0);
    assert_eq!(app.asked(), caught_up, "the schedule resumes from the catch-up");
    app.run_for(2.0);
    assert_eq!(app.asked().0, caught_up.0 + 1, "then every interval again");
}

/// An anonymous connection polls every [`ANONYMOUS_PIPELINE_INTERVAL`], not every 15 s.
#[test]
fn anonymous_connections_poll_more_slowly() {
    let mut app = App::start("ci-live-anonymous", None);
    app.kit.page_mut().show_tab(Tab::Overview);
    app.until("the stage bars", 10.0, |app| app.overview_bars().is_some());
    let opened = app.asked();
    let interval = ANONYMOUS_PIPELINE_INTERVAL.as_secs_f64();
    app.run_for(interval - 2.0);
    assert_eq!(app.asked().0, opened.0, "no poll before the anonymous interval");
    app.run_for(3.0);
    assert_eq!(app.asked().0, opened.0 + 1, "one poll at the anonymous interval");
}

fn at(minute: i64) -> Timestamp {
    datetime!(2026-10-27 09:00 UTC) + time::Duration::minutes(minute)
}

fn job(name: &str, status: RunStatus, finished: i64) -> PipelineJob {
    PipelineJob {
        id: format!("job:{name}"),
        name: name.to_owned(),
        status,
        started_at: (status != RunStatus::Queued).then(|| at(0)),
        finished_at: status.is_finished().then(|| at(finished)),
        ..PipelineJob::default()
    }
}

fn stage(name: &str, jobs: Vec<PipelineJob>) -> PipelineJob {
    PipelineJob {
        id: format!("stage:{name}"),
        name: name.to_owned(),
        status: RunStatus::combined(jobs.iter().map(|job| job.status)),
        started_at: jobs.iter().filter_map(|job| job.started_at).min(),
        finished_at: jobs.iter().filter_map(|job| job.finished_at).max(),
        children: jobs,
        ..PipelineJob::default()
    }
}

fn stages(phase: Phase) -> Vec<PipelineJob> {
    use RunStatus::*;
    let done = |is_done: bool| if is_done { Succeeded } else { Running };
    let deploy = match phase {
        Phase::Testing | Phase::LintDone => Queued,
        Phase::Deploying => Running,
        Phase::Finished => Succeeded,
    };
    vec![
        stage("build", vec![job("compile", Succeeded, 2)]),
        stage(
            "test",
            vec![
                job("rspec", done(phase >= Phase::Deploying), 6),
                job("lint", done(phase >= Phase::LintDone), 4),
            ],
        ),
        stage("deploy", vec![job("release", deploy, 8)]),
    ]
}

fn runs(phase: Phase) -> Vec<PipelineRun> {
    let is_finished = phase == Phase::Finished;
    vec![PipelineRun {
        id: RUN.to_owned(),
        name: RUN_NAME.to_owned(),
        status: if is_finished { RunStatus::Succeeded } else { RunStatus::Running },
        description: Some("pull_request".to_owned()),
        started_at: Some(at(0)),
        finished_at: is_finished.then(|| at(8)),
        has_jobs: true,
        is_head: true,
        ..PipelineRun::default()
    }]
}

/// The platform as the page sees it: one open pull request with the scripted pipeline.
struct Platform(Arc<Mutex<Pipeline>>);

impl Platform {
    fn pipeline(&self) -> MutexGuard<'_, Pipeline> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl Script for Platform {
    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        let detail = samples::detail(repo, number);
        Ok(PullRequestDetail {
            summary: PullRequestSummary {
                title: "Issue an invoice for every charge".to_owned(),
                source_branch: "feat/invoicing".to_owned(),
                ..detail.summary
            },
            description: "Every successful charge now issues an invoice.".to_owned(),
            ..detail
        })
    }

    /// The live probe fails throughout: the pipelines follow without it.
    async fn revision(&self, _repo: &str, _number: u64) -> Result<PullRequestRevision> {
        Err(ProviderError::Local("The probe is off in this walkthrough.".to_owned()))
    }

    async fn pipeline_runs(&self, _repo: &str, _number: u64) -> Result<Vec<PipelineRun>> {
        let mut pipeline = self.pipeline();
        pipeline.runs_asked += 1;
        Ok(runs(pipeline.phase))
    }

    async fn pipeline_jobs(
        &self,
        _repo: &str,
        _number: u64,
        run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        let mut pipeline = self.pipeline();
        pipeline.jobs_asked += 1;
        Ok(if run_id == RUN { stages(pipeline.phase) } else { Vec::new() })
    }
}
