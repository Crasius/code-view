//! Walks a finished pipeline's job logs on a real pull request page, offline: a scripted GitLab
//! provider serves the logs and counts every download. A finished job's log is downloaded once,
//! then comes from disk, when switching back to it and after a restart; a download cut off
//! mid-body leaves no whole log, so the next open downloads it again. Runs with `cargo test`.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{
    Capabilities, Connection, GitRemote, Label, MergeOptions, MergeStatus, MergeStrategy,
    NewThread, PipelineJob, PipelineRun, Provider, ProviderError, ProviderKind, PullRequestDetail,
    PullRequestQuery, PullRequestState, PullRequestSummary, RepositoryInfo, RerunAction, Result,
    ReviewThread, ReviewVote, RunStatus, RunningLogs, ThreadComment, ThreadStatus, UserIdentity,
};
use review_store::{AppPaths, ConnectionStore};
use time::macros::datetime;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::ReviewApp;
use cv_runtime::route::{PullRequestKey, Route};
use cv_settings::onboarding::CliTools;
use cv_testkit as test_support;
use cv_testkit::Clicks;

const ID: &str = "log-reuse";
const REPOSITORY: &str = "erika/billing";
const NUMBER: u64 = 23;
const RUN: &str = "Pipeline #50";
const SIZE: [f32; 2] = [1500.0, 900.0];
/// The job whose first download the host drops mid-body.
const CUT_OFF: &str = "7:3";

/// Logs downloaded, by log id, across restarts.
static DOWNLOADS: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[test]
fn log_reuse_finished_logs_open_from_disk_and_a_cut_download_is_fetched_again() {
    let paths = start();
    let mut harness = open(&paths);
    open_run(&mut harness);

    // J opens the run's first job; its log is downloaded once.
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "build");
    assert_eq!(downloads("7:1"), 1, "the first open downloads the log");

    // Another job and back: the finished job's log comes from disk.
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "lint");
    harness.key_press(egui::Key::K);
    wait_for_log(&mut harness, "build");
    assert_eq!(downloads("7:1"), 1, "switching back downloads nothing");
    assert_eq!(downloads("7:2"), 1);
    assert_eq!(whole_logs(&paths), 2, "each finished job's log rests whole");

    // The host drops the third job's log mid-body: an error, and no whole file.
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "lint");
    harness.key_press(egui::Key::J);
    wait_for(&mut harness, "the cut-off download", |harness| {
        has(harness, "flaky (current page)") && has(harness, "The log of flaky did not download")
    });
    assert_eq!(downloads(CUT_OFF), 1);
    assert_eq!(whole_logs(&paths), 2, "a cut-off download never looks whole");
    harness.key_press(egui::Key::K);
    wait_for_log(&mut harness, "lint");
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "flaky");
    assert_eq!(downloads(CUT_OFF), 2, "the next open downloads it again");
    assert_eq!(whole_logs(&paths), 3);
    drop(harness);

    // After a restart on the same data folder, every finished job's log opens from disk.
    let before = DOWNLOADS.lock().unwrap_or_else(PoisonError::into_inner).len();
    let mut harness = open(&paths);
    open_run(&mut harness);
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "build");
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "lint");
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "flaky");
    assert_eq!(
        DOWNLOADS.lock().unwrap_or_else(PoisonError::into_inner).len(),
        before,
        "no request after the restart"
    );
}

/// A fresh data folder with the scripted connection.
fn start() -> AppPaths {
    let root = std::env::temp_dir().join(format!("review-{ID}-{}", std::process::id()));
    test_support::remove_dir(&root);
    let paths = AppPaths::new(root.join("config"), root.join("cache"));
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    store
        .upsert(Connection {
            id: ID.to_owned(),
            name: "example.com".to_owned(),
            kind: ProviderKind::GitLab,
            base_url: "https://gitlab.example.com".to_owned(),
            organizations: Vec::new(),
            token: Some("glpat-walkthrough".to_owned()),
        })
        .expect("store the connection");
    paths
}

/// The app on `paths`, at the pull request.
fn open(paths: &AppPaths) -> Harness<'static, ReviewApp> {
    let app_paths = paths.clone();
    let builder =
        Harness::builder().with_size(SIZE).with_step_dt(1.0 / 60.0).allow_missing_glyphs();
    let mut harness = super::startup::start(builder, move |creation| {
        ReviewApp::new(creation, app_paths, scripted_provider)
    });
    harness.state_mut().settings.cli_tools = Some(CliTools::default());
    harness.state_mut().navigator.go(Route::PullRequest(PullRequestKey {
        connection_id: ID.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number: NUMBER,
    }));
    harness
}

fn open_run(harness: &mut Harness<'_, ReviewApp>) {
    wait_for(harness, "the tabs", |harness| harness.query_by_label("Pipelines").is_some());
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_for(harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });
    harness.click_at(harness.get_by_label(&format!("{RUN}, Succeeded")).rect());
    wait_for(harness, "the stage graph", |harness| harness.query_by_label("Stage test").is_some());
}

/// Waits until `job`'s log is shown and nothing loads any more.
fn wait_for_log(harness: &mut Harness<'_, ReviewApp>, job: &str) {
    wait_for(harness, &format!("the log of {job}"), |harness| {
        has(harness, &format!("{job} (current page)"))
            && harness.query_by_label("Copy log").is_some()
            && harness.state().services.background.running() == 0
    });
}

fn downloads(log_id: &str) -> usize {
    DOWNLOADS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|id| *id == log_id)
        .count()
}

/// The whole logs in the cache.
fn whole_logs(paths: &AppPaths) -> usize {
    count_whole(&paths.connection_logs_dir(ID))
}

fn count_whole(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".whole.log"))
            .count()
    })
}

fn has(harness: &Harness<'_, ReviewApp>, text: &str) -> bool {
    harness.query_by_label_contains(text).is_some()
}

fn wait_for(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    super::action_e2e::wait_for(harness, what, |harness| is_done(harness).then_some(()), |_| {});
    for _ in 0..10 {
        harness.step();
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

fn job(id: u32, name: &str) -> PipelineJob {
    PipelineJob {
        id: format!("job:{id}"),
        name: name.to_owned(),
        status: RunStatus::Succeeded,
        started_at: Some(datetime!(2026-10-19 09:00 UTC)),
        finished_at: Some(datetime!(2026-10-19 09:04 UTC)),
        log_id: Some(format!("7:{id}")),
        web_url: Some(format!("https://gitlab.example.com/{REPOSITORY}/-/jobs/{id}")),
        ..PipelineJob::default()
    }
}

fn stage_test() -> PipelineJob {
    let jobs = vec![job(1, "build"), job(2, "lint"), job(3, "flaky")];
    PipelineJob {
        id: "stage:50:test".to_owned(),
        name: "test".to_owned(),
        status: RunStatus::Succeeded,
        started_at: Some(datetime!(2026-10-19 09:00 UTC)),
        finished_at: Some(datetime!(2026-10-19 09:04 UTC)),
        children: jobs,
        ..PipelineJob::default()
    }
}

fn scripted_provider(_connection: Connection, _http: reqwest::Client) -> Arc<dyn Provider> {
    Arc::new(ScriptedProvider {
        capabilities: Capabilities {
            votes: vec![ReviewVote::Approved, ReviewVote::None],
            merge_strategies: vec![MergeStrategy::Merge],
            has_label_catalog: true,
            running_logs: RunningLogs::Growing,
        },
    })
}

struct ScriptedProvider {
    capabilities: Capabilities,
}

impl ScriptedProvider {
    fn unused<T>() -> Result<T> {
        Err(ProviderError::Unsupported("Not part of the log reuse walkthrough.".to_owned()))
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
        Err(ProviderError::Local("The log reuse walkthrough has no git remote.".to_owned()))
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
                repository_name: "billing".to_owned(),
                number,
                title: "Retry failed charges once".to_owned(),
                author: person("1", "Erika Hart", "erika"),
                state: PullRequestState::Open,
                is_draft: false,
                source_branch: "feat/retry".to_owned(),
                target_branch: "main".to_owned(),
                created_at: datetime!(2026-10-17 08:30 UTC),
                updated_at: None,
                web_url: format!("https://gitlab.example.com/{repo}/-/merge_requests/{number}"),
                labels: Vec::new(),
                reviewers: Vec::new(),
                checks: None,
                diff_stat: review_core::DiffStat::default(),
                head_sha: None,
                target_sha: None,
            },
            description: "A declined charge is tried once more after a minute.".to_owned(),
            target_sha: "0".repeat(40),
            head_sha: "5".repeat(40),
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
        Ok(vec![PipelineRun {
            id: "pipeline:50".to_owned(),
            name: RUN.to_owned(),
            status: RunStatus::Succeeded,
            description: Some("merge_request_event".to_owned()),
            web_url: Some(format!("https://gitlab.example.com/{REPOSITORY}/-/pipelines/50")),
            started_at: Some(datetime!(2026-10-19 09:00 UTC)),
            finished_at: Some(datetime!(2026-10-19 09:04 UTC)),
            has_jobs: true,
            is_head: true,
            reruns: Vec::new(),
        }])
    }

    async fn pipeline_jobs(
        &self,
        _repo: &str,
        _number: u64,
        run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        Ok(if run_id == "pipeline:50" { vec![stage_test()] } else { Vec::new() })
    }

    async fn pipeline_attempt(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
        _target: &str,
    ) -> Result<PipelineJob> {
        Self::unused()
    }

    async fn download_log(
        &self,
        _repo: &str,
        _number: u64,
        log_id: &str,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()> {
        let request = {
            let mut downloads = DOWNLOADS.lock().unwrap_or_else(PoisonError::into_inner);
            downloads.push(log_id.to_owned());
            downloads.iter().filter(|id| *id == log_id).count()
        };
        let text = format!("$ ./ci/run {log_id}\nRunning on runner-7 (docker)\nJob succeeded\n");
        let written = |error: std::io::Error| ProviderError::Local(error.to_string());
        if log_id == CUT_OFF && request == 1 {
            sink.write_all(&text.as_bytes()[..text.len() / 2]).await.map_err(written)?;
            return Err(ProviderError::Network("connection reset by the host".to_owned()));
        }
        sink.write_all(text.as_bytes()).await.map_err(written)
    }

    async fn rerun(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
        _action: &RerunAction,
    ) -> Result<()> {
        Self::unused()
    }
}
