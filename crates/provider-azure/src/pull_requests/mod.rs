//! Pull request listing, detail, diff stats and the lifecycle writes (complete, auto-complete,
//! abandon, reactivate, draft, title and description). The submodules act on one pull request: reviews,
//! tags, the branch's merge-strategy policy and change detection.

pub(crate) mod labels;
pub(crate) mod merge_policy;
pub(crate) mod policies;
pub(crate) mod reviews;
pub(crate) mod revision;

use futures_util::future::try_join_all;
use reqwest::Method;
use review_core::{
    DiffStat, MergeOptions, ProviderError, PullRequestDetail, PullRequestQuery, PullRequestScope,
    PullRequestSummary, Result,
};
use serde_json::{Value, json};

use crate::client::{Client, RepoKey};
use crate::dto::ValueList;
use crate::dto::iteration::count_files;
use crate::dto::pull_request::{PullRequest, merge_strategy_to_ado, state_filter_to_ado};
use crate::dto::repository::GitRef;
use crate::identity::CurrentUsers;
use crate::iterations;

const PAGE_SIZE: usize = 100;
/// The identity id that clears `autoCompleteSetBy`, which turns auto-complete off.
pub(crate) const NO_IDENTITY: &str = "00000000-0000-0000-0000-000000000000";

pub(crate) async fn list(
    client: &Client,
    users: &CurrentUsers,
    query: &PullRequestQuery,
) -> Result<Vec<PullRequestSummary>> {
    let status = ("searchCriteria.status", state_filter_to_ado(query.state).to_owned());
    let criterion = match query.scope {
        PullRequestScope::Repository | PullRequestScope::Visible => None,
        PullRequestScope::CreatedByMe => Some("searchCriteria.creatorId"),
        PullRequestScope::ReviewRequested => Some("searchCriteria.reviewerId"),
    };
    if criterion.is_some() {
        client.require_token("Listing your pull requests")?;
    }
    let repository_key = match query.scope {
        PullRequestScope::Repository => Some(query.repository_key.as_deref().ok_or_else(|| {
            ProviderError::Local(
                "Listing a repository's pull requests needs a repository.".to_owned(),
            )
        })?),
        PullRequestScope::Visible => None,
        PullRequestScope::CreatedByMe | PullRequestScope::ReviewRequested => {
            query.repository_key.as_deref()
        }
    };
    let mut summaries = if let Some(key) = repository_key {
        let repo = RepoKey::parse(key)?;
        let url = client.repository_api(repo, "pullrequests");
        let params = listing_params(client, users, repo.organization, &status, criterion).await?;
        let pull_requests = fetch_pages(client, &url, &params, query.top).await?;
        to_summaries(client, repo.organization, &pull_requests)
    } else {
        // Organization-wide listing (`{org}/_apis/git/pullrequests`, the project segment left out):
        // one request per organization instead of one per project or repository, all at once.
        let listings = client.organizations()?.iter().map(|organization| async {
            let url = client.organization_api(organization, "git/pullrequests");
            let params = listing_params(client, users, organization, &status, criterion).await?;
            let pull_requests = fetch_pages(client, &url, &params, query.top).await?;
            Ok::<_, ProviderError>(to_summaries(client, organization, &pull_requests))
        });
        // On the calling task, not spawned, so the connection's scopes stay in force.
        try_join_all(listings).await?.into_iter().flatten().collect()
    };
    summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.number.cmp(&a.number)));
    summaries.truncate(query.top);
    Ok(summaries)
}

/// The query of a listing in `organization`: the state, and with a criterion the signed-in user.
async fn listing_params(
    client: &Client,
    users: &CurrentUsers,
    organization: &str,
    status: &(&'static str, String),
    criterion: Option<&'static str>,
) -> Result<Vec<(&'static str, String)>> {
    let mut params = vec![status.clone()];
    if let Some(criterion) = criterion {
        params.push((criterion, users.get(client, organization).await?.id));
    }
    Ok(params)
}

fn to_summaries(
    client: &Client,
    organization: &str,
    pull_requests: &[PullRequest],
) -> Vec<PullRequestSummary> {
    pull_requests
        .iter()
        .map(|pull_request| {
            pull_request
                .to_summary(organization, |key, number| client.pull_request_web_url(key, number))
        })
        .collect()
}

/// Pages with `$top`/`$skip` until `top` rows arrived or a short page ends the list.
async fn fetch_pages(
    client: &Client,
    url: &str,
    params: &[(&str, String)],
    top: usize,
) -> Result<Vec<PullRequest>> {
    let mut rows = Vec::new();
    while rows.len() < top {
        let page_size = (top - rows.len()).min(PAGE_SIZE);
        let mut page_params = params.to_vec();
        page_params.push(("$top", page_size.to_string()));
        page_params.push(("$skip", rows.len().to_string()));
        let page: ValueList<PullRequest> = client.get(url, &page_params).await?;
        let received = page.value.len();
        rows.extend(page.value);
        if received < page_size {
            break;
        }
    }
    Ok(rows)
}

/// The changed files of each pull request: the latest iteration's changes against the common
/// commit (`$compareTo` left at its default, 0), two requests per pull request below
/// [`CHANGES_PAGE_SIZE`](crate::dto::iteration::CHANGES_PAGE_SIZE) files. Azure DevOps counts
/// no lines. Iterations need sign-in even on public projects, so an anonymous connection learns
/// nothing here.
pub(crate) async fn diff_stats(
    client: &Client,
    repo: &str,
    numbers: &[u64],
) -> Result<Vec<(u64, DiffStat)>> {
    if !client.has_token() {
        return Ok(Vec::new());
    }
    let key = RepoKey::parse(repo)?;
    let mut stats = Vec::new();
    for &number in numbers {
        let Some(latest) = iterations::latest(&iterations::list(client, key, number).await?) else {
            continue;
        };
        let mut files = 0;
        iterations::changes_until(client, key, number, latest, |entries| {
            files += count_files(entries);
            None::<()>
        })
        .await?;
        stats.push((number, DiffStat { lines: None, files: Some(files) }));
    }
    Ok(stats)
}

pub(crate) async fn fetch(client: &Client, repo: RepoKey<'_>, number: u64) -> Result<PullRequest> {
    client.get(&client.pull_request_api(repo, number, ""), &[]).await
}

pub(crate) async fn detail(client: &Client, repo: &str, number: u64) -> Result<PullRequestDetail> {
    let key = RepoKey::parse(repo)?;
    let pull_request = fetch(client, key, number).await?;
    pull_request.to_detail(key.organization, |key, number| client.pull_request_web_url(key, number))
}

async fn update(client: &Client, repo: &str, number: u64, action: &str, body: Value) -> Result<()> {
    client.require_token(action)?;
    let key = RepoKey::parse(repo)?;
    client.send_unit(Method::PATCH, &client.pull_request_api(key, number, ""), Some(&body)).await
}

/// Completes against the reviewed source commit (`lastMergeSourceCommit`), so a push after the
/// review fails instead of merging unseen code.
///
/// Azure DevOps takes a push as the pull request's new source commit only once it computed the
/// merge again; until then it accepts a completion against the commit before the push (seen live:
/// 200, the pull request still active). So the source branch itself is checked first: a tip past
/// the reviewed commit is the head-moved error before anything is sent. A push the platform
/// already took is refused by it (seen live: 409, TF401192), documented with no status of its own,
/// so a refused completion is the head-moved error when the pull request's source commit is no
/// longer the reviewed one; any other refusal (a token one too) stays as the platform said it.
pub(crate) async fn merge(
    client: &Client,
    repo: &str,
    number: u64,
    options: &MergeOptions,
) -> Result<()> {
    let action = "Completing a pull request";
    client.require_token(action)?;
    let key = RepoKey::parse(repo)?;
    if source_tip(client, key, number).await?.is_some_and(|tip| tip != options.head_sha) {
        return Err(ProviderError::HeadMoved);
    }
    let Err(error) = update(client, repo, number, action, merge_body(options)).await else {
        return Ok(());
    };
    if !matches!(error.status(), Some(status) if status != 401 && status != 403) {
        return Err(error);
    }
    match fetch(client, key, number).await {
        Ok(PullRequest { last_merge_source_commit: Some(source), .. })
            if source.commit_id != options.head_sha =>
        {
            Err(ProviderError::HeadMoved)
        }
        _ => Err(error),
    }
}

/// The commit the pull request's source branch points at now (`Refs - List`, whose `filter` is a
/// prefix: the exact name is picked). `None` when the branch is not in this repository (a fork's)
/// or gone; the platform's own check then decides.
async fn source_tip(client: &Client, key: RepoKey<'_>, number: u64) -> Result<Option<String>> {
    let source = fetch(client, key, number).await?.source_ref_name;
    let Some(filter) = source.strip_prefix("refs/") else {
        return Ok(None);
    };
    let refs: ValueList<GitRef> =
        client.get(&client.repository_api(key, "refs"), &[("filter", filter.to_owned())]).await?;
    Ok(refs.value.into_iter().find(|branch| branch.name == source).map(|branch| branch.object_id))
}

/// "Set auto-complete": `autoCompleteSetBy` is the signed-in user of the organization (identity
/// ids differ per organization), `completionOptions` what the completion will use. The update
/// takes no guard commit, so a source branch pushed past the reviewed commit is refused here
/// first, as [`merge`] does.
pub(crate) async fn set_auto_complete(
    client: &Client,
    users: &CurrentUsers,
    repo: &str,
    number: u64,
    options: &MergeOptions,
) -> Result<()> {
    let action = "Setting auto-complete";
    client.require_token(action)?;
    let key = RepoKey::parse(repo)?;
    if source_tip(client, key, number).await?.is_some_and(|tip| tip != options.head_sha) {
        return Err(ProviderError::HeadMoved);
    }
    let user = users.get(client, key.organization).await?;
    let body = json!({
        "autoCompleteSetBy": { "id": user.id },
        "completionOptions": completion_options(options),
    });
    update(client, repo, number, action, body).await
}

/// "Cancel auto-complete": `autoCompleteSetBy` set to the empty identity.
pub(crate) async fn cancel_auto_complete(client: &Client, repo: &str, number: u64) -> Result<()> {
    let body = json!({ "autoCompleteSetBy": { "id": NO_IDENTITY } });
    update(client, repo, number, "Cancelling auto-complete", body).await
}

pub(crate) fn merge_body(options: &MergeOptions) -> Value {
    json!({
        "status": "completed",
        "lastMergeSourceCommit": { "commitId": options.head_sha },
        "completionOptions": completion_options(options),
    })
}

/// `GitPullRequestCompletionOptions` of a completion, now or by auto-complete.
fn completion_options(options: &MergeOptions) -> Value {
    let mut completion = json!({
        "mergeStrategy": merge_strategy_to_ado(options.strategy),
        "deleteSourceBranch": options.delete_source_branch,
    });
    if let Some(message) =
        options.commit_message.as_deref().filter(|message| !message.trim().is_empty())
    {
        completion["mergeCommitMessage"] = json!(message);
    }
    completion
}

pub(crate) async fn set_status(
    client: &Client,
    repo: &str,
    number: u64,
    status: &str,
) -> Result<()> {
    update(client, repo, number, "Changing the pull request status", json!({ "status": status }))
        .await
}

pub(crate) async fn set_draft(
    client: &Client,
    repo: &str,
    number: u64,
    is_draft: bool,
) -> Result<()> {
    update(client, repo, number, "Changing the draft state", json!({ "isDraft": is_draft })).await
}

pub(crate) async fn update_title_description(
    client: &Client,
    repo: &str,
    number: u64,
    title: &str,
    description: &str,
) -> Result<()> {
    let body = json!({ "title": title, "description": description });
    update(client, repo, number, "Editing the title and description", body).await
}

#[cfg(test)]
mod tests {
    use review_core::MergeStrategy;

    use super::*;

    #[test]
    fn builds_completion_body() {
        let options = MergeOptions {
            strategy: MergeStrategy::Squash,
            delete_source_branch: true,
            commit_message: Some("Msg".to_owned()),
            head_sha: "abc".to_owned(),
        };
        let body = merge_body(&options);
        assert_eq!(body["status"], "completed");
        assert_eq!(body["lastMergeSourceCommit"]["commitId"], "abc");
        assert_eq!(
            body["completionOptions"],
            json!({"mergeStrategy": "squash", "deleteSourceBranch": true, "mergeCommitMessage": "Msg"})
        );
        let options = MergeOptions {
            strategy: MergeStrategy::Rebase,
            delete_source_branch: false,
            commit_message: None,
            head_sha: "abc".to_owned(),
        };
        assert!(merge_body(&options)["completionOptions"].get("mergeCommitMessage").is_none());
    }
}
