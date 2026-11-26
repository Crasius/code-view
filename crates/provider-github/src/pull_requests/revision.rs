//! Change detection for an open pull request: `GET /repos/{o}/{r}/pulls/{n}`, plus the reviews
//! list when a token is set. Both go through the conditional-request cache, so an unchanged pull
//! request costs two `304 Not Modified` answers, which GitHub does not count against the primary
//! rate limit for authorized requests. Anonymous connections skip the reviews (their 304s count).
//! The pull request's `updated_at` moves with comments, review comments, labels and pushes.

use review_core::{Fingerprint, PullRequestRevision, Result};

use crate::client::GitHubClient;
use crate::dto::pull_request::PullRequestDto;
use crate::dto::review::ReviewDto;
use crate::pull_requests;
use crate::pull_requests::reviews;
use crate::repo_key::RepoKey;

pub(crate) async fn revision(
    client: &GitHubClient,
    key: &str,
    number: u64,
) -> Result<PullRequestRevision> {
    let repo = RepoKey::parse(key)?;
    // Same URLs and headers as the detail, so the probe and the detail share one cached answer.
    let (pull, reviews) = if client.has_token() {
        tokio::try_join!(
            pull_requests::fetch(client, &repo, number),
            reviews::list(client, &repo, number)
        )?
    } else {
        (pull_requests::fetch(client, &repo, number).await?, Vec::new())
    };
    Ok(of(&pull, &reviews))
}

fn of(pull: &PullRequestDto, reviews: &[ReviewDto]) -> PullRequestRevision {
    let mut fingerprint = Fingerprint::default();
    fingerprint
        .add(pull.updated_at)
        .add(&pull.title)
        .add(&pull.body)
        .add(&pull.state)
        .add(pull.draft)
        .add(pull.merged_at)
        .add(&pull.base.sha)
        .add(&pull.mergeable_state);
    for label in &pull.labels {
        fingerprint.add(&label.name);
    }
    for user in &pull.requested_reviewers {
        fingerprint.add(user.id);
    }
    for team in &pull.requested_teams {
        fingerprint.add(team.id);
    }
    for review in reviews {
        fingerprint.add(review.id).add(&review.state);
    }
    PullRequestRevision { head_sha: pull.head.sha.clone(), marker: fingerprint.finish() }
}
