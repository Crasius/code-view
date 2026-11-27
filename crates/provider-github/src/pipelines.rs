//! Actions workflow runs, third-party check runs, commit statuses, jobs and log streaming.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::Method;
use review_core::{PipelineJob, PipelineRun, ProviderError, RerunAction, RerunKind, Result, http};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::client::{GitHubClient, PER_PAGE, Paging, lock};
use crate::dto::pipeline::{
    CheckRunsPage, CombinedStatusDto, FileContentDto, JobsPage, WorkflowRunDto, WorkflowRunsPage,
};
use crate::mapping::pipelines::{self, RunId};
use crate::mapping::workflow::WorkflowGraph;
use crate::pull_requests;
use crate::repo_key::RepoKey;

const MAX_PIPELINE_PAGES: usize = 10;

/// Everything reported on the pull request's current head commit, fetched concurrently.
pub(crate) async fn runs(
    client: &GitHubClient,
    workflows: &WorkflowCache,
    key: &str,
    number: u64,
) -> Result<Vec<PipelineRun>> {
    let repo = RepoKey::parse(key)?;
    let head_sha = pull_requests::fetch(client, &repo, number).await?.head.sha;
    let per_page = ("per_page", PER_PAGE.to_string());
    let paging = Paging::pages(MAX_PIPELINE_PAGES);

    let runs_query = [("head_sha", head_sha.clone()), per_page.clone()];
    let runs_path = format!("{}/actions/runs", repo.api_path());
    let workflow_runs =
        client.collect_pages(&runs_path, &runs_query, paging, |page: WorkflowRunsPage| {
            page.workflow_runs
        });

    let commit_path = format!("{}/commits/{}", repo.api_path(), urlencoding::encode(&head_sha));
    let page_query = [per_page];
    let checks_path = format!("{commit_path}/check-runs");
    let check_runs = async {
        let pages =
            client.collect_pages(&checks_path, &page_query, paging, |page: CheckRunsPage| {
                page.check_runs
            });
        match pages.await {
            // Fine-grained tokens cannot read the checks API at all; Actions runs and
            // commit statuses still show, so the tab stays useful without them.
            Err(error) if error.status() == Some(403) => Ok(Vec::new()),
            other => other,
        }
    };
    let status_path = format!("{commit_path}/status");
    let statuses =
        client.collect_pages(&status_path, &page_query, paging, |page: CombinedStatusDto| {
            page.statuses
        });

    let (workflow_runs, check_runs, statuses) =
        tokio::try_join!(workflow_runs, check_runs, statuses)?;
    workflows.remember_files(&workflow_runs);
    Ok(pipelines::runs(workflow_runs, check_runs, statuses))
}

/// Jobs of an Actions run with the executions of its earlier attempts; check runs and commit
/// statuses have none. `filter=all` lists the jobs of every attempt in the same listing
/// (<https://docs.github.com/rest/actions/workflow-jobs#list-jobs-for-a-workflow-run>), so a run
/// that was never re-run costs what `filter=latest` costs, and attempts need no extra request.
pub(crate) async fn jobs(
    client: &GitHubClient,
    workflows: &WorkflowCache,
    key: &str,
    run_id: &str,
) -> Result<Vec<PipelineJob>> {
    let repo = RepoKey::parse(key)?;
    let run = RunId::parse(run_id)
        .ok_or_else(|| ProviderError::Local(format!("'{run_id}' is not a GitHub run id.")))?;
    let RunId::Workflow(workflow_run_id) = run else {
        return Ok(Vec::new());
    };
    let path = format!("{}/actions/runs/{workflow_run_id}/jobs", repo.api_path());
    let query = [("filter", "all".to_owned()), ("per_page", PER_PAGE.to_string())];
    let jobs =
        client.collect_pages(&path, &query, Paging::pages(MAX_PIPELINE_PAGES), |page: JobsPage| {
            page.jobs
        });
    let (jobs, workflow) = tokio::join!(jobs, workflows.get(client, &repo, workflow_run_id));
    Ok(pipelines::jobs(jobs?, workflow.as_ref()))
}

/// Workflow files by run id. A run's file at its commit never changes, so each is read once;
/// failed reads (rate limit, a dynamic workflow without a file) are retried on the next refresh.
#[derive(Default)]
pub(crate) struct WorkflowCache {
    graphs: Mutex<HashMap<u64, Option<WorkflowGraph>>>,
    /// The workflow file and commit of each listed run, so its jobs need no `GET /actions/runs/{id}`.
    files: Mutex<HashMap<u64, (String, String)>>,
}

impl WorkflowCache {
    fn remember_files(&self, runs: &[WorkflowRunDto]) {
        let mut files = lock(&self.files);
        for run in runs {
            if let (Some(path), Some(sha)) = (&run.path, &run.head_sha) {
                files.insert(run.id, (path.clone(), sha.clone()));
            }
        }
    }

    async fn get(
        &self,
        client: &GitHubClient,
        repo: &RepoKey<'_>,
        run_id: u64,
    ) -> Option<WorkflowGraph> {
        let known = lock(&self.graphs).get(&run_id).cloned();
        if let Some(known) = known {
            return known;
        }
        let file = lock(&self.files).get(&run_id).cloned();
        // The jobs API has no `needs`: without the file the stage graph comes from timing instead.
        let graph = fetch_workflow(client, repo, run_id, file).await.ok()?;
        lock(&self.graphs).insert(run_id, graph.clone());
        graph
    }
}

/// `GET /contents/{path}?ref={sha}` with the workflow path and commit of the run listing, else of
/// `GET /actions/runs/{id}`. `Ok(None)` when the file is not a parsable workflow.
async fn fetch_workflow(
    client: &GitHubClient,
    repo: &RepoKey<'_>,
    run_id: u64,
    listed: Option<(String, String)>,
) -> Result<Option<WorkflowGraph>> {
    let file = if let Some(file) = listed {
        Some(file)
    } else {
        let run: WorkflowRunDto =
            client.get_json(&format!("{}/actions/runs/{run_id}", repo.api_path())).await?;
        run.path.zip(run.head_sha)
    };
    let Some((path, sha)) = file else {
        return Ok(None);
    };
    // Some runs name the file with the ref it came from: `.github/workflows/ci.yml@refs/heads/main`.
    let path = path.split('@').next().unwrap_or(&path).to_owned();
    // Workflow files are `.yml` or `.yaml`, in lowercase as GitHub writes them.
    if !matches!(path.rsplit_once('.'), Some((_, "yml" | "yaml"))) {
        return Ok(None);
    }
    let encoded: Vec<String> =
        path.split('/').map(|segment| urlencoding::encode(segment).into_owned()).collect();
    let contents_path = format!("{}/contents/{}", repo.api_path(), encoded.join("/"));
    let file: FileContentDto =
        http::json(client.rest(Method::GET, &contents_path).query(&[("ref", sha)])).await?;
    let (Some(content), Some("base64")) = (file.content, file.encoding.as_deref()) else {
        return Ok(None);
    };
    let compact: String = content.split_whitespace().collect();
    let Ok(bytes) = STANDARD.decode(compact) else {
        return Ok(None);
    };
    Ok(String::from_utf8(bytes).ok().and_then(|source| WorkflowGraph::parse(&source)))
}

/// Re-run a job (`POST /actions/jobs/{id}/rerun`), the failed jobs of a run
/// (`POST /actions/runs/{id}/rerun-failed-jobs`) or a whole run (`POST /actions/runs/{id}/rerun`),
/// <https://docs.github.com/rest/actions/workflow-runs> and
/// <https://docs.github.com/rest/actions/workflow-jobs>. All bodies are optional and sent empty.
pub(crate) async fn rerun(client: &GitHubClient, key: &str, action: &RerunAction) -> Result<()> {
    client.require_token("Re-running jobs")?;
    let repo = RepoKey::parse(key)?;
    let id: u64 = action.target.parse().map_err(|_| {
        ProviderError::Local(format!("'{}' is not a GitHub job or run id.", action.target))
    })?;
    let path = match action.kind {
        RerunKind::Job => format!("{}/actions/jobs/{id}/rerun", repo.api_path()),
        RerunKind::RunFailed => format!("{}/actions/runs/{id}/rerun-failed-jobs", repo.api_path()),
        RerunKind::RunAll => format!("{}/actions/runs/{id}/rerun", repo.api_path()),
        RerunKind::StageAll | RerunKind::StageFailed => {
            return Err(ProviderError::Unsupported(
                "GitHub re-runs jobs and runs, not stages.".to_owned(),
            ));
        }
    };
    http::send(client.rest(Method::POST, &path)).await.map(drop)
}

/// Streams a job log chunk by chunk. GitHub redirects to blob storage; reqwest drops the
/// `Authorization` header on that cross-host hop.
pub(crate) async fn download_log(
    client: &GitHubClient,
    key: &str,
    log_id: &str,
    sink: &mut (dyn AsyncWrite + Unpin + Send),
) -> Result<()> {
    client.require_token("Downloading logs")?;
    let repo = RepoKey::parse(key)?;
    let job_id: u64 = log_id
        .parse()
        .map_err(|_| ProviderError::Local(format!("'{log_id}' is not a GitHub job id.")))?;
    let path = format!("{}/actions/jobs/{job_id}/logs", repo.api_path());
    let mut response = http::send(client.rest(Method::GET, &path)).await?;
    while let Some(chunk) = response.chunk().await? {
        sink.write_all(&chunk).await?;
    }
    sink.flush().await?;
    Ok(())
}
