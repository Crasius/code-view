//! Workflow runs, check runs and commit statuses → [`PipelineRun`]; Actions jobs and steps → [`PipelineJob`].

use std::collections::HashMap;

use review_core::{
    AttemptSource, PipelineAttempt, PipelineJob, PipelineRun, RerunAction, RerunKind, RunStatus,
    as_earlier_attempt, infer_dependencies_from_timing,
};

use crate::dto::pipeline::{CheckRunDto, CommitStatusDto, JobDto, StepDto, WorkflowRunDto};
use crate::mapping::run_status;
use crate::mapping::workflow::WorkflowGraph;

const WORKFLOW_RUN_PREFIX: &str = "run:";
const CHECK_RUN_PREFIX: &str = "check:";
const COMMIT_STATUS_PREFIX: &str = "status:";
/// Check runs of this app are the jobs of workflow runs, already listed through Actions.
const ACTIONS_APP_SLUG: &str = "github-actions";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunId {
    /// Actions workflow run: has jobs and logs.
    Workflow(u64),
    /// Check run of a non-Actions app or a commit status: no jobs.
    WithoutJobs,
}

impl RunId {
    pub fn parse(id: &str) -> Option<Self> {
        if let Some(run_id) = id.strip_prefix(WORKFLOW_RUN_PREFIX) {
            return run_id.parse().ok().map(RunId::Workflow);
        }
        let is_other = id.starts_with(CHECK_RUN_PREFIX) || id.starts_with(COMMIT_STATUS_PREFIX);
        is_other.then_some(RunId::WithoutJobs)
    }
}

/// Everything is listed for the head commit. A workflow triggered again on the same commit by the
/// same event (a label added, the pull request reopened) supersedes its earlier runs, which stay
/// listed as older runs.
pub(crate) fn runs(
    workflow_runs: Vec<WorkflowRunDto>,
    check_runs: Vec<CheckRunDto>,
    statuses: Vec<CommitStatusDto>,
) -> Vec<PipelineRun> {
    let trigger =
        |run: &WorkflowRunDto| run.workflow_id.map(|workflow| (workflow, run.event.clone()));
    let mut newest: HashMap<(u64, Option<String>), u64> = HashMap::new();
    for run in &workflow_runs {
        if let Some(key) = trigger(run) {
            let id = newest.entry(key).or_insert(run.id);
            *id = (*id).max(run.id);
        }
    }
    let workflows = workflow_runs.into_iter().map(|dto| {
        let is_head = trigger(&dto).is_none_or(|key| newest.get(&key) == Some(&dto.id));
        workflow_run(dto, is_head)
    });
    let checks = check_runs
        .into_iter()
        .filter(|check| {
            check.app.as_ref().and_then(|app| app.slug.as_deref()) != Some(ACTIONS_APP_SLUG)
        })
        .map(check_run);
    let commit_statuses = statuses.into_iter().map(commit_status);
    workflows.chain(checks).chain(commit_statuses).collect()
}

fn workflow_run(dto: WorkflowRunDto, is_head: bool) -> PipelineRun {
    let status = run_status::from_check(dto.status.as_deref(), dto.conclusion.as_deref());
    let name = dto
        .name
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("Workflow run {}", dto.id));
    let description = match (dto.display_title, dto.event) {
        (Some(title), Some(event)) => Some(format!("{title} ({event})")),
        (title, event) => title.or(event),
    };
    PipelineRun {
        id: format!("{WORKFLOW_RUN_PREFIX}{}", dto.id),
        name,
        status,
        description,
        web_url: Some(dto.html_url),
        started_at: dto.run_started_at,
        finished_at: if status.is_finished() { dto.updated_at } else { None },
        has_jobs: true,
        is_head,
        reruns: workflow_reruns(dto.id, status),
    }
}

/// `POST /actions/runs/{id}/rerun-failed-jobs` (failed and cancelled jobs plus their dependents)
/// and `POST /actions/runs/{id}/rerun`, both only once the run completed.
fn workflow_reruns(run_id: u64, status: RunStatus) -> Vec<RerunAction> {
    if !status.is_finished() {
        return Vec::new();
    }
    let has_failures = matches!(status, RunStatus::Failed | RunStatus::Canceled);
    let failed = has_failures.then(|| RerunAction::new(RerunKind::RunFailed, run_id.to_string()));
    failed.into_iter().chain([RerunAction::new(RerunKind::RunAll, run_id.to_string())]).collect()
}

fn check_run(dto: CheckRunDto) -> PipelineRun {
    PipelineRun {
        id: format!("{CHECK_RUN_PREFIX}{}", dto.id),
        name: dto.name,
        status: run_status::from_check(Some(&dto.status), dto.conclusion.as_deref()),
        description: dto.output.and_then(|output| output.title).filter(|title| !title.is_empty()),
        web_url: dto.html_url.or(dto.details_url),
        started_at: dto.started_at,
        finished_at: dto.completed_at,
        has_jobs: false,
        is_head: true,
        reruns: Vec::new(),
    }
}

fn commit_status(dto: CommitStatusDto) -> PipelineRun {
    let status = run_status::from_commit_state(&dto.state);
    PipelineRun {
        id: format!("{COMMIT_STATUS_PREFIX}{}", dto.id),
        name: dto.context,
        status,
        description: dto.description.filter(|description| !description.is_empty()),
        web_url: dto.target_url.filter(|url| !url.is_empty()),
        started_at: dto.created_at,
        finished_at: if status.is_finished() { dto.updated_at } else { None },
        has_jobs: false,
        is_head: true,
        reruns: Vec::new(),
    }
}

/// Stages are the workflow's jobs (all legs of a matrix job together), their children the Actions
/// jobs, whose children are the steps. Jobs carry the log; steps have none of their own.
///
/// With the workflow file, stages follow its `needs`. Without it (a dynamic workflow, or the file
/// could not be read) jobs are grouped by the name before ` (matrix values)` and the graph is
/// derived from start and finish times, since the jobs API reports no dependencies.
pub(crate) fn jobs(jobs: Vec<JobDto>, workflow: Option<&WorkflowGraph>) -> Vec<PipelineJob> {
    let jobs = with_attempts(jobs);
    let is_run_finished = jobs.iter().all(|(job, _)| {
        run_status::from_check(Some(&job.status), job.conclusion.as_deref()).is_finished()
    });
    let mut groups: Vec<(String, String, Vec<PipelineJob>)> = Vec::new();
    for (dto, earlier) in jobs {
        let (key, name) = match workflow.and_then(|workflow| workflow.job_for(&dto.name)) {
            Some(workflow_job) => {
                (workflow_job.key.clone(), workflow_job.display_name().to_owned())
            }
            None if workflow.is_some() => (format!("job:{}", dto.id), dto.name.clone()),
            None => {
                let base = dto.name.split(" (").next().unwrap_or(&dto.name).to_owned();
                (base.clone(), base)
            }
        };
        let mut node = job(dto, is_run_finished);
        node.previous_attempts = earlier
            .into_iter()
            .map(|old| {
                let number = old.run_attempt.unwrap_or(1);
                PipelineAttempt {
                    number,
                    source: AttemptSource::Loaded(Box::new(as_earlier_attempt(
                        job(old, false),
                        number,
                    ))),
                }
            })
            .collect();
        match groups.iter_mut().find(|(group, _, _)| *group == key) {
            Some((_, _, children)) => children.push(node),
            None => groups.push((key, name, vec![node])),
        }
    }
    if let Some(workflow) = workflow {
        let position =
            |key: &str| workflow.jobs.iter().position(|job| job.key == key).unwrap_or(usize::MAX);
        groups.sort_by_key(|(key, _, _)| position(key));
    }
    let mut stages: Vec<PipelineJob> = groups
        .into_iter()
        .map(|(key, name, children)| {
            let needs = workflow
                .and_then(|workflow| workflow.jobs.iter().find(|job| job.key == key))
                .map(|job| job.needs.iter().map(|needed| stage_id(needed)).collect())
                .unwrap_or_default();
            stage(&key, name, children, needs)
        })
        .collect();
    match workflow {
        Some(_) => {
            let ids: Vec<String> = stages.iter().map(|stage| stage.id.clone()).collect();
            for stage in &mut stages {
                stage.depends_on.retain(|needed| ids.contains(needed));
            }
        }
        None => infer_dependencies_from_timing(&mut stages),
    }
    stages
}

/// Jobs of every attempt (`filter=all`) → the jobs of the latest attempt, each with its earlier
/// executions that really ran, oldest first.
///
/// "Re-run failed jobs" copies the jobs that passed into the new attempt under new ids with the
/// times of the attempt they ran in, and skipped jobs keep their completion time; an execution
/// that finished at the same moment with the same outcome as the one after it is that copy, not
/// an attempt. A job's executions share its name.
fn with_attempts(jobs: Vec<JobDto>) -> Vec<(JobDto, Vec<JobDto>)> {
    let latest_attempt = jobs.iter().filter_map(|job| job.run_attempt).max().unwrap_or(1);
    let (latest, mut older): (Vec<JobDto>, Vec<JobDto>) = jobs
        .into_iter()
        .partition(|job| job.run_attempt.unwrap_or(latest_attempt) == latest_attempt);
    older.sort_by_key(|job| (job.run_attempt, job.id));
    let is_same_execution =
        |a: &JobDto, b: &JobDto| a.completed_at == b.completed_at && a.conclusion == b.conclusion;
    latest
        .into_iter()
        .map(|job| {
            let mut earlier: Vec<JobDto> = Vec::new();
            for old in older.extract_if(.., |old| old.name == job.name) {
                let is_copy = is_same_execution(&old, &job)
                    || earlier.last().is_some_and(|kept| is_same_execution(kept, &old));
                if !is_copy {
                    earlier.push(old);
                }
            }
            (job, earlier)
        })
        .collect()
}

fn stage_id(key: &str) -> String {
    format!("group:{key}")
}

fn stage(
    key: &str,
    name: String,
    children: Vec<PipelineJob>,
    depends_on: Vec<String>,
) -> PipelineJob {
    let all_finished = children.iter().all(|job| job.status.is_finished());
    // A stage of one job reruns as that job.
    let reruns = match children.as_slice() {
        [only] => only.reruns.clone(),
        _ => Vec::new(),
    };
    PipelineJob {
        id: stage_id(key),
        name,
        status: RunStatus::combined(children.iter().map(|job| job.status)),
        started_at: children.iter().filter_map(|job| job.started_at).min(),
        finished_at: if all_finished {
            children.iter().filter_map(|job| job.finished_at).max()
        } else {
            None
        },
        log_id: None,
        web_url: children.first().and_then(|job| job.web_url.clone()),
        children,
        depends_on,
        reruns,
        attempt: 0,
        previous_attempts: Vec::new(),
    }
}

/// `POST /actions/jobs/{id}/rerun` reruns a job and the jobs that need it, once the run completed.
fn job(dto: JobDto, is_run_finished: bool) -> PipelineJob {
    let job_id = dto.id;
    let web_url = dto.html_url;
    let children =
        dto.steps.into_iter().map(|step| step_node(job_id, web_url.as_deref(), step)).collect();
    PipelineJob {
        id: job_id.to_string(),
        name: dto.name,
        status: run_status::from_check(Some(&dto.status), dto.conclusion.as_deref()),
        started_at: dto.started_at,
        finished_at: dto.completed_at,
        log_id: Some(job_id.to_string()),
        web_url,
        children,
        depends_on: Vec::new(),
        reruns: is_run_finished
            .then(|| RerunAction::new(RerunKind::Job, job_id.to_string()))
            .into_iter()
            .collect(),
        attempt: dto.run_attempt.unwrap_or(0),
        previous_attempts: Vec::new(),
    }
}

fn step_node(job_id: u64, job_url: Option<&str>, dto: StepDto) -> PipelineJob {
    let status: RunStatus = run_status::from_check(Some(&dto.status), dto.conclusion.as_deref());
    PipelineJob {
        id: format!("{job_id}:{}", dto.number),
        name: dto.name,
        status,
        started_at: dto.started_at,
        finished_at: dto.completed_at,
        log_id: None,
        web_url: job_url.map(|url| format!("{url}#step:{}:1", dto.number)),
        children: Vec::new(),
        depends_on: Vec::new(),
        reruns: Vec::new(),
        attempt: 0,
        previous_attempts: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::pipeline::{CheckRunsPage, CombinedStatusDto, JobsPage, WorkflowRunsPage};
    use crate::mapping::fixtures::parse;

    #[test]
    fn parses_run_ids() {
        assert_eq!(RunId::parse("run:42"), Some(RunId::Workflow(42)));
        assert_eq!(RunId::parse("check:1"), Some(RunId::WithoutJobs));
        assert_eq!(RunId::parse("status:1"), Some(RunId::WithoutJobs));
        assert_eq!(RunId::parse("run:x"), None);
        assert_eq!(RunId::parse("42"), None);
    }

    #[test]
    fn merges_runs_checks_and_statuses() {
        let workflows: WorkflowRunsPage =
            parse(include_str!("../../tests/fixtures/workflow_runs.json"));
        let checks: CheckRunsPage = parse(include_str!("../../tests/fixtures/check_runs.json"));
        let statuses: CombinedStatusDto =
            parse(include_str!("../../tests/fixtures/combined_status.json"));
        let runs = runs(workflows.workflow_runs, checks.check_runs, statuses.statuses);
        let summary: Vec<(&str, RunStatus, bool)> =
            runs.iter().map(|run| (run.id.as_str(), run.status, run.has_jobs)).collect();
        assert_eq!(
            summary,
            vec![
                ("run:11", RunStatus::Succeeded, true),
                ("run:12", RunStatus::Running, true),
                ("check:22", RunStatus::Failed, false),
                ("status:33", RunStatus::Running, false),
            ]
        );
        assert_eq!(runs[0].description.as_deref(), Some("Fix the widget (pull_request)"));
        assert!(runs[0].finished_at.is_some());
        assert!(runs[1].finished_at.is_none());
        assert_eq!(runs[2].description.as_deref(), Some("2 issues"));
        assert_eq!(runs[3].web_url, None);
    }

    #[test]
    fn a_workflow_triggered_again_on_the_head_supersedes_its_earlier_runs() {
        let run = |id: u64, workflow: u64, event: &str| -> WorkflowRunDto {
            parse(&format!(
                r#"{{"id":{id},"workflow_id":{workflow},"name":"w{workflow}","status":"completed","conclusion":"success",
                    "event":"{event}","html_url":"https://github.com/o/r/actions/runs/{id}"}}"#
            ))
        };
        // Labelling the pull request twice ran CI three times; Docs ran on push and pull_request.
        let listed = vec![
            run(30, 1, "pull_request"),
            run(29, 2, "push"),
            run(28, 2, "pull_request"),
            run(20, 1, "pull_request"),
            run(10, 1, "pull_request"),
        ];
        let heads: Vec<(String, bool)> = runs(listed, Vec::new(), Vec::new())
            .into_iter()
            .map(|run| (run.id, run.is_head))
            .collect();
        let expected = [
            ("run:30", true),
            ("run:29", true),
            ("run:28", true),
            ("run:20", false),
            ("run:10", false),
        ];
        assert_eq!(heads, expected.map(|(id, is_head)| (id.to_owned(), is_head)));
    }

    #[test]
    fn jobs_have_logs_and_steps_do_not() {
        let page: JobsPage = parse(include_str!("../../tests/fixtures/jobs.json"));
        let stages = jobs(page.jobs, None);
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].name, "test", "matrix legs group under the job's name");
        assert_eq!(stages[0].reruns, vec![RerunAction::new(RerunKind::Job, "501")]);
        let jobs = &stages[0].children;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].log_id.as_deref(), Some("501"));
        assert_eq!(jobs[0].status, RunStatus::Failed);
        let steps: Vec<(&str, RunStatus, bool)> = jobs[0]
            .children
            .iter()
            .map(|step| (step.id.as_str(), step.status, step.log_id.is_none()))
            .collect();
        assert_eq!(
            steps,
            vec![
                ("501:1", RunStatus::Succeeded, true),
                ("501:2", RunStatus::Failed, true),
                ("501:3", RunStatus::Skipped, true)
            ]
        );
        assert_eq!(
            jobs[0].children[1].web_url.as_deref(),
            Some("https://github.com/contoso/app/actions/runs/11/job/501#step:2:1")
        );
    }

    fn api_job(id: u64, name: &str, conclusion: &str, started: &str, completed: &str) -> JobDto {
        parse(&format!(
            r#"{{"id":{id},"name":"{name}","status":"completed","conclusion":"{conclusion}",
                "started_at":"{started}","completed_at":"{completed}"}}"#
        ))
    }

    #[test]
    fn workflow_needs_order_and_link_the_stages() {
        let workflow = WorkflowGraph::parse(
            "jobs:\n  lint: {}\n  build:\n    name: Build\n    needs: lint\n  test:\n    needs: [build, missing]\n",
        )
        .expect("workflow");
        let api_jobs = vec![
            api_job(3, "test", "failure", "2024-05-01T10:05:00Z", "2024-05-01T10:06:00Z"),
            api_job(
                2,
                "Build (windows)",
                "success",
                "2024-05-01T10:01:00Z",
                "2024-05-01T10:04:00Z",
            ),
            api_job(1, "Build (linux)", "success", "2024-05-01T10:01:00Z", "2024-05-01T10:03:00Z"),
            api_job(4, "lint", "success", "2024-05-01T10:00:00Z", "2024-05-01T10:00:30Z"),
        ];
        let stages = jobs(api_jobs, Some(&workflow));
        let summary: Vec<(&str, &str, usize, Vec<&str>)> = stages
            .iter()
            .map(|stage| {
                let needs = stage.depends_on.iter().map(String::as_str).collect();
                (stage.id.as_str(), stage.name.as_str(), stage.children.len(), needs)
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("group:lint", "lint", 1, vec![]),
                ("group:build", "Build", 2, vec!["group:lint"]),
                ("group:test", "test", 1, vec!["group:build"]),
            ]
        );
        assert!(stages[1].reruns.is_empty(), "a matrix stage reruns per job");
        assert_eq!(stages[2].status, RunStatus::Failed);
    }

    #[test]
    fn without_a_workflow_file_timing_gives_the_graph() {
        let api_jobs = vec![
            api_job(1, "unit (linux)", "success", "2024-05-01T10:00:00Z", "2024-05-01T10:03:00Z"),
            api_job(2, "lint", "success", "2024-05-01T10:00:00Z", "2024-05-01T10:01:00Z"),
            api_job(3, "deploy", "success", "2024-05-01T10:04:00Z", "2024-05-01T10:05:00Z"),
        ];
        let stages = jobs(api_jobs, None);
        let needs: Vec<Vec<&str>> = stages
            .iter()
            .map(|stage| stage.depends_on.iter().map(String::as_str).collect())
            .collect();
        // Both finished before deploy started; timing cannot tell which one it needed.
        assert_eq!(needs, vec![vec![], vec![], vec!["group:unit", "group:lint"]]);
    }

    /// A public repository's run (fetched anonymously on 2026-10-19 with `filter=all`, renamed,
    /// trimmed): "Re-run failed jobs" made attempt 2. Two failed jobs ran again; the jobs that
    /// passed or were skipped were copied into attempt 2 with their old times.
    #[test]
    fn rerun_jobs_list_the_attempts_that_ran_and_copies_list_none() {
        let page: JobsPage = parse(include_str!("../../tests/fixtures/jobs_all_attempts.json"));
        let listed = page.jobs.len();
        let stages = jobs(page.jobs, None);
        let all: Vec<&PipelineJob> = stages.iter().flat_map(|stage| &stage.children).collect();
        assert_eq!((listed, all.len()), (8, 4), "one job per name: the latest attempt's");
        let named = |name: &str| *all.iter().find(|job| job.name == name).expect(name);
        for name in ["macOS (integration & smoke) / Browser", "macOS / Browser"] {
            let job = named(name);
            assert_eq!(
                (job.status, job.attempt_number(), job.attempt_count()),
                (RunStatus::Succeeded, 2, 2),
                "{name}"
            );
            let earlier = job.previous_attempts[0].job().expect("reported with the run");
            assert_eq!(
                (job.previous_attempts[0].number, earlier.status, earlier.attempt),
                (1, RunStatus::Failed, 1)
            );
            assert_ne!(earlier.log_id, job.log_id, "each attempt has its own log");
            assert!(earlier.reruns.is_empty(), "only the latest attempt reruns");
        }
        let earlier = named("macOS (integration & smoke) / Browser").previous_attempts[0]
            .job()
            .expect("attempt 1");
        assert_eq!(earlier.log_id.as_deref(), Some("117827729010"));
        assert!(!earlier.children.is_empty(), "with its steps");
        for name in ["Compile & Hygiene", "Copilot - Test (Windows)"] {
            let job = named(name);
            assert_eq!(
                (job.attempt_number(), job.attempt_count()),
                (2, 1),
                "{name} was copied, not run again"
            );
        }
        assert!(
            stages
                .iter()
                .all(|stage| stage.status == RunStatus::Succeeded
                    || stage.status == RunStatus::Skipped),
            "statuses of the latest attempt"
        );
    }

    #[test]
    fn finished_runs_offer_reruns() {
        let workflows: WorkflowRunsPage =
            parse(include_str!("../../tests/fixtures/workflow_runs.json"));
        let runs = runs(workflows.workflow_runs, Vec::new(), Vec::new());
        assert_eq!(runs[0].reruns, vec![RerunAction::new(RerunKind::RunAll, "11")]);
        assert!(runs[1].reruns.is_empty(), "a running run cannot be rerun");
        assert_eq!(
            workflow_reruns(5, RunStatus::Failed),
            vec![
                RerunAction::new(RerunKind::RunFailed, "5"),
                RerunAction::new(RerunKind::RunAll, "5")
            ]
        );
    }
}
