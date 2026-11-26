//! Pure DTO → domain mapping for pipelines, jobs and commit statuses.

// RS-DONT-04: parses provider data, so indexing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::collections::HashMap;

use review_core::{
    AttemptSource, PipelineAttempt, PipelineJob, PipelineRun, RerunAction, RerunKind, RunStatus,
    Timestamp, as_earlier_attempt, chain_sequentially,
};

use crate::dto::{CommitStatusDto, JobDto, PipelineDto};

/// How a job is scheduled, from GraphQL `CiJob.schedulingType` and `CiJob.needs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobNeeds {
    /// `dag`: the job starts when its `needs` finished, regardless of stages.
    pub is_dag: bool,
    pub needs: Vec<String>,
}

/// Pipeline and job statuses. `manual` jobs wait for a person and never start on their own, so
/// they count as finished without pass/fail meaning (`Neutral`), like jobs that failed with
/// `allow_failure` (GitLab shows those as warnings).
pub(crate) fn run_status(status: &str, allow_failure: bool) -> RunStatus {
    match status {
        "created"
        | "pending"
        | "waiting_for_resource"
        | "preparing"
        | "scheduled"
        | "waiting_for_callback" => RunStatus::Queued,
        "running" | "canceling" => RunStatus::Running,
        "success" => RunStatus::Succeeded,
        "failed" if allow_failure => RunStatus::Neutral,
        "failed" => RunStatus::Failed,
        "canceled" => RunStatus::Canceled,
        "skipped" => RunStatus::Skipped,
        _ => RunStatus::Neutral,
    }
}

/// Newest first with the merge request's `head_pipeline` leading, then other pipelines of the
/// head commit, then older ones. Merged-results and merge-train pipelines run on a merge commit
/// whose SHA differs from the head, which is why `head_pipeline` is placed explicitly and the
/// list is not filtered to the head SHA only.
///
/// `head_pipeline` and the head commit's pipelines are the head runs ([`PipelineRun::is_head`]);
/// when the merge request names neither, the newest pipeline stands for them.
pub(crate) fn pipeline_runs(
    listed: Vec<PipelineDto>,
    head_pipeline: Option<PipelineDto>,
    head_sha: Option<&str>,
    limit: usize,
) -> Vec<PipelineRun> {
    let head_id = head_pipeline.as_ref().map(|pipeline| pipeline.id);
    let (on_head, older): (Vec<PipelineDto>, Vec<PipelineDto>) = listed
        .into_iter()
        .filter(|pipeline| Some(pipeline.id) != head_id)
        .partition(|pipeline| Some(pipeline.sha.as_str()) == head_sha);
    let head_count = match (head_id, head_sha) {
        (None, None) => 1,
        _ => usize::from(head_id.is_some()) + on_head.len(),
    };
    head_pipeline
        .into_iter()
        .chain(on_head)
        .chain(older)
        .take(limit)
        .enumerate()
        .map(|(index, pipeline)| to_run(&pipeline, index < head_count))
        .collect()
}

/// A pipeline is named by its `workflow:name`, else by its kind as GitLab's pipeline list labels
/// it (<https://docs.gitlab.com/ci/pipelines/pipeline_types/>); its number goes to the line under
/// the name, where GitLab's merge request widget shows it too.
pub(crate) fn to_run(pipeline: &PipelineDto, is_head: bool) -> PipelineRun {
    let status = run_status(&pipeline.status, false);
    let name = pipeline
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| kind_name(pipeline).to_owned());
    let origin = match (pipeline.source.as_deref(), pipeline.git_ref.as_deref()) {
        (Some(source), Some(git_ref)) => Some(format!("{source} on {git_ref}")),
        (Some(source), None) => Some(source.to_owned()),
        (None, git_ref) => git_ref.map(str::to_owned),
    };
    let number = format!("#{}", pipeline.id);
    let description =
        Some(origin.map_or_else(|| number.clone(), |origin| format!("{number} · {origin}")));
    let finished_at =
        pipeline.finished_at.or_else(|| pipeline.updated_at.filter(|_| status.is_finished()));
    PipelineRun {
        id: format!("pipeline:{}:{}", pipeline.project_id, pipeline.id),
        name,
        status,
        description,
        web_url: pipeline.web_url.clone(),
        started_at: pipeline.started_at.or(pipeline.created_at),
        finished_at,
        has_jobs: true,
        is_head,
        reruns: pipeline_reruns(pipeline, status),
    }
}

/// What kind of pipeline it is: the merge request event type where GraphQL named it, else its
/// source (`CiPipelineSources`, in lower case over REST).
fn kind_name(pipeline: &PipelineDto) -> &'static str {
    match pipeline.merge_request_event_type.as_deref() {
        Some("MERGED_RESULT") => return "Merged results pipeline",
        Some("MERGE_TRAIN") => return "Merge train pipeline",
        Some("DETACHED") => return "Merge request pipeline",
        _ => {}
    }
    match pipeline.source.as_deref() {
        Some("merge_request_event") => "Merge request pipeline",
        Some("push") => "Branch pipeline",
        Some("schedule") => "Scheduled pipeline",
        Some("parent_pipeline") => "Child pipeline",
        Some("pipeline") => "Downstream pipeline",
        Some("external_pull_request_event") => "External pull request pipeline",
        _ => "Pipeline",
    }
}

/// `POST /projects/:id/pipelines/:pipeline_id/retry` retries the failed and canceled jobs.
fn pipeline_reruns(pipeline: &PipelineDto, status: RunStatus) -> Vec<RerunAction> {
    let target = format!("{}:{}", pipeline.project_id, pipeline.id);
    let has_failures = matches!(status, RunStatus::Failed | RunStatus::Canceled);
    has_failures.then(|| RerunAction::new(RerunKind::RunFailed, target)).into_iter().collect()
}

/// CI jobs appear among commit statuses too; their target URL is the job page.
pub(crate) fn is_ci_job_status(status: &CommitStatusDto) -> bool {
    status
        .target_url
        .as_deref()
        .is_some_and(|url| url.contains("/-/jobs/") || url.contains("/builds/"))
}

pub(crate) fn status_run(status: &CommitStatusDto) -> PipelineRun {
    PipelineRun {
        id: format!("status:{}", status.id),
        name: status.name.clone(),
        status: run_status(&status.status, status.allow_failure),
        description: status.description.clone().filter(|description| !description.is_empty()),
        web_url: status.target_url.clone(),
        started_at: status.started_at,
        finished_at: status.finished_at,
        has_jobs: false,
        // Read for the head commit only.
        is_head: true,
        reruns: Vec::new(),
    }
}

/// Stage nodes in pipeline order (first appearance by ascending job id, a retried job counting
/// with its first attempt) with their jobs as children.
/// Bridges (trigger jobs) have no log; their link points at the downstream pipeline.
///
/// Stages follow each other in the configured stage order, as GitLab's pipeline graph groups jobs
/// by stage by default. That order is presentation where jobs use `needs` (such a job starts once
/// the jobs it needs finished, whatever its stage), so the scheduling lives on the jobs: a job that
/// uses `needs` lists the jobs it needs in [`PipelineJob::depends_on`]. Jobs scheduled by stage, and
/// every job when GraphQL is unavailable, list none.
pub(crate) fn group_stages(
    pipeline_id: u64,
    project_id: u64,
    jobs: Vec<JobDto>,
    bridges: Vec<JobDto>,
    needs: Option<&HashMap<String, JobNeeds>>,
) -> Vec<PipelineJob> {
    let mut all: Vec<(JobDto, bool, Vec<JobDto>)> = with_attempts(jobs)
        .into_iter()
        .map(|(job, earlier)| (job, true, earlier))
        .chain(bridges.into_iter().map(|bridge| (bridge, false, Vec::new())))
        .collect();
    // A retry gets a new, higher id; the job keeps the place of its first attempt.
    all.sort_by_key(|(job, _, earlier)| earlier.first().map_or(job.id, |first| first.id));
    let mut stages: Vec<(String, Vec<PipelineJob>)> = Vec::new();
    for (job, has_log, earlier) in all {
        let mut node = job_node(&job, project_id, has_log);
        node.attempt = u32::try_from(earlier.len() + 1).unwrap_or(u32::MAX);
        node.previous_attempts = earlier
            .iter()
            .zip(1..)
            .map(|(old, number)| PipelineAttempt {
                number,
                source: AttemptSource::Loaded(Box::new(as_earlier_attempt(
                    job_node(old, project_id, true),
                    number,
                ))),
            })
            .collect();
        match stages.iter_mut().find(|(stage, _)| *stage == job.stage) {
            Some((_, children)) => children.push(node),
            None => stages.push((job.stage, vec![node])),
        }
    }
    let mut stages: Vec<PipelineJob> = stages
        .into_iter()
        .map(|(stage, children)| stage_node(pipeline_id, stage, children))
        .collect();
    chain_sequentially(&mut stages);
    if let Some(needs) = needs {
        link_needs(&mut stages, needs);
    }
    stages
}

/// Jobs listed with `include_retried=true`: a retried job keeps its name and stage, so the newest
/// (highest id) of a name is the job, the older ones its earlier attempts, oldest first. The REST
/// answer has no `retried` flag; the name is unique within a pipeline.
fn with_attempts(jobs: Vec<JobDto>) -> Vec<(JobDto, Vec<JobDto>)> {
    let mut by_name: Vec<(JobDto, Vec<JobDto>)> = Vec::new();
    let mut sorted = jobs;
    sorted.sort_by_key(|job| job.id);
    for job in sorted {
        match by_name
            .iter_mut()
            .find(|(latest, _)| latest.name == job.name && latest.stage == job.stage)
        {
            Some((latest, earlier)) => earlier.push(std::mem::replace(latest, job)),
            None => by_name.push((job, Vec::new())),
        }
    }
    by_name
}

/// Fills the `depends_on` of jobs scheduled by `needs` (`schedulingType: dag`) with the ids of the
/// jobs they need. Needed jobs missing from the pipeline (optional needs, other pipelines) are left out.
fn link_needs(stages: &mut [PipelineJob], needs: &HashMap<String, JobNeeds>) {
    let job_ids: HashMap<String, String> = stages
        .iter()
        .flat_map(|stage| &stage.children)
        .map(|job| (job.name.clone(), job.id.clone()))
        .collect();
    for job in stages.iter_mut().flat_map(|stage| &mut stage.children) {
        let Some(job_needs) = needs.get(&job.name).filter(|job_needs| job_needs.is_dag) else {
            continue;
        };
        job.depends_on =
            job_needs.needs.iter().filter_map(|name| job_ids.get(name)).cloned().collect();
    }
}

/// `POST /projects/:id/jobs/:job_id/retry` works on finished jobs that ran or were canceled;
/// bridges are not jobs of that API.
fn job_node(job: &JobDto, project_id: u64, has_log: bool) -> PipelineJob {
    let downstream_url =
        job.downstream_pipeline.as_ref().and_then(|pipeline| pipeline.web_url.clone());
    let is_retryable = has_log && matches!(job.status.as_str(), "failed" | "canceled" | "success");
    let target = format!("{project_id}:{}", job.id);
    PipelineJob {
        id: format!("job:{}", job.id),
        name: job.name.clone(),
        status: run_status(&job.status, job.allow_failure),
        started_at: job.started_at,
        finished_at: job.finished_at,
        log_id: has_log.then(|| target.clone()),
        web_url: downstream_url.or_else(|| job.web_url.clone()),
        children: Vec::new(),
        depends_on: Vec::new(),
        reruns: is_retryable
            .then(|| RerunAction::new(RerunKind::Job, target))
            .into_iter()
            .collect(),
        attempt: 1,
        previous_attempts: Vec::new(),
    }
}

fn stage_node(pipeline_id: u64, stage: String, children: Vec<PipelineJob>) -> PipelineJob {
    let started_at = children.iter().filter_map(|job| job.started_at).min();
    let all_finished = children.iter().all(|job| job.status.is_finished());
    let finished_at: Option<Timestamp> =
        if all_finished { children.iter().filter_map(|job| job.finished_at).max() } else { None };
    PipelineJob {
        id: format!("stage:{pipeline_id}:{stage}"),
        name: stage,
        status: RunStatus::combined(children.iter().map(|job| job.status)),
        started_at,
        finished_at,
        log_id: None,
        web_url: None,
        children,
        depends_on: Vec::new(),
        reruns: Vec::new(),
        attempt: 0,
        previous_attempts: Vec::new(),
    }
}

#[cfg(test)]
#[path = "pipeline_mapping_tests.rs"]
mod tests;
