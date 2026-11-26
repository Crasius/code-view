//! Change detection for an open pull request: `GET pullrequests/{id}` (source and target commit,
//! status, votes, tags, title) and `GET pullrequests/{id}/threads` (each thread's
//! `lastUpdatedDate`). Azure DevOps has no single "last updated" field on the pull request, so these
//! two requests are the cheapest complete signal; neither takes query parameters beyond
//! `api-version`.

use review_core::{Fingerprint, PullRequestRevision, Result};

use crate::client::{Client, RepoKey};
use crate::dto::ValueList;
use crate::dto::pull_request::PullRequest;
use crate::dto::thread::Thread;
use crate::pull_requests;

pub(crate) async fn revision(
    client: &Client,
    repo: &str,
    number: u64,
) -> Result<PullRequestRevision> {
    let key = RepoKey::parse(repo)?;
    let threads_url = client.pull_request_api(key, number, "threads");
    let (pull_request, threads) = tokio::try_join!(
        pull_requests::fetch(client, key, number),
        client.get::<ValueList<Thread>>(&threads_url, &[]),
    )?;
    Ok(of(&pull_request, &threads.value))
}

fn of(pull_request: &PullRequest, threads: &[Thread]) -> PullRequestRevision {
    let commit = |commit: &Option<crate::dto::pull_request::CommitRef>| {
        commit.as_ref().map(|commit| commit.commit_id.clone())
    };
    let mut fingerprint = Fingerprint::default();
    fingerprint
        .add(&pull_request.status)
        .add(&pull_request.title)
        .add(&pull_request.description)
        .add(pull_request.is_draft)
        .add(&pull_request.merge_status)
        .add(commit(&pull_request.last_merge_target_commit))
        .add(pull_request.closed_date);
    for reviewer in &pull_request.reviewers {
        fingerprint.add(&reviewer.identity.id).add(reviewer.vote).add(reviewer.is_required);
    }
    for tag in &pull_request.labels {
        fingerprint.add(&tag.name).add(tag.active);
    }
    for thread in threads {
        fingerprint
            .add(thread.id)
            .add(&thread.status)
            .add(thread.is_deleted)
            .add(thread.last_updated_date)
            .add(thread.comments.len());
        for comment in &thread.comments {
            fingerprint
                .add(comment.id)
                .add(comment.is_deleted)
                .add(comment.last_content_updated_date);
        }
    }
    PullRequestRevision {
        head_sha: commit(&pull_request.last_merge_source_commit).unwrap_or_default(),
        marker: fingerprint.finish(),
    }
}
