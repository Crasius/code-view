//! Anonymous live check against the public `dnceng-public/public` project:
//! `cargo run -p provider-azure --example azure_smoke [-- <pull request number with builds>]`.
//! Uses no token: only reads what Azure DevOps exposes publicly.

#![expect(clippy::print_stdout, reason = "a command-line probe that prints what it finds")]

use std::pin::Pin;
use std::task::{Context, Poll};

use provider_azure::AzureDevOpsProvider;
use review_core::{
    Connection, PipelineJob, Provider, ProviderKind, PullRequestQuery, PullRequestScope,
    StateFilter, http,
};
use tokio::io::AsyncWrite;

const ORGANIZATION: &str = "dnceng-public";
const REPOSITORY: &str = "dnceng-public/public/dotnet-public-wiki";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let http = http::build_client();
    let connection = Connection {
        id: "smoke".to_owned(),
        name: "dnceng-public".to_owned(),
        kind: ProviderKind::AzureDevOps,
        base_url: ProviderKind::AzureDevOps.default_base_url().to_owned(),
        organizations: vec![ORGANIZATION.to_owned()],
        token: None,
    };
    let provider = AzureDevOpsProvider::new(connection, http.clone());

    let me = provider.current_user().await?;
    println!("current user: {} ({})", me.display_name, me.id);
    match provider.list_repositories(None).await {
        Ok(repositories) => println!("repositories: {}", repositories.len()),
        Err(error) => {
            println!("repositories (organization level, expected to need sign-in): {error}");
        }
    }
    println!("git remote: {:?}", provider.git_remote(REPOSITORY).await?);
    print_pull_requests(&provider).await?;

    let number = match std::env::args().nth(1) {
        Some(number) => number.parse()?,
        None => latest_pull_request_build(&http).await?,
    };
    print_pipeline(&provider, number).await
}

/// The newest pull requests of the repository, and threads and runs of the first one.
async fn print_pull_requests(
    provider: &AzureDevOpsProvider,
) -> Result<(), Box<dyn std::error::Error>> {
    let query = PullRequestQuery {
        scope: PullRequestScope::Repository,
        state: StateFilter::All,
        repository_key: Some(REPOSITORY.to_owned()),
        top: 5,
    };
    let pull_requests = provider.list_pull_requests(&query).await?;
    println!("pull requests in {REPOSITORY}: {}", pull_requests.len());
    for summary in &pull_requests {
        println!(
            "  !{} [{:?}] {} by {} ({} reviewers, {} labels) {}",
            summary.number,
            summary.state,
            summary.title,
            summary.author.display_name,
            summary.reviewers.len(),
            summary.labels.len(),
            summary.web_url
        );
    }
    if let Some(first) = pull_requests.first() {
        let detail = provider.pull_request(REPOSITORY, first.number).await?;
        println!(
            "detail !{}: head {} target {} merge {:?} refs {:?}",
            first.number,
            detail.head_sha,
            detail.target_sha,
            detail.merge_status,
            detail.git_fetch_refs
        );
        let threads = provider.threads(REPOSITORY, first.number).await?;
        let system = threads.iter().filter(|thread| thread.is_system).count();
        let inline = threads.iter().filter(|thread| thread.anchor.is_some()).count();
        println!("threads: {} ({system} system, {inline} inline)", threads.len());
        for thread in threads.iter().filter(|thread| !thread.is_system).take(3) {
            let body = thread.comments.first().map_or("", |comment| comment.body.as_str());
            let first_line = body.lines().next().unwrap_or_default();
            println!(
                "  #{} {:?}: {}",
                thread.id,
                thread.status,
                first_line.chars().take(80).collect::<String>()
            );
        }
        let runs = provider.pipeline_runs(REPOSITORY, first.number).await?;
        println!("pipeline runs of !{}: {}", first.number, runs.len());
    }
    Ok(())
}

/// The runs of one pull request, the job tree of the first run with jobs and one streamed log.
async fn print_pipeline(
    provider: &AzureDevOpsProvider,
    number: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let runs = provider.pipeline_runs(REPOSITORY, number).await?;
    println!("pipeline runs of refs/pull/{number}/merge: {}", runs.len());
    for run in runs.iter().take(5) {
        println!(
            "  {} {} [{:?}] {}",
            run.id,
            run.name,
            run.status,
            run.description.as_deref().unwrap_or("")
        );
    }
    let Some(run) = runs.iter().find(|run| run.has_jobs) else {
        return Ok(());
    };
    let jobs = provider.pipeline_jobs(REPOSITORY, number, &run.id).await?;
    let (nodes, with_logs) = count(&jobs);
    println!(
        "timeline of {}: {} top-level nodes, {nodes} nodes, {with_logs} with logs",
        run.id,
        jobs.len()
    );
    for stage in jobs.iter().take(3) {
        println!("  {} [{:?}] {} children", stage.name, stage.status, stage.children.len());
    }
    if let Some(log_id) = first_log(&jobs) {
        let mut sink = CountingSink::default();
        provider.download_log(REPOSITORY, number, &log_id, &mut sink).await?;
        println!("log {log_id}: {} bytes streamed", sink.bytes);
    }
    Ok(())
}

/// Finds a current pull request validation build in the project (GitHub pull requests built here).
async fn latest_pull_request_build(
    http: &reqwest::Client,
) -> Result<u64, Box<dyn std::error::Error>> {
    let url = format!("https://dev.azure.com/{ORGANIZATION}/public/_apis/build/builds");
    let request = http.get(url).query(&[
        ("api-version", "7.1"),
        ("reasonFilter", "pullRequest"),
        ("$top", "1"),
    ]);
    let builds: serde_json::Value = http::json(request).await?;
    let branch =
        builds["value"][0]["sourceBranch"].as_str().ok_or("no pull request build found")?;
    let number = branch
        .strip_prefix("refs/pull/")
        .and_then(|rest| rest.strip_suffix("/merge"))
        .ok_or("unexpected branch")?;
    Ok(number.parse()?)
}

fn count(jobs: &[PipelineJob]) -> (usize, usize) {
    jobs.iter().fold((0, 0), |(nodes, logs), job| {
        let (child_nodes, child_logs) = count(&job.children);
        (nodes + 1 + child_nodes, logs + usize::from(job.log_id.is_some()) + child_logs)
    })
}

fn first_log(jobs: &[PipelineJob]) -> Option<String> {
    jobs.iter().find_map(|job| first_log(&job.children).or_else(|| job.log_id.clone()))
}

#[derive(Default)]
struct CountingSink {
    bytes: usize,
}

impl AsyncWrite for CountingSink {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.bytes += buf.len();
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
