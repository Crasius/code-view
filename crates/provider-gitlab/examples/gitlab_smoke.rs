//! Anonymous live check against gitlab.com: `cargo run -p provider-gitlab --example gitlab_smoke [project]`.
//! Uses no token; endpoints gitlab.com only serves with a token are reported, not fatal.

#![expect(clippy::print_stdout, reason = "a command-line probe that prints what it finds")]

use provider_gitlab::GitLabProvider;
use review_core::{
    Connection, Provider, ProviderKind, PullRequestQuery, PullRequestScope, StateFilter, http,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo = std::env::args().nth(1).unwrap_or_else(|| "gitlab-org/cli".to_owned());
    let connection = Connection {
        id: "smoke".to_owned(),
        name: "gitlab.com (anonymous)".to_owned(),
        kind: ProviderKind::GitLab,
        base_url: ProviderKind::GitLab.default_base_url().to_owned(),
        organizations: Vec::new(),
        token: None,
    };
    let provider = GitLabProvider::new(connection, http::build_client());

    let remote = provider.git_remote(&repo).await?;
    println!("remote: {} (auth: {})", remote.clone_url, remote.authorization_header.is_some());

    let query = PullRequestQuery {
        scope: PullRequestScope::Repository,
        state: StateFilter::Merged,
        repository_key: Some(repo.clone()),
        top: 5,
    };
    let merge_requests = provider.list_pull_requests(&query).await?;
    println!("merge requests: {}", merge_requests.len());
    for summary in &merge_requests {
        println!(
            "  !{} [{:?}] {} by {} ({} labels)",
            summary.number,
            summary.state,
            summary.title,
            summary.author.display_name,
            summary.labels.len()
        );
    }
    let Some(first) = merge_requests.first() else {
        return Ok(());
    };
    print_detail(&provider, &repo, first.number).await?;
    print_pipeline(&provider, &repo, first.number).await
}

/// Detail, reviewers and threads of one merge request.
async fn print_detail(
    provider: &GitLabProvider,
    repo: &str,
    number: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let detail = provider.pull_request(repo, number).await?;
    println!(
        "detail !{}: target {} head {} status {:?} refs {:?}",
        number, detail.target_sha, detail.head_sha, detail.merge_status, detail.git_fetch_refs
    );
    for reviewer in &detail.summary.reviewers {
        println!("  reviewer {} vote {:?}", reviewer.identity.display_name, reviewer.vote);
    }

    match provider.threads(repo, number).await {
        Ok(threads) => {
            let inline = threads.iter().filter(|thread| thread.anchor.is_some()).count();
            let system = threads.iter().filter(|thread| thread.is_system).count();
            println!("threads: {} ({inline} inline, {system} system)", threads.len());
        }
        Err(error) => println!("threads: {error}"),
    }
    Ok(())
}

/// The runs of one merge request, the stages of the first run with jobs and one job log.
async fn print_pipeline(
    provider: &GitLabProvider,
    repo: &str,
    number: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let runs = provider.pipeline_runs(repo, number).await?;
    println!("pipeline runs: {}", runs.len());
    for run in &runs {
        println!(
            "  {} {} {:?} ({})",
            run.id,
            run.name,
            run.status,
            run.description.as_deref().unwrap_or("")
        );
    }
    let Some(run) = runs.iter().find(|run| run.has_jobs) else {
        return Ok(());
    };
    let stages = provider.pipeline_jobs(repo, number, &run.id).await?;
    println!("jobs of {}:", run.id);
    for stage in &stages {
        println!("  stage {} {:?}: {} jobs", stage.name, stage.status, stage.children.len());
    }
    let log_id = stages.iter().flat_map(|stage| &stage.children).find_map(|job| job.log_id.clone());
    if let Some(log_id) = log_id {
        let mut log = Vec::new();
        match provider.download_log(repo, number, &log_id, &mut log).await {
            Ok(()) => println!("log {log_id}: {} bytes", log.len()),
            Err(error) => println!("log {log_id}: {error}"),
        }
    }
    Ok(())
}
