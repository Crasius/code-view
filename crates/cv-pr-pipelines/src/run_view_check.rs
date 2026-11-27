//! The run view against a fixed, Azure DevOps-like pipeline: parallel stages, a failed stage and
//! skipped ones, and a GitLab pipeline whose jobs use `needs`. Checks what the user can do. No
//! network, no tokens. Also the harness and the sample pipeline the pan and zoom checks share.

use cv_base::theme;
use cv_testkit::Clicks;
use egui::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, HarnessBuilder};
use review_core::{PipelineJob, ProviderKind, RerunAction, RerunKind, RunStatus, Timestamp};
use review_store::ThemePreference;
use time::macros::datetime;

use super::run_view::{RunView, RunViewAction, RunViewInput};

pub(super) struct State {
    pub(super) view: RunView,
    pub(super) stages: Vec<PipelineJob>,
    pub(super) platform: ProviderKind,
    pub(super) theme: ThemePreference,
    pub(super) has_token: bool,
    /// What the view's last action was, in [`describe`]'s words.
    pub(super) last: Option<String>,
}

impl State {
    /// `stages` on Azure DevOps, in the light theme, with a token.
    pub(super) fn new(stages: Vec<PipelineJob>) -> Self {
        Self {
            view: RunView::default(),
            stages,
            platform: ProviderKind::AzureDevOps,
            theme: ThemePreference::Light,
            has_token: true,
            last: None,
        }
    }
}

/// The run view over `state`, from `builder` with its size and frame timing. Motion is on,
/// whatever the machine says (a macOS runner may have Reduce Motion on), so every host draws the
/// same frames and a fling or a zoom step glides.
pub(super) fn build(builder: HarnessBuilder<State>, state: State) -> Harness<'static, State> {
    builder
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(
            |ui, state: &mut State| {
                let palette = theme::activate(ui.ctx(), state.theme);
                ui.painter().rect_filled(ui.max_rect(), 0, palette.ground);
                let input = RunViewInput {
                    run_id: "build:1",
                    stages: &state.stages,
                    platform: state.platform,
                    has_token: state.has_token,
                    is_busy: false,
                    known: super::attempts::Known::default(),
                };
                if let Some(action) = state.view.show(ui, input) {
                    state.last = Some(describe(action));
                }
            },
            state,
        )
}

fn harness(state: State) -> Harness<'static, State> {
    build(Harness::builder().with_size([1440.0, 820.0]), state)
}

pub(super) fn describe(action: RunViewAction) -> String {
    match action {
        RunViewAction::Log(target) => {
            format!("log {} {}", target.title, target.log_id.as_deref().unwrap_or("none"))
        }
        RunViewAction::Rerun(rerun) => format!("rerun {:?} {}", rerun.kind, rerun.target),
        RunViewAction::AddToken => "add token".to_owned(),
        RunViewAction::Attempt { node_id, number } => format!("attempt {number} of {node_id}"),
    }
}

fn at(minute: i64, second: i64) -> Timestamp {
    datetime!(2026-10-18 10:00 UTC)
        + time::Duration::minutes(minute)
        + time::Duration::seconds(second)
}

fn job(
    name: &str,
    status: RunStatus,
    start: (i64, i64),
    end: (i64, i64),
    log: &str,
) -> PipelineJob {
    let ran = status != RunStatus::Skipped;
    PipelineJob {
        id: format!("job-{name}"),
        name: name.to_owned(),
        status,
        started_at: ran.then(|| at(start.0, start.1)),
        finished_at: ran.then(|| at(end.0, end.1)),
        log_id: ran.then(|| format!("1:{log}")),
        web_url: Some(format!(
            "https://dev.azure.com/org/project/_build/results?buildId=1&view=logs&j={name}"
        )),
        ..PipelineJob::default()
    }
}

fn stage(
    id: &str,
    name: &str,
    status: RunStatus,
    depends_on: &[&str],
    jobs: Vec<PipelineJob>,
) -> PipelineJob {
    let reruns: Vec<RerunKind> = match status {
        RunStatus::Succeeded => vec![RerunKind::StageAll],
        RunStatus::Failed => vec![RerunKind::StageFailed, RerunKind::StageAll],
        RunStatus::Skipped => vec![RerunKind::StageFailed],
        _ => Vec::new(),
    };
    PipelineJob {
        id: id.to_owned(),
        name: name.to_owned(),
        status,
        started_at: jobs.iter().filter_map(|job| job.started_at).min(),
        finished_at: jobs.iter().filter_map(|job| job.finished_at).max(),
        depends_on: depends_on.iter().map(|id| (*id).to_owned()).collect(),
        reruns: reruns.into_iter().map(|kind| RerunAction::new(kind, format!("1:{id}"))).collect(),
        children: jobs,
        ..PipelineJob::default()
    }
}

/// Shaped like the user's Azure DevOps run: two stages in parallel after "Analyze", a failed
/// infrastructure stage, then skipped deployment stages and an always-running cleanup.
pub(super) fn sample_pipeline() -> Vec<PipelineJob> {
    use RunStatus::*;
    let skipped = |name: &str| job(name, Skipped, (0, 0), (0, 0), "0");
    vec![
        stage(
            "analyze",
            "Analyze",
            Succeeded,
            &[],
            vec![job("Analyze code", Succeeded, (0, 0), (0, 24), "4")],
        ),
        stage(
            "build",
            "Build",
            Succeeded,
            &["analyze"],
            vec![job("Build and Tag Images", Succeeded, (0, 30), (3, 10), "8")],
        ),
        stage(
            "infra_shared",
            "Infra Shared",
            Succeeded,
            &["analyze"],
            vec![job("Pulumi Up", Succeeded, (0, 30), (2, 2), "9")],
        ),
        stage(
            "infra_apps",
            "Infra Apps (integration environment)",
            Failed,
            &["build", "infra_shared"],
            vec![
                job("Apply infrastructure in Azure", Failed, (3, 20), (4, 1), "12"),
                job(
                    "Read outputs and check the deployment health",
                    Succeeded,
                    (3, 20),
                    (4, 48),
                    "13",
                ),
            ],
        ),
        stage(
            "deploy",
            "Deploy (integration)",
            Skipped,
            &["infra_apps"],
            ["keycloak", "frontend", "gateway", "content", "creator", "library"]
                .into_iter()
                .map(skipped)
                .collect(),
        ),
        stage(
            "pr_comment",
            "PR Comment (integration)",
            Skipped,
            &["deploy"],
            vec![skipped("Comment on the pull request")],
        ),
        stage(
            "cleanup",
            "Cleanup",
            Succeeded,
            &["deploy"],
            vec![job("Remove preview resources", Succeeded, (5, 0), (5, 30), "20")],
        ),
    ]
}

#[test]
fn stage_view_lists_every_stage_with_its_jobs() {
    let mut harness = harness(State::new(sample_pipeline()));
    harness.run();
    assert!(harness.query_by_label("Stage Infra Apps (integration environment)").is_some());
    // Every card lists its jobs from the start, as the platform's run page does.
    for job in ["Analyze code", "Build and Tag Images", "Pulumi Up", "Remove preview resources"] {
        assert!(harness.query_by_label(job).is_some(), "{job} is listed in its card");
    }
}

#[test]
fn stage_view_without_token_offers_add_a_token() {
    let mut harness = harness(State { has_token: false, ..State::new(sample_pipeline()) });
    harness.run();
    let reruns: Vec<_> = harness.query_all_by_role_and_label(Role::Button, "Retry stage").collect();
    assert!(!reruns.is_empty());
    assert!(
        reruns.iter().all(|rerun| rerun.accesskit_node().is_disabled()),
        "reruns stay visible but disabled without a token"
    );
    harness.click_at(
        harness
            .get_all_by_label("Add a token to rerun")
            .next()
            .expect("a way to add a token")
            .rect(),
    );
    harness.run();
    assert_eq!(harness.state().last.as_deref(), Some("add token"));
}

#[test]
fn stage_view_opens_logs_toggles_cards_and_reruns() {
    let mut harness = harness(State::new(sample_pipeline()));
    harness.run();
    // The failed stage opens expanded; its failed job opens the log.
    harness.click_at(harness.get_by_label("Apply infrastructure in Azure").rect());
    harness.run();
    assert_eq!(harness.state().last.as_deref(), Some("log Apply infrastructure in Azure 1:12"));
    // Every stage opens expanded; the failed stage's card is the one under its job.
    let job_x = harness.get_by_label("Apply infrastructure in Azure").rect().center().x;
    let retry = harness
        .get_all_by_label("Retry stage")
        .find(|button| button.rect().x_range().contains(job_x))
        .expect("the failed stage offers a retry")
        .rect();
    harness.click_at(retry);
    harness.run();
    assert_eq!(harness.state().last.as_deref(), Some("rerun StageFailed 1:infra_apps"));
    // A collapsed stage hides its jobs until it is expanded again.
    harness.click_at(harness.get_by_label("Collapse stage Infra Shared").rect());
    harness.run();
    assert!(harness.query_by_label("Pulumi Up").is_none());
    harness.click_at(harness.get_by_label("Expand stage Infra Shared").rect());
    harness.run();
    assert!(harness.query_by_label("Pulumi Up").is_some());
    // The header opens the stage's log.
    harness.click_at(harness.get_by_label("Stage Build").rect());
    harness.run();
    assert_eq!(harness.state().last.as_deref(), Some("log Build and Tag Images 1:8"));
    // Jobs pivot: one flat list.
    harness.click_at(harness.get_by_label("Jobs").rect());
    harness.run();
    assert!(harness.query_by_label("Job keycloak").is_some());
}

/// Shaped like a Go CLI's pipeline: every job declares `needs`, most of them `needs: []`, so the stages
/// are lanes. As the GitLab provider maps it, the stages are chained in their configured order
/// and the jobs carry their needs.
fn gitlab_pipeline() -> Vec<PipelineJob> {
    use RunStatus::*;
    let mut number = 0;
    let mut gitlab_job =
        |name: &str, status: RunStatus, start: (i64, i64), end: (i64, i64), needs: &[&str]| {
            number += 1;
            PipelineJob {
                id: format!("job:{number}"),
                log_id: Some(format!("7:{number}")),
                web_url: Some(format!("https://gitlab.example.com/acme/cli/-/jobs/{number}")),
                depends_on: needs.iter().map(|id| (*id).to_owned()).collect(),
                ..job(name, status, start, end, "")
            }
        };
    let test = vec![
        gitlab_job("lint", Succeeded, (0, 0), (1, 12), &[]),
        gitlab_job("tests:unit", Succeeded, (0, 0), (4, 40), &[]),
        gitlab_job("tests:integration", Failed, (0, 0), (6, 5), &[]),
        gitlab_job("check_docs_update", Succeeded, (0, 0), (0, 31), &[]),
    ];
    let build = vec![
        gitlab_job("build:linux", Succeeded, (0, 0), (2, 50), &[]),
        gitlab_job("build:windows", Succeeded, (0, 0), (3, 34), &[]),
        gitlab_job("build:macos", Succeeded, (0, 0), (3, 2), &[]),
    ];
    let release = vec![gitlab_job(
        "release:dry-run",
        Succeeded,
        (3, 40),
        (5, 10),
        &["job:5", "job:6", "job:7"],
    )];
    let documentation = vec![
        gitlab_job("docs:lint", Succeeded, (0, 0), (0, 48), &[]),
        gitlab_job("docs:check-links", Neutral, (0, 0), (1, 20), &[]),
    ];
    let lane =
        |name: &str, status: RunStatus, depends_on: &[&str], jobs: Vec<PipelineJob>| PipelineJob {
            reruns: Vec::new(),
            ..stage(&format!("stage:1:{name}"), name, status, depends_on, jobs)
        };
    vec![
        lane("test", Failed, &[], test),
        lane("build", Succeeded, &["stage:1:test"], build),
        lane("release", Succeeded, &["stage:1:build"], release),
        lane("documentation", Succeeded, &["stage:1:release"], documentation),
    ]
}

#[test]
fn gitlab_stages_stand_in_columns_even_when_jobs_use_needs() {
    let mut harness =
        harness(State { platform: ProviderKind::GitLab, ..State::new(gitlab_pipeline()) });
    harness.run();
    let lefts: Vec<f32> = ["test", "build", "release", "documentation"]
        .iter()
        .map(|stage| harness.get_by_label(&format!("Stage {stage}")).rect().left())
        .collect();
    assert!(
        lefts.windows(2).all(|pair| pair[1] > pair[0] + 200.0),
        "one column per stage, in order: {lefts:?}"
    );
    harness.get_by_label("release:dry-run").hover();
    harness.run_steps(60);
    assert!(
        harness
            .query_by_label_contains("Waits for: build:linux, build:windows, build:macos")
            .is_some(),
        "the expanded card names what a job needs"
    );
}

/// 60 stages of 10 jobs, all expanded: only what is on screen becomes widgets.
#[test]
fn large_pipelines_only_build_what_is_visible() {
    let stages: Vec<PipelineJob> = (0..60)
        .map(|index| {
            let id = format!("s{index}");
            let depends_on: Vec<String> =
                if index < 4 { Vec::new() } else { vec![format!("s{}", index - 4)] };
            let jobs = (0..10)
                .map(|number| {
                    job(&format!("job {index}.{number}"), RunStatus::Succeeded, (0, 0), (1, 0), "1")
                })
                .collect();
            let mut stage =
                stage(&id, &format!("Stage number {index}"), RunStatus::Succeeded, &[], jobs);
            stage.depends_on = depends_on;
            stage
        })
        .collect();
    let mut harness = harness(State { theme: ThemePreference::Dark, ..State::new(stages) });
    harness.run();
    let cards = harness.query_all_by_label_contains("Stage Stage number").count();
    let rows = harness.query_all_by_label_contains("job ").count();
    assert!(cards > 0 && cards < 24, "{cards} cards built");
    assert!(rows > 0 && rows < 120, "{rows} job rows built");
}
