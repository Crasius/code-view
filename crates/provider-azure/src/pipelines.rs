//! Pull request statuses and the builds of the pull request's merge ref, their timelines and logs.

use reqwest::Method;
use review_core::{PipelineJob, PipelineRun, ProviderError, RerunAction, RerunKind, Result, http};
use serde_json::json;
use tokio::io::AsyncWrite;

use crate::client::{Client, RepoKey};
use crate::dto::ValueList;
use crate::dto::pipeline::{Build, PullRequestStatus, latest_builds, latest_statuses};
use crate::dto::timeline::{
    Timeline, attempt_node, build_job_tree, parse_attempt_target, parse_log_id, parse_stage_target,
};
use crate::iterations;

const BUILD_PREFIX: &str = "build:";
const STATUS_PREFIX: &str = "status:";

/// Builds first (they have job trees), then posted statuses. Builds of `refs/pull/{id}/merge` are
/// the pull request's validation builds; the newest per definition is the current one.
pub(crate) async fn runs(client: &Client, repo: &str, number: u64) -> Result<Vec<PipelineRun>> {
    let key = RepoKey::parse(repo)?;
    let params = [
        ("branchName", format!("refs/pull/{number}/merge")),
        ("queryOrder", "queueTimeDescending".to_owned()),
        ("$top", "50".to_owned()),
    ];
    let builds: ValueList<Build> =
        client.get(&client.project_api(key, "build/builds"), &params).await?;
    let mut runs = latest_builds(&builds.value);
    runs.extend(statuses(client, key, number).await?);
    Ok(runs)
}

/// Pull request statuses are not readable anonymously, even on public projects (Azure DevOps
/// answers with a sign-in redirect). Without a token that 401 means "no statuses visible".
/// Statuses posted on an iteration need the latest iteration to tell current from older ones.
async fn statuses(client: &Client, repo: RepoKey<'_>, number: u64) -> Result<Vec<PipelineRun>> {
    let url = client.pull_request_api(repo, number, "statuses");
    let statuses = match client.get::<ValueList<PullRequestStatus>>(&url, &[]).await {
        Ok(statuses) => statuses.value,
        Err(error) if !client.has_token() && error.status() == Some(401) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let latest_iteration = if statuses.iter().any(|status| status.iteration_id.is_some()) {
        iterations::latest(&iterations::list(client, repo, number).await?)
    } else {
        None
    };
    Ok(latest_statuses(&statuses, latest_iteration))
}

/// Status runs have no jobs; build runs map their timeline.
pub(crate) async fn jobs(client: &Client, repo: &str, run_id: &str) -> Result<Vec<PipelineJob>> {
    if run_id.starts_with(STATUS_PREFIX) {
        return Ok(Vec::new());
    }
    let build_id: u64 =
        run_id.strip_prefix(BUILD_PREFIX).and_then(|id| id.parse().ok()).ok_or_else(|| {
            ProviderError::Local(format!("'{run_id}' is not an Azure DevOps run id."))
        })?;
    let key = RepoKey::parse(repo)?;
    let url = client.project_api(key, &format!("build/builds/{build_id}/timeline"));
    let timeline: Timeline = client.get_or_default(&url).await?;
    let web_url = client.build_web_url(key, build_id);
    Ok(build_job_tree(build_id, &timeline.records, Some(&web_url)))
}

/// An earlier attempt of a stage or job: the timeline of that attempt
/// (`GET build/builds/{buildId}/timeline/{timelineId}`,
/// <https://learn.microsoft.com/rest/api/azure/devops/build/timeline/get?view=azure-devops-rest-7.1>),
/// narrowed to the attempt's record and its children.
pub(crate) async fn attempt(client: &Client, repo: &str, target: &str) -> Result<PipelineJob> {
    let (build_id, timeline_id, record_id) = parse_attempt_target(target).ok_or_else(|| {
        ProviderError::Local(format!("'{target}' is not an Azure DevOps attempt."))
    })?;
    let key = RepoKey::parse(repo)?;
    let url = client.project_api(
        key,
        &format!("build/builds/{build_id}/timeline/{}", urlencoding::encode(timeline_id)),
    );
    let timeline: Timeline = client.get(&url, &[]).await?;
    let web_url = client.build_web_url(key, build_id);
    attempt_node(build_id, &timeline.records, record_id, Some(&web_url)).ok_or_else(|| {
        ProviderError::Local(format!(
            "Azure DevOps no longer reports attempt {record_id} of build {build_id}."
        ))
    })
}

/// Retries one stage: `PATCH build/builds/{buildId}/stages/{stageRefName}` with `state: retry`
/// (<https://learn.microsoft.com/rest/api/azure/devops/build/stages/update?view=azure-devops-rest-7.1>).
/// `forceRetryAllJobs` reruns the jobs that succeeded too.
pub(crate) async fn rerun(client: &Client, repo: &str, action: &RerunAction) -> Result<()> {
    client.require_token("Retrying a stage")?;
    let force_retry_all_jobs = match action.kind {
        RerunKind::StageAll => true,
        RerunKind::StageFailed => false,
        RerunKind::Job | RerunKind::RunFailed | RerunKind::RunAll => {
            return Err(ProviderError::Unsupported(
                "Azure DevOps reruns whole stages only.".to_owned(),
            ));
        }
    };
    let (build_id, stage) = parse_stage_target(&action.target).ok_or_else(|| {
        ProviderError::Local(format!("'{}' is not an Azure DevOps stage.", action.target))
    })?;
    let key = RepoKey::parse(repo)?;
    let url = client.project_api(
        key,
        &format!("build/builds/{build_id}/stages/{}", urlencoding::encode(stage)),
    );
    let body = json!({ "state": "retry", "forceRetryAllJobs": force_retry_all_jobs });
    client.send_unit(Method::PATCH, &url, Some(&body)).await
}

/// Streams `build/builds/{build}/logs/{log}` chunk by chunk.
pub(crate) async fn download_log(
    client: &Client,
    repo: &str,
    log_id: &str,
    sink: &mut (dyn AsyncWrite + Unpin + Send),
) -> Result<()> {
    let (build_id, log) = parse_log_id(log_id).ok_or_else(|| {
        ProviderError::Local(format!("'{log_id}' is not an Azure DevOps log id."))
    })?;
    let key = RepoKey::parse(repo)?;
    let url = client.project_api(key, &format!("build/builds/{build_id}/logs/{log}"));
    let request = client.request(Method::GET, &url).header(reqwest::header::ACCEPT, "text/plain");
    http::copy_body(client.send(request).await?, sink).await
}
