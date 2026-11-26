//! The checks and votes of list rows ([`review_core::Provider::row_statuses`]).
//!
//! The REST listing has neither: `head_pipeline` comes only with a single merge request, approvals
//! and reviewer states only from per-merge-request endpoints. One GraphQL request per
//! [`ROWS_PER_QUERY`] merge requests of a project asks them all at once
//! (<https://docs.gitlab.com/api/graphql/reference/#mergerequest>): the head pipeline's stages are
//! the checks, the approval rule's count is the row's required approvals (no check), and the
//! reviewers come with their review states. A numeric project key (an older host without `references`) has no path to ask by.
//!
//! Two parts are not on every host, and one unknown field fails the whole query (`crate::graphql`):
//! the approval counts are Premium fields the Community Edition lacks, and `CiStage.status` came
//! with GitLab 14.1. A host that lacks them gets the rows without the approval check, or with the
//! pipeline's own status in place of its stages.

// RS-DONT-04: parses provider data, so indexing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use review_core::{
    Check, CheckState, ProviderError, Result, ReviewVote, Reviewer, RowStatus, UserIdentity,
};
use serde::Deserialize;
use serde_json::json;

use crate::client::ApiClient;
use crate::graphql::SchemaGaps;

/// Merge requests one query names at most; the reviewers and stages of each stay small.
const ROWS_PER_QUERY: usize = 50;

/// The query for what `schema` is known to lack.
fn query(schema: &SchemaGaps) -> String {
    let approvals = if schema.has_field("MergeRequest", "approvalsRequired")
        && schema.has_field("MergeRequest", "approvalsLeft")
    {
        "approvalsRequired approvalsLeft"
    } else {
        ""
    };
    let stages =
        if schema.has_field("CiStage", "status") { "stages { nodes { name status } }" } else { "" };
    format!(
        "query($project: ID!, $iids: [String!]) {{
  project(fullPath: $project) {{ mergeRequests(iids: $iids) {{ nodes {{
    iid {approvals}
    approvedBy {{ nodes {{ id username name avatarUrl }} }}
    reviewers {{ nodes {{ id username name avatarUrl mergeRequestInteraction {{ reviewState }} }} }}
    headPipeline {{ status {stages} }}
  }} }} }}
}}"
    )
}

#[derive(Debug, Deserialize)]
struct Data {
    project: Option<Project>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    merge_requests: Nodes<MergeRequestNode>,
}

#[derive(Debug, Deserialize)]
struct Nodes<T> {
    #[serde(default = "Vec::new")]
    nodes: Vec<Option<T>>,
}

impl<T> Nodes<T> {
    fn into_items(self) -> impl Iterator<Item = T> {
        self.nodes.into_iter().flatten()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeRequestNode {
    iid: String,
    #[serde(default)]
    approvals_required: Option<u32>,
    #[serde(default)]
    approvals_left: Option<u32>,
    #[serde(default)]
    approved_by: Option<Nodes<UserNode>>,
    #[serde(default)]
    reviewers: Option<Nodes<UserNode>>,
    #[serde(default)]
    head_pipeline: Option<PipelineNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserNode {
    /// A global id: `gid://gitlab/User/42`.
    id: String,
    username: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
    #[serde(default)]
    merge_request_interaction: Option<Interaction>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Interaction {
    /// `MergeRequestReviewState`: `APPROVED`, `REQUESTED_CHANGES`, `REVIEWED`, `REVIEW_STARTED`,
    /// `UNAPPROVED`, `UNREVIEWED`.
    #[serde(default)]
    review_state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PipelineNode {
    /// `PipelineStatusEnum`, upper case.
    status: String,
    #[serde(default)]
    stages: Option<Nodes<StageNode>>,
}

#[derive(Debug, Deserialize)]
struct StageNode {
    name: String,
    /// The stage's status in lower case (`success`, `failed`, `running`, ...).
    #[serde(default)]
    status: Option<String>,
}

pub(crate) async fn row_statuses(
    client: &ApiClient,
    repo: &str,
    iids: &[u64],
) -> Result<Vec<(u64, RowStatus)>> {
    if repo.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(Vec::new());
    }
    let mut statuses = Vec::new();
    for chunk in iids.chunks(ROWS_PER_QUERY) {
        let variables = json!({ "project": repo, "iids": chunk.iter().map(u64::to_string).collect::<Vec<_>>() });
        let data: Data = client.graphql_fitted(query, variables).await?;
        let project = data.project.ok_or_else(|| {
            ProviderError::Local(format!("GitLab GraphQL does not show the project {repo}."))
        })?;
        for node in project.merge_requests.into_items() {
            if let Ok(iid) = node.iid.parse() {
                statuses.push((iid, row_status(node, client.web_root())));
            }
        }
    }
    Ok(statuses)
}

fn row_status(node: MergeRequestNode, web_root: &str) -> RowStatus {
    let mut checks = Vec::new();
    let required_approvals =
        node.approvals_required.filter(|required| *required > 0).map(|required| {
            let left = node.approvals_left.unwrap_or(required).min(required);
            let state = if left == 0 { CheckState::Passed } else { CheckState::Pending };
            let noun = if required == 1 { "approval" } else { "approvals" };
            let detail = format!("{} of {required} {noun}", required - left);
            Check::new("Approvals", state).required(Some(true)).with_detail(Some(detail))
        });
    if let Some(pipeline) = node.head_pipeline {
        let stages: Vec<StageNode> =
            pipeline.stages.map(|stages| stages.into_items().collect()).unwrap_or_default();
        if stages.is_empty() {
            checks.push(Check::new("Pipeline", status_state(&pipeline.status)));
        }
        checks.extend(stages.into_iter().map(|stage| {
            let state = stage.status.as_deref().map_or(CheckState::Neutral, status_state);
            Check::new(stage.name, state)
        }));
    }
    let reviewers = reviewers(node.reviewers, node.approved_by, web_root);
    RowStatus {
        checks,
        unlisted: 0,
        reported: None,
        reviewers: Some(reviewers),
        required_approvals,
    }
}

/// Pipeline and stage statuses; GitLab writes the pipeline's in upper case, a stage's in lower.
fn status_state(status: &str) -> CheckState {
    match status.to_ascii_lowercase().as_str() {
        "success" | "passed" => CheckState::Passed,
        "failed" => CheckState::Failed,
        "created"
        | "pending"
        | "running"
        | "preparing"
        | "scheduled"
        | "canceling"
        | "waiting_for_resource"
        | "waiting_for_callback" => CheckState::Pending,
        // skipped, canceled, manual: finished or waiting for nobody in particular.
        _ => CheckState::Neutral,
    }
}

/// Reviewers with their review states; approvers who are not reviewers are added, since an
/// approval is a vote.
fn reviewers(
    reviewers: Option<Nodes<UserNode>>,
    approved_by: Option<Nodes<UserNode>>,
    web_root: &str,
) -> Vec<Reviewer> {
    let mut voted: Vec<Reviewer> = reviewers
        .into_iter()
        .flat_map(Nodes::into_items)
        .map(|user| {
            let state = user
                .merge_request_interaction
                .as_ref()
                .and_then(|interaction| interaction.review_state.as_deref());
            let vote = match state {
                Some("APPROVED") => ReviewVote::Approved,
                Some("REQUESTED_CHANGES") => ReviewVote::ChangesRequested,
                _ => ReviewVote::None,
            };
            reviewer(&user, vote, web_root)
        })
        .collect();
    for approver in approved_by.into_iter().flat_map(Nodes::into_items) {
        let approved = reviewer(&approver, ReviewVote::Approved, web_root);
        match voted.iter_mut().find(|known| known.identity.id == approved.identity.id) {
            Some(known) => {
                known.vote = ReviewVote::Approved;
                if known.identity.avatar_url.is_none() {
                    known.identity.avatar_url = approved.identity.avatar_url;
                }
            }
            None => voted.push(approved),
        }
    }
    voted
}

fn reviewer(user: &UserNode, vote: ReviewVote, web_root: &str) -> Reviewer {
    let display_name =
        user.name.clone().filter(|name| !name.is_empty()).unwrap_or_else(|| user.username.clone());
    // GraphQL names uploaded avatars by path; REST and the avatar loader need the full address.
    let avatar_url = user.avatar_url.as_ref().map(|url| {
        if url.starts_with('/') {
            format!("{}{url}", web_root.trim_end_matches('/'))
        } else {
            url.clone()
        }
    });
    Reviewer {
        identity: UserIdentity {
            id: numeric_id(&user.id),
            display_name,
            login: Some(user.username.clone()),
            avatar_url,
        },
        vote,
        is_required: false,
        is_group: false,
    }
}

/// `gid://gitlab/User/42` → `42`, the id REST reports.
fn numeric_id(global_id: &str) -> String {
    global_id.rsplit('/').next().unwrap_or(global_id).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_stages_approvals_and_review_states() {
        let node: MergeRequestNode = serde_json::from_str(
            r#"{"iid": "4", "approvalsRequired": 2, "approvalsLeft": 1,
                "approvedBy": {"nodes": [{"id": "gid://gitlab/User/2", "username": "bob", "name": "Bob", "avatarUrl": "/uploads/bob.png"},
                                          {"id": "gid://gitlab/User/9", "username": "dee", "name": ""}]},
                "reviewers": {"nodes": [{"id": "gid://gitlab/User/1", "username": "ann", "name": "Ann", "mergeRequestInteraction": {"reviewState": "REQUESTED_CHANGES"}},
                                         {"id": "gid://gitlab/User/2", "username": "bob", "name": "Bob", "mergeRequestInteraction": {"reviewState": "UNREVIEWED"}}]},
                "headPipeline": {"status": "RUNNING", "stages": {"nodes": [{"name": "build", "status": "success"}, {"name": "test", "status": "running"}, {"name": "deploy", "status": "manual"}]}}}"#,
        )
        .expect("fixture");
        let status = row_status(node, "https://gitlab.example");
        let checks: Vec<(&str, CheckState, Option<&str>)> = status
            .checks
            .iter()
            .map(|check| (check.name.as_str(), check.state, check.detail.as_deref()))
            .collect();
        assert_eq!(
            checks,
            [
                ("build", CheckState::Passed, None),
                ("test", CheckState::Pending, None),
                ("deploy", CheckState::Neutral, None),
            ]
        );
        assert_eq!(status.verdict(), Some(CheckState::Pending));
        let approvals = status.required_approvals.as_ref().expect("the approval rule");
        assert_eq!(
            (approvals.state, approvals.detail.as_deref()),
            (CheckState::Pending, Some("1 of 2 approvals")),
            "the rule is no check"
        );
        let reviewers = status.reviewers.expect("reviewers");
        let votes: Vec<(&str, &str, ReviewVote)> = reviewers
            .iter()
            .map(|reviewer| {
                (
                    reviewer.identity.id.as_str(),
                    reviewer.identity.display_name.as_str(),
                    reviewer.vote,
                )
            })
            .collect();
        assert_eq!(
            votes,
            [
                ("1", "Ann", ReviewVote::ChangesRequested),
                ("2", "Bob", ReviewVote::Approved),
                ("9", "dee", ReviewVote::Approved)
            ],
            "an approval counts for a reviewer and adds an approver who is none"
        );
        assert_eq!(
            reviewers[1].identity.avatar_url.as_deref(),
            Some("https://gitlab.example/uploads/bob.png")
        );
    }

    #[test]
    fn a_merge_request_without_pipeline_or_rule_has_no_checks() {
        let node: MergeRequestNode =
            serde_json::from_str(r#"{"iid": "5", "approvalsRequired": 0, "headPipeline": null}"#)
                .expect("fixture");
        let status = row_status(node, "https://gitlab.example");
        assert_eq!(status.verdict(), None, "nothing configured");
        assert!(status.required_approvals.is_none(), "no rule: the votes decide");
        let failed: MergeRequestNode = serde_json::from_str(
            r#"{"iid": "6", "headPipeline": {"status": "FAILED", "stages": {"nodes": []}}}"#,
        )
        .expect("fixture");
        assert_eq!(row_status(failed, "").verdict(), Some(CheckState::Failed));
    }
}
