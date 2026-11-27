//! Merge request listing and detail, plus their DTO → domain mapping. The submodules act on one
//! merge request: its lifecycle, reviews, labels, merge policy and change detection.

pub(crate) mod labels;
pub(crate) mod lifecycle;
pub(crate) mod merge_policy;
pub(crate) mod reviews;
pub(crate) mod revision;
pub(crate) mod statuses;

use review_core::{
    AutoComplete, DiffStat, MergeStatus, MergeStrategy, ProviderError, PullRequestDetail,
    PullRequestQuery, PullRequestScope, PullRequestState, PullRequestSummary, Result, StateFilter,
};
use serde::Deserialize;
use serde_json::json;

use crate::client::{ApiClient, group_path, merge_request_path, project_path};
use crate::dto::{ApprovalsDto, MergeRequestDto, ReviewerDto, UserDto};
use crate::graphql::Nodes;

use self::labels::to_label;
use self::reviews::combine_reviewers;

pub(crate) async fn list(
    client: &ApiClient,
    organizations: &[String],
    query: &PullRequestQuery,
) -> Result<Vec<PullRequestSummary>> {
    let top = query.top.max(1);
    let base = vec![
        ("state", state_param(query.state).to_owned()),
        ("order_by", "created_at".to_owned()),
        ("sort", "desc".to_owned()),
        ("with_labels_details", "true".to_owned()),
    ];
    let with = |extra: &[(&'static str, String)]| [base.as_slice(), extra].concat();
    // Each (path, parameters) pair is one listing; together they make up the scope.
    let listings: Vec<(String, Vec<(&str, String)>)> = match query.scope {
        PullRequestScope::Repository => {
            let repo = query.repository_key.as_deref().ok_or_else(|| {
                ProviderError::Local(
                    "Listing a repository's merge requests needs a repository.".to_owned(),
                )
            })?;
            vec![(format!("{}/merge_requests", project_path(repo)), base.clone())]
        }
        // Groups bound "everything" (the group endpoint includes subgroups and needs no token
        // for public groups); without groups only what involves the user is bounded.
        PullRequestScope::Visible if !organizations.is_empty() => group_paths(organizations)
            .into_iter()
            .map(|path| (path, with(&[("scope", "all".to_owned())])))
            .collect(),
        PullRequestScope::Visible => {
            let me = client.me().await?;
            vec![
                ("/merge_requests".to_owned(), with(&[("scope", "created_by_me".to_owned())])),
                ("/merge_requests".to_owned(), with(&[("scope", "assigned_to_me".to_owned())])),
                (
                    "/merge_requests".to_owned(),
                    with(&[("scope", "all".to_owned()), ("reviewer_id", me.id.to_string())]),
                ),
            ]
        }
        PullRequestScope::CreatedByMe => {
            client.require_token("Listing your merge requests")?;
            let params = with(&[("scope", "created_by_me".to_owned())]);
            user_paths(organizations, query.repository_key.as_deref())
                .into_iter()
                .map(|path| (path, params.clone()))
                .collect()
        }
        PullRequestScope::ReviewRequested => {
            let me = client.me().await?;
            let params = with(&[("scope", "all".to_owned()), ("reviewer_id", me.id.to_string())]);
            user_paths(organizations, query.repository_key.as_deref())
                .into_iter()
                .map(|path| (path, params.clone()))
                .collect()
        }
    };
    let mut merge_requests: Vec<MergeRequestDto> = Vec::new();
    for (path, params) in &listings {
        for merge_request in client.get_pages::<MergeRequestDto>(path, params, top).await? {
            let is_known = merge_requests.iter().any(|known| {
                known.project_id == merge_request.project_id && known.iid == merge_request.iid
            });
            if !is_known {
                merge_requests.push(merge_request);
            }
        }
    }
    merge_requests.sort_by_key(|merge_request| std::cmp::Reverse(merge_request.created_at));
    merge_requests.truncate(top);
    let web_root = client.web_root();
    Ok(merge_requests
        .into_iter()
        .map(|merge_request| {
            let key = repository_key(&merge_request, web_root);
            to_summary(merge_request, key)
        })
        .collect())
}

/// Reviewers one merge request lists at most.
const MAX_REVIEWERS: usize = 100;

pub(crate) async fn detail(client: &ApiClient, repo: &str, iid: u64) -> Result<PullRequestDetail> {
    let (merge_request, reviewers, approvals) = with_votes(client, repo, iid).await?;
    to_detail(merge_request, repo.to_owned(), &reviewers, &approvals)
}

/// The merge request with its reviewers' states and its approvals, in three requests at once.
/// The detail and the revision probe both ask through here, so a reload right after the detail
/// shares its answers.
pub(crate) async fn with_votes(
    client: &ApiClient,
    repo: &str,
    iid: u64,
) -> Result<(MergeRequestDto, Vec<ReviewerDto>, ApprovalsDto)> {
    let path = merge_request_path(repo, iid);
    let reviewers_path = format!("{path}/reviewers");
    let approvals_path = format!("{path}/approvals");
    let label_details = [("with_labels_details", "true".to_owned())];
    tokio::try_join!(
        client.get::<MergeRequestDto>(&path, &label_details),
        client.get_pages::<ReviewerDto>(&reviewers_path, &[], MAX_REVIEWERS),
        client.get::<ApprovalsDto>(&approvals_path, &[]),
    )
}

/// The merge request without reviewers' votes; enough for writes that need ids, SHAs or the title.
pub(crate) async fn fetch(client: &ApiClient, repo: &str, iid: u64) -> Result<MergeRequestDto> {
    client.get(&merge_request_path(repo, iid), &[]).await
}

/// Where the user's merge requests are listed: the one project `repository_key` names, else the
/// connection's groups, else everything the token sees.
fn user_paths(organizations: &[String], repository_key: Option<&str>) -> Vec<String> {
    if let Some(repository) = repository_key {
        return vec![format!("{}/merge_requests", project_path(repository))];
    }
    if organizations.is_empty() {
        return vec!["/merge_requests".to_owned()];
    }
    group_paths(organizations)
}

fn group_paths(organizations: &[String]) -> Vec<String> {
    organizations.iter().map(|group| format!("{}/merge_requests", group_path(group))).collect()
}

fn state_param(state: StateFilter) -> &'static str {
    match state {
        StateFilter::Open => "opened",
        StateFilter::Merged => "merged",
        StateFilter::Closed => "closed",
        StateFilter::All => "all",
    }
}

/// `references.full` is `group/sub/project!iid`; older hosts lack it, so the web URL
/// (`{base}/group/sub/project/-/merge_requests/iid`) and finally the numeric project id serve.
pub(crate) fn repository_key(merge_request: &MergeRequestDto, web_root: &str) -> String {
    if let Some((key, _)) =
        merge_request.references.as_ref().and_then(|references| references.full.rsplit_once('!'))
    {
        return key.to_owned();
    }
    let from_url = merge_request
        .web_url
        .strip_prefix(web_root)
        .and_then(|rest| rest.trim_start_matches('/').split_once("/-/"))
        .map(|(key, _)| key.to_owned());
    from_url.unwrap_or_else(|| merge_request.project_id.to_string())
}

/// A merge request's `state` (`opened`, `closed`, `locked`, `merged`); `locked` is still open.
pub(crate) fn pull_request_state(state: &str) -> PullRequestState {
    match state {
        "merged" => PullRequestState::Merged,
        "closed" => PullRequestState::Closed,
        _ => PullRequestState::Open,
    }
}

pub(crate) fn to_summary(
    merge_request: MergeRequestDto,
    repository_key: String,
) -> PullRequestSummary {
    let repository_name = repository_key.rsplit('/').next().unwrap_or(&repository_key).to_owned();
    let state = pull_request_state(&merge_request.state);
    PullRequestSummary {
        repository_key,
        repository_name,
        number: merge_request.iid,
        title: merge_request.title,
        author: merge_request.author.to_identity(),
        state,
        is_draft: merge_request.draft || merge_request.work_in_progress,
        source_branch: merge_request.source_branch,
        target_branch: merge_request.target_branch,
        created_at: merge_request.created_at,
        updated_at: merge_request.updated_at,
        web_url: merge_request.web_url,
        labels: merge_request.labels.into_iter().map(to_label).collect(),
        reviewers: combine_reviewers(&[], &ApprovalsDto::default(), &merge_request.reviewers),
        // The listing has no pipeline; `head_pipeline` comes only with a single merge request.
        checks: None,
        // REST reports no line counts; `diff_stats` asks GraphQL for those of many rows at once.
        diff_stat: DiffStat::default(),
        head_sha: merge_request.sha,
        target_sha: None,
    }
}

const DIFF_STATS_QUERY: &str = "query($project: ID!, $iids: [String!]) {
  project(fullPath: $project) { mergeRequests(iids: $iids) { nodes { iid diffStatsSummary { additions deletions fileCount } } } }
}";

/// Iids one GraphQL request names at most: GitLab's largest page.
const DIFF_STATS_PAGE: usize = 100;

/// Line and file counts of the merge requests `iids` of the project `repo` from GraphQL's
/// `MergeRequest.diffStatsSummary` (<https://docs.gitlab.com/api/graphql/reference/#diffstatssummary>):
/// one request per [`DIFF_STATS_PAGE`] of them. A numeric project key (an older host without
/// `references`) has no path to ask by, so it gets nothing.
pub(crate) async fn diff_stats(
    client: &ApiClient,
    repo: &str,
    iids: &[u64],
) -> Result<Vec<(u64, DiffStat)>> {
    #[derive(Deserialize)]
    struct Data {
        project: Option<Project>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Project {
        merge_requests: Nodes<Node>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Node {
        iid: String,
        diff_stats_summary: Option<Summary>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Summary {
        additions: u64,
        deletions: u64,
        file_count: u64,
    }
    if repo.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(Vec::new());
    }
    let mut stats = Vec::new();
    for chunk in iids.chunks(DIFF_STATS_PAGE) {
        let variables = json!({ "project": repo, "iids": chunk.iter().map(u64::to_string).collect::<Vec<_>>() });
        let data: Data = client.graphql(DIFF_STATS_QUERY, variables).await?;
        let project = data.project.ok_or_else(|| {
            ProviderError::Local(format!("GitLab GraphQL does not show the project {repo}."))
        })?;
        for node in project.merge_requests.into_items() {
            if let (Ok(iid), Some(summary)) = (node.iid.parse(), node.diff_stats_summary) {
                stats.push((
                    iid,
                    DiffStat::new(summary.additions, summary.deletions, summary.file_count),
                ));
            }
        }
    }
    Ok(stats)
}

pub(crate) fn to_detail(
    mut merge_request: MergeRequestDto,
    repository_key: String,
    reviewers: &[ReviewerDto],
    approvals: &ApprovalsDto,
) -> Result<PullRequestDetail> {
    let iid = merge_request.iid;
    let head_sha = merge_request.head_sha().map(str::to_owned);
    let diff_refs = merge_request.diff_refs.take();
    let target_sha = diff_refs
        .as_ref()
        .and_then(|refs| refs.base_sha.clone().or_else(|| refs.start_sha.clone()));
    let (Some(target_sha), Some(head_sha)) = (target_sha, head_sha) else {
        return Err(ProviderError::Decode(format!(
            "GitLab has not computed the diff refs of !{iid} yet; reload in a moment."
        )));
    };
    let merge_status = merge_status(
        merge_request.detailed_merge_status.as_deref(),
        merge_request.merge_status.as_deref(),
        merge_request.has_conflicts,
    );
    let git_fetch_refs = vec![
        format!("refs/merge-requests/{iid}/head"),
        format!("refs/heads/{}", merge_request.target_branch),
    ];
    let closed_at = merge_request.merged_at.or(merge_request.closed_at);
    let description = merge_request.description.take().unwrap_or_default();
    let listed_reviewers = std::mem::take(&mut merge_request.reviewers);
    let auto_complete = auto_complete(&mut merge_request);
    let mut summary = to_summary(merge_request, repository_key);
    summary.reviewers = combine_reviewers(reviewers, approvals, &listed_reviewers);
    Ok(PullRequestDetail {
        summary,
        description,
        target_sha,
        head_sha,
        merge_status,
        git_fetch_refs,
        closed_at,
        auto_complete,
    })
}

/// The auto-merge set on the merge request. It merges with the project's merge method, squashed
/// when the merge request squashes; only the squash names a strategy here.
fn auto_complete(merge_request: &mut MergeRequestDto) -> Option<AutoComplete> {
    if !merge_request.merge_when_pipeline_succeeds {
        return None;
    }
    let set_by = merge_request.merge_user.take().or_else(|| merge_request.merged_by.take());
    let squashes = merge_request.squash_on_merge.unwrap_or(merge_request.squash);
    Some(AutoComplete {
        set_by: set_by.as_ref().map(UserDto::to_identity),
        strategy: squashes.then_some(MergeStrategy::Squash),
    })
}

/// `detailed_merge_status` (GitLab 15.6+) wins; the legacy `merge_status` covers older hosts.
pub(crate) fn merge_status(
    detailed: Option<&str>,
    legacy: Option<&str>,
    has_conflicts: bool,
) -> MergeStatus {
    if has_conflicts {
        return MergeStatus::Conflicts;
    }
    match detailed {
        Some("mergeable") => MergeStatus::Mergeable,
        Some("conflict" | "broken_status") => MergeStatus::Conflicts,
        Some("checking" | "unchecked" | "preparing" | "approvals_syncing" | "not_open") => {
            MergeStatus::Unknown
        }
        Some(_) => MergeStatus::Blocked,
        None => match legacy {
            Some("can_be_merged") => MergeStatus::Mergeable,
            Some("cannot_be_merged" | "cannot_be_merged_recheck") => MergeStatus::Conflicts,
            _ => MergeStatus::Unknown,
        },
    }
}

#[cfg(test)]
mod tests;
