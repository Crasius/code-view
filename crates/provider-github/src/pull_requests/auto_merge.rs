//! "Enable auto-merge" and "Disable auto-merge": the GraphQL mutations `enablePullRequestAutoMerge`
//! and `disablePullRequestAutoMerge` (docs.github.com/graphql/reference/pulls), which need the pull
//! request's node id. REST has no endpoint for them.
//!
//! GitHub refuses auto-merge where the repository does not allow it, and on a pull request it can
//! merge right away; both refusals are rewritten to say what to do, the first with the repository
//! settings page where "Allow auto-merge" lives.

use review_core::{MergeOptions, ProviderError, Result};
use serde::de::IgnoredAny;
use serde_json::{Value, json};

use crate::client::GitHubClient;
use crate::graphql;
use crate::mapping::pull_request::merge_method_to_github;
use crate::pull_requests;
use crate::repo_key::RepoKey;

const ENABLE: &str = "mutation($id: ID!, $method: PullRequestMergeMethod!, $head: GitObjectID!, $headline: String, $body: String) { enablePullRequestAutoMerge(input: { pullRequestId: $id, mergeMethod: $method, expectedHeadOid: $head, commitHeadline: $headline, commitBody: $body }) { pullRequest { id } } }";
const DISABLE: &str = "mutation($id: ID!) { disablePullRequestAutoMerge(input: { pullRequestId: $id }) { pullRequest { id } } }";

/// Turns auto-merge on with the method and message the user chose, guarded by the reviewed head
/// (`expectedHeadOid`). GitHub's auto-merge cannot delete the head branch; the repository's
/// "Automatically delete head branches" setting decides that.
pub(crate) async fn enable(
    client: &GitHubClient,
    key: &str,
    number: u64,
    options: &MergeOptions,
) -> Result<()> {
    let action = "Enabling auto-merge";
    client.require_token(action)?;
    let repo = RepoKey::parse(key)?;
    let method = merge_method_to_github(options.strategy).ok_or_else(|| {
        ProviderError::Unsupported(
            "GitHub has no semi-linear merge; pick a merge commit, squash or rebase.".to_owned(),
        )
    })?;
    let pull = pull_requests::fetch(client, &repo, number).await?;
    let mut variables = json!({
        "id": pull.node_id,
        "method": method.to_uppercase(),
        "head": options.head_sha,
    });
    if let Some((headline, body)) = commit_message(options) {
        variables["headline"] = Value::from(headline);
        if !body.is_empty() {
            variables["body"] = Value::from(body);
        }
    }
    graphql::execute::<IgnoredAny>(client, action, ENABLE, variables)
        .await
        .map(drop)
        .map_err(|error| explain(error, client, &repo))
}

pub(crate) async fn disable(client: &GitHubClient, key: &str, number: u64) -> Result<()> {
    let action = "Disabling auto-merge";
    client.require_token(action)?;
    let repo = RepoKey::parse(key)?;
    let pull = pull_requests::fetch(client, &repo, number).await?;
    let _: IgnoredAny =
        graphql::execute(client, action, DISABLE, json!({ "id": pull.node_id })).await?;
    Ok(())
}

/// The first line of the message is the commit headline, the rest its body; rebasing writes no
/// commit of its own.
fn commit_message(options: &MergeOptions) -> Option<(&str, &str)> {
    let message =
        options.commit_message.as_deref().map(str::trim).filter(|message| !message.is_empty())?;
    if !options.strategy.takes_message() {
        return None;
    }
    let (headline, body) = message.split_once('\n').unwrap_or((message, ""));
    Some((headline.trim(), body.trim()))
}

/// GitHub's refusals of auto-merge, said as what to do about them. Its messages are not
/// documented; these are matched loosely and everything else stays as GitHub said it.
fn explain(error: ProviderError, client: &GitHubClient, repo: &RepoKey<'_>) -> ProviderError {
    let Some(message) = error.server_message().map(|message| message.to_lowercase()) else {
        return error;
    };
    if message.contains("auto merge is not allowed")
        || message.contains("auto-merge is not allowed")
    {
        let settings = settings_url(client, repo);
        return ProviderError::Unsupported(format!(
            "Auto-merge is turned off for {repo}. A repository admin turns on “Allow auto-merge” in its settings: {settings}"
        ));
    }
    if message.contains("clean status") {
        return ProviderError::Unsupported(
            "GitHub can merge this pull request now, so it takes no auto-merge. Merge it instead."
                .to_owned(),
        );
    }
    error
}

/// The repository's general settings, where "Allow auto-merge" is.
pub(crate) fn settings_url(client: &GitHubClient, repo: &RepoKey<'_>) -> String {
    format!("{}/{repo}/settings", client.connection().base_url())
}

#[cfg(test)]
mod tests {
    use review_core::{MergeOptions, MergeStrategy};

    use super::commit_message;

    fn options(strategy: MergeStrategy, message: &str) -> MergeOptions {
        MergeOptions {
            strategy,
            delete_source_branch: false,
            commit_message: Some(message.to_owned()),
            head_sha: "abc".to_owned(),
        }
    }

    #[test]
    fn the_message_splits_into_headline_and_body_except_for_rebase() {
        assert_eq!(
            commit_message(&options(MergeStrategy::Squash, "Title\n\nBody")),
            Some(("Title", "Body"))
        );
        assert_eq!(commit_message(&options(MergeStrategy::Merge, "  ")), None);
        assert_eq!(commit_message(&options(MergeStrategy::Rebase, "Title")), None);
    }
}
