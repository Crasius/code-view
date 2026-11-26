//! Anonymous live check against a public repository. No token is read or used.
//!
//! `cargo run -p provider-github --example github_smoke [owner/repo]` (default `cli/cli`).
//! Costs about a dozen requests of the 60/hour anonymous budget.

#![expect(clippy::print_stdout, reason = "a command-line probe that prints what it finds")]

use provider_github::GitHubProvider;
use review_core::{
    Connection, Provider, ProviderKind, PullRequestQuery, PullRequestScope, StateFilter, http,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo = std::env::args().nth(1).unwrap_or_else(|| "cli/cli".to_owned());
    let connection = Connection {
        id: "smoke".to_owned(),
        name: "GitHub (anonymous)".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: ProviderKind::GitHub.default_base_url().to_owned(),
        organizations: Vec::new(),
        token: None,
    };
    let provider = GitHubProvider::new(connection, http::build_client());

    let query = PullRequestQuery {
        scope: PullRequestScope::Repository,
        state: StateFilter::Open,
        repository_key: Some(repo.clone()),
        top: 5,
    };
    let pulls = provider.list_pull_requests(&query).await?;
    println!("{} open pull requests (top 5) in {repo}:", pulls.len());
    for pull in &pulls {
        println!(
            "  #{} {} [{} -> {}] by {} draft={} labels={} reviewers={}",
            pull.number,
            pull.title,
            pull.source_branch,
            pull.target_branch,
            pull.author.display_name,
            pull.is_draft,
            pull.labels.len(),
            pull.reviewers.len()
        );
    }
    let Some(first) = pulls.first() else {
        return Ok(());
    };
    print_detail(&provider, &repo, first.number).await?;
    print_pipeline(&provider, &repo, first.number).await
}

/// Detail and review threads of one pull request.
async fn print_detail(
    provider: &GitHubProvider,
    repo: &str,
    number: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let detail = provider.pull_request(repo, number).await?;
    println!(
        "detail #{}: head {} target {} merge={:?} fetch={:?} reviewers={:?}",
        detail.summary.number,
        detail.head_sha,
        detail.target_sha,
        detail.merge_status,
        detail.git_fetch_refs,
        detail
            .summary
            .reviewers
            .iter()
            .map(|reviewer| (&reviewer.identity.display_name, reviewer.vote))
            .collect::<Vec<_>>()
    );

    let threads = provider.threads(repo, number).await?;
    let inline = threads.iter().filter(|thread| thread.anchor.is_some()).count();
    let comments: usize = threads.iter().map(|thread| thread.comments.len()).sum();
    println!("threads: {} ({inline} inline, {comments} comments)", threads.len());
    for thread in threads.iter().take(3) {
        let first_comment = thread.comments.first();
        println!(
            "  {} {:?} outdated={} by {} at {:?}",
            thread.id,
            thread.anchor.as_ref().map(|anchor| (&anchor.path, anchor.old_line, anchor.new_line)),
            thread.is_outdated,
            first_comment.map_or("-", |comment| comment.author.display_name.as_str()),
            first_comment.map(|comment| comment.created_at)
        );
    }
    Ok(())
}

/// The runs of one pull request and the jobs of the first run with jobs.
async fn print_pipeline(
    provider: &GitHubProvider,
    repo: &str,
    number: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let runs = provider.pipeline_runs(repo, number).await?;
    println!("pipeline runs: {}", runs.len());
    for run in runs.iter().take(8) {
        println!("  {} {} {:?} jobs={}", run.id, run.name, run.status, run.has_jobs);
    }
    if let Some(run) = runs.iter().find(|run| run.has_jobs) {
        let jobs = provider.pipeline_jobs(repo, number, &run.id).await?;
        println!("jobs of {}: {}", run.id, jobs.len());
        for job in jobs.iter().take(3) {
            println!(
                "  {} {:?} steps={} log={:?}",
                job.name,
                job.status,
                job.children.len(),
                job.log_id
            );
        }
    }
    Ok(())
}
