//! Merge, close, reopen, draft toggling and title/description edits.

use reqwest::Method;
use review_core::{MergeOptions, ProviderError, Result, http};
use serde::de::IgnoredAny;
use serde_json::{Value, json};

use crate::client::GitHubClient;
use crate::graphql;
use crate::mapping::pull_request::{github_method, head_is_in_base_repository};
use crate::pull_requests;
use crate::repo_key::{RepoKey, encode_branch};

const CONVERT_TO_DRAFT: &str = "mutation($id: ID!) { convertPullRequestToDraft(input: { pullRequestId: $id }) { pullRequest { id } } }";
const MARK_READY: &str = "mutation($id: ID!) { markPullRequestReadyForReview(input: { pullRequestId: $id }) { pullRequest { id } } }";

/// Merges only while the head is still the commit the user reviewed (`sha`, answered with 409
/// when it moved), then deletes the head branch when asked and it is not a fork. Right after a
/// push GitHub may answer 405 "not mergeable" instead (seen live: it is still computing the merge
/// of the new head), so a 405 while the head is no longer the reviewed one is the head-moved
/// error too.
pub(crate) async fn merge(
    client: &GitHubClient,
    key: &str,
    number: u64,
    options: &MergeOptions,
) -> Result<()> {
    client.require_token("Merging")?;
    let repo = RepoKey::parse(key)?;
    let method = github_method(options.strategy)?;
    // Only the branch name and the fork check come from here; the guard is the reviewed head.
    let pull = if options.delete_source_branch {
        Some(pull_requests::fetch(client, &repo, number).await?)
    } else {
        None
    };
    let body = merge_body(options, method);
    let request =
        client.rest(Method::PUT, &format!("{}/merge", repo.pull_path(number))).json(&body);
    match http::unit(request).await {
        Err(ProviderError::Status { status: 409, .. }) => return Err(ProviderError::HeadMoved),
        Err(error @ ProviderError::Status { status: 405, .. }) => {
            let head = pull_requests::fetch(client, &repo, number).await.map(|pull| pull.head.sha);
            let has_moved = head.is_ok_and(|sha| sha != options.head_sha);
            return Err(if has_moved { ProviderError::HeadMoved } else { error });
        }
        result => result?,
    }
    if let Some(pull) = pull.filter(head_is_in_base_repository) {
        delete_branch(client, &repo, &pull.head.name).await?;
    }
    Ok(())
}

/// `DELETE /git/refs/heads/{branch}`. A repository that deletes head branches on merge may have
/// removed it already; the docs do not say how the delete answers then, so a failed delete counts
/// only while `GET /git/ref/heads/{branch}` still finds the branch (404 once it is gone).
async fn delete_branch(client: &GitHubClient, repo: &RepoKey<'_>, branch: &str) -> Result<()> {
    let encoded = encode_branch(branch);
    let deleted = http::unit(
        client.rest(Method::DELETE, &format!("{}/git/refs/heads/{encoded}", repo.api_path())),
    )
    .await;
    let Err(error) = deleted else {
        return Ok(());
    };
    let lookup = format!("{}/git/ref/heads/{encoded}", repo.api_path());
    match http::unit(client.rest(Method::GET, &lookup)).await {
        Err(ProviderError::Status { status: 404, .. }) => Ok(()),
        _ => Err(error),
    }
}

fn merge_body(options: &MergeOptions, method: &str) -> Value {
    let mut body = json!({ "merge_method": method, "sha": options.head_sha });
    if let Some((title, message)) = pull_requests::commit_message(options) {
        body["commit_title"] = Value::from(title);
        if !message.is_empty() {
            body["commit_message"] = Value::from(message);
        }
    }
    body
}

pub(crate) async fn close(client: &GitHubClient, key: &str, number: u64) -> Result<()> {
    client.require_token("Changing the pull request state")?;
    patch(client, key, number, json!({ "state": "closed" })).await
}

pub(crate) async fn reopen(client: &GitHubClient, key: &str, number: u64) -> Result<()> {
    client.require_token("Changing the pull request state")?;
    patch(client, key, number, json!({ "state": "open" })).await
}

pub(crate) async fn update_title_description(
    client: &GitHubClient,
    key: &str,
    number: u64,
    title: &str,
    description: &str,
) -> Result<()> {
    client.require_token("Editing the pull request")?;
    patch(client, key, number, json!({ "title": title, "body": description })).await
}

async fn patch(client: &GitHubClient, key: &str, number: u64, body: Value) -> Result<()> {
    let repo = RepoKey::parse(key)?;
    http::unit(client.rest(Method::PATCH, &repo.pull_path(number)).json(&body)).await
}

/// Drafts toggle only through GraphQL, which needs the pull request's node id.
pub(crate) async fn set_draft(
    client: &GitHubClient,
    key: &str,
    number: u64,
    is_draft: bool,
) -> Result<()> {
    client.require_token("Changing the draft state")?;
    let repo = RepoKey::parse(key)?;
    let pull = pull_requests::fetch(client, &repo, number).await?;
    if pull.draft == is_draft {
        return Ok(());
    }
    let mutation = if is_draft { CONVERT_TO_DRAFT } else { MARK_READY };
    let _: IgnoredAny = graphql::execute(
        client,
        "Changing the draft state",
        mutation,
        json!({ "id": pull.node_id }),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::merge_body;
    use review_core::{MergeOptions, MergeStrategy};
    use serde_json::json;

    fn options(strategy: MergeStrategy, message: Option<&str>) -> MergeOptions {
        MergeOptions {
            strategy,
            delete_source_branch: false,
            commit_message: message.map(str::to_owned),
            head_sha: "abc".to_owned(),
        }
    }

    #[test]
    fn merge_body_sends_title_and_message_only_when_given() {
        let body =
            merge_body(&options(MergeStrategy::Squash, Some("Title\n\nBody line")), "squash");
        assert_eq!(
            body,
            json!({ "merge_method": "squash", "sha": "abc", "commit_title": "Title", "commit_message": "Body line" })
        );
        let title_only = merge_body(&options(MergeStrategy::Merge, Some("Title")), "merge");
        assert_eq!(
            title_only,
            json!({ "merge_method": "merge", "sha": "abc", "commit_title": "Title" })
        );
        let none = merge_body(&options(MergeStrategy::Merge, None), "merge");
        assert_eq!(none, json!({ "merge_method": "merge", "sha": "abc" }));
    }
}
