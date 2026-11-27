//! Pipelines and external commit statuses of a merge request, job trees and streamed job logs.
//!
//! Ids are opaque to the app and carry the project, because merge requests from forks can run
//! pipelines in the source project: runs are `pipeline:{project_id}:{pipeline_id}` or
//! `status:{status_id}`, log ids `{project_id}:{job_id}`.

use std::collections::HashMap;

use reqwest::Method;
use review_core::{PipelineJob, PipelineRun, ProviderError, RerunAction, RerunKind, Result, http};
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncWrite;

use crate::client::{ApiClient, merge_request_path};
use crate::dto::{CommitStatusDto, JobDto, PipelineDto};
use crate::graphql::{Nodes, SchemaGaps};
use crate::merge_requests::fetch;
use crate::pipeline_mapping::{
    JobNeeds, group_stages, is_ci_job_status, pipeline_runs, status_run,
};
use crate::pipeline_names;

const MAX_LISTED_PIPELINES: usize = 50;
const MAX_RUNS: usize = 10;
const MAX_STATUSES: usize = 200;
const MAX_JOBS: usize = 2_000;

pub(crate) async fn runs(client: &ApiClient, repo: &str, iid: u64) -> Result<Vec<PipelineRun>> {
    let path = format!("{}/pipelines", merge_request_path(repo, iid));
    let (mut merge_request, mut pipelines): (_, Vec<PipelineDto>) = tokio::try_join!(
        fetch(client, repo, iid),
        client.get_pages(&path, &[], MAX_LISTED_PIPELINES)
    )?;
    let mut named: Vec<&mut PipelineDto> =
        pipelines.iter_mut().chain(merge_request.head_pipeline.as_mut()).collect();
    pipeline_names::name(client, merge_request.project_id, iid, &mut named).await;
    let head_sha = merge_request.head_sha().map(str::to_owned);
    let mut runs =
        pipeline_runs(pipelines, merge_request.head_pipeline, head_sha.as_deref(), MAX_RUNS);
    // gitlab.com answers commit statuses only with a token; anonymous runs show pipelines alone.
    if let (Some(head_sha), Some(_)) = (head_sha, client.token()) {
        let project_id = merge_request.source_project_id.unwrap_or(merge_request.project_id);
        let path = format!("/projects/{project_id}/repository/commits/{head_sha}/statuses");
        let statuses: Vec<CommitStatusDto> = client.get_pages(&path, &[], MAX_STATUSES).await?;
        runs.extend(statuses.iter().filter(|status| !is_ci_job_status(status)).map(status_run));
    }
    Ok(runs)
}

pub(crate) async fn jobs(client: &ApiClient, run_id: &str) -> Result<Vec<PipelineJob>> {
    let Some((project_id, pipeline_id)) = parse_pipeline_run_id(run_id)? else {
        return Ok(Vec::new());
    };
    let base = format!("/projects/{project_id}/pipelines/{pipeline_id}");
    let jobs_path = format!("{base}/jobs");
    let bridges_path = format!("{base}/bridges");
    // Retried jobs come along in the same listing and become the earlier attempts of their job
    // (<https://docs.gitlab.com/api/jobs/#list-pipeline-jobs>, `include_retried`).
    let with_retries = [("include_retried", "true".to_owned())];
    // The needs only add the graph's edges, so they never wait out a rate limit: gitlab.com limits
    // anonymous GraphQL per IP and answers 429 with a `Retry-After` up to the next full hour.
    let (jobs, bridges, needs) = tokio::join!(
        client.get_pages::<JobDto>(&jobs_path, &with_retries, MAX_JOBS),
        client.get_pages::<JobDto>(&bridges_path, &[], MAX_JOBS),
        http::without_waiting(job_needs(client, project_id, pipeline_id)),
    );
    // The REST jobs API has no `needs`; only GraphQL does. When GraphQL is unavailable (disabled
    // or too old on a self-managed instance, or rate-limited) the jobs carry no needs; the stages
    // keep their order.
    Ok(group_stages(pipeline_id, project_id, jobs?, bridges?, needs.ok().as_ref()))
}

/// How the needs query names the pipeline.
enum PipelineKey {
    /// `Project.pipeline(id:)` takes the global id since GitLab 17.2.
    Id(String),
    /// Every release takes the iid, which only the REST pipeline names.
    Iid(String),
}

/// The needs query for what `schema` is known to lack. `Pipeline.jobs(retried:)` is newer than
/// GitLab 15.0; without it a retried job comes again under its name, with the same needs.
fn needs_query(schema: &SchemaGaps, key: &PipelineKey) -> String {
    let (variable, argument) = match key {
        PipelineKey::Id(_) => ("CiPipelineID!", "id"),
        PipelineKey::Iid(_) => ("ID!", "iid"),
    };
    let retried = if schema.accepts("jobs", "retried") { ", retried: false" } else { "" };
    format!(
        "query($project: ID!, $pipeline: {variable}, $after: String) {{
  projects(ids: [$project]) {{ nodes {{ pipeline({argument}: $pipeline) {{
    jobs(first: 100, after: $after{retried}) {{
      pageInfo {{ hasNextPage endCursor }}
      nodes {{ name schedulingType needs {{ nodes {{ name }} }} }}
    }}
  }} }} }}
}}"
    )
}

/// `CiJob.schedulingType` and `CiJob.needs` of every job, by job name
/// (<https://docs.gitlab.com/api/graphql/reference/#cijob>). The pipeline is asked by its global
/// id; a host older than GitLab 17.2 rejects that argument and is asked by the iid of the REST
/// pipeline (<https://docs.gitlab.com/api/pipelines/#get-a-single-pipeline>) from then on.
async fn job_needs(
    client: &ApiClient,
    project_id: u64,
    pipeline_id: u64,
) -> Result<HashMap<String, JobNeeds>> {
    #[derive(Deserialize)]
    struct Pipeline {
        iid: u64,
    }
    if client.schema_gaps().accepts("pipeline", "id") {
        let key = PipelineKey::Id(format!("gid://gitlab/Ci::Pipeline/{pipeline_id}"));
        match needs_of(client, project_id, pipeline_id, &key).await {
            Err(_) if !client.schema_gaps().accepts("pipeline", "id") => {}
            answer => return answer,
        }
    }
    let path = format!("/projects/{project_id}/pipelines/{pipeline_id}");
    let pipeline: Pipeline = client.get(&path, &[]).await?;
    needs_of(client, project_id, pipeline_id, &PipelineKey::Iid(pipeline.iid.to_string())).await
}

async fn needs_of(
    client: &ApiClient,
    project_id: u64,
    pipeline_id: u64,
    key: &PipelineKey,
) -> Result<HashMap<String, JobNeeds>> {
    let (PipelineKey::Id(pipeline) | PipelineKey::Iid(pipeline)) = key;
    let mut needs = HashMap::new();
    let mut after: Option<String> = None;
    while needs.len() < MAX_JOBS {
        let variables = json!({
            "project": format!("gid://gitlab/Project/{project_id}"),
            "pipeline": pipeline,
            "after": after,
        });
        let data: NeedsData =
            client.graphql_fitted(|schema| needs_query(schema, key), variables).await?;
        let Some(jobs) = data
            .projects
            .into_items()
            .next()
            .and_then(|project| project.pipeline)
            .map(|pipeline| pipeline.jobs)
        else {
            return Err(ProviderError::Local(format!(
                "GitLab GraphQL does not show pipeline {pipeline_id}."
            )));
        };
        for job in jobs.nodes {
            let job_needs = JobNeeds {
                is_dag: job.scheduling_type.as_deref() == Some("dag"),
                needs: job.needs.into_items().map(|need| need.name).collect(),
            };
            needs.insert(job.name, job_needs);
        }
        match jobs.page_info {
            PageInfo { has_next_page: true, end_cursor: Some(cursor) } => after = Some(cursor),
            _ => break,
        }
    }
    Ok(needs)
}

#[derive(Deserialize)]
struct NeedsData {
    projects: Nodes<ProjectNode>,
}

#[derive(Deserialize)]
struct ProjectNode {
    pipeline: Option<PipelineNode>,
}

#[derive(Deserialize)]
struct PipelineNode {
    jobs: JobConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobConnection {
    page_info: PageInfo,
    nodes: Vec<JobNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobNode {
    name: String,
    scheduling_type: Option<String>,
    needs: Nodes<NeedNode>,
}

#[derive(Deserialize)]
struct NeedNode {
    name: String,
}

/// Retry a job (`POST /projects/:id/jobs/:job_id/retry`, <https://docs.gitlab.com/api/jobs/#retry-a-job>)
/// or the failed and canceled jobs of a pipeline (`POST /projects/:id/pipelines/:pipeline_id/retry`,
/// <https://docs.gitlab.com/api/pipelines/#retry-jobs-in-a-pipeline>). Neither takes a body.
pub(crate) async fn rerun(client: &ApiClient, action: &RerunAction) -> Result<()> {
    client.require_token("Retrying jobs")?;
    let (project_id, id) = parse_id_pair(&action.target).ok_or_else(|| {
        ProviderError::Local(format!("'{}' is not a GitLab job or pipeline.", action.target))
    })?;
    let path = match action.kind {
        RerunKind::Job => format!("/projects/{project_id}/jobs/{id}/retry"),
        RerunKind::RunFailed => format!("/projects/{project_id}/pipelines/{id}/retry"),
        RerunKind::StageAll | RerunKind::StageFailed | RerunKind::RunAll => {
            return Err(ProviderError::Unsupported(
                "GitLab retries jobs and pipelines, not stages.".to_owned(),
            ));
        }
    };
    client.unit(client.request(Method::POST, &path)).await
}

/// Streams `/jobs/:id/trace` chunk by chunk into `sink`.
pub(crate) async fn download_log(
    client: &ApiClient,
    log_id: &str,
    sink: &mut (dyn AsyncWrite + Unpin + Send),
) -> Result<()> {
    let (project_id, job_id) = parse_log_id(log_id)?;
    let request =
        client.request(Method::GET, &format!("/projects/{project_id}/jobs/{job_id}/trace"));
    http::copy_body(client.send(request).await?, sink).await
}

/// `Some((project, pipeline))` for pipeline runs, `None` for external statuses (no jobs).
pub(crate) fn parse_pipeline_run_id(run_id: &str) -> Result<Option<(u64, u64)>> {
    if run_id.starts_with("status:") {
        return Ok(None);
    }
    run_id
        .strip_prefix("pipeline:")
        .and_then(parse_id_pair)
        .map(Some)
        .ok_or_else(|| ProviderError::Local(format!("'{run_id}' is not a GitLab pipeline run id.")))
}

fn parse_log_id(log_id: &str) -> Result<(u64, u64)> {
    parse_id_pair(log_id)
        .ok_or_else(|| ProviderError::Local(format!("'{log_id}' is not a GitLab job log id.")))
}

fn parse_id_pair(value: &str) -> Option<(u64, u64)> {
    let (left, right) = value.split_once(':')?;
    Some((left.parse().ok()?, right.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_run_and_log_ids() {
        assert_eq!(
            parse_pipeline_run_id("pipeline:34675721:2953380535").expect("run"),
            Some((34_675_721, 2_953_380_535))
        );
        assert_eq!(parse_pipeline_run_id("status:9").expect("status"), None);
        assert!(parse_pipeline_run_id("pipeline:x").is_err());
        assert_eq!(parse_log_id("1:2").expect("log"), (1, 2));
        assert!(parse_log_id("17220013682").is_err());
    }

    #[test]
    fn a_null_need_is_skipped() {
        let job: JobNode = serde_json::from_str(
            r#"{"name": "lint", "schedulingType": "dag", "needs": {"nodes": [null, {"name": "compile"}]}}"#,
        )
        .expect("a null need parses");
        let needs: Vec<String> = job.needs.into_items().map(|need| need.name).collect();
        assert_eq!(needs, ["compile"]);
    }
}
