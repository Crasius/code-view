use serde::Deserialize;
use time::OffsetDateTime;

use super::actor::{ActorDto, TeamNodeDto};
use super::label::LabelDto;
use crate::graphql::Connection;

#[derive(Debug, Deserialize)]
pub(crate) struct SearchData {
    pub search: Connection<SearchPullRequestDto>,
}

/// `search(type: ISSUE, query: "is:pr ...") { nodes { ... on PullRequest } }`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchPullRequestDto {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// OPEN, CLOSED, MERGED
    pub state: String,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
    pub head_ref_name: String,
    pub base_ref_name: String,
    #[serde(default)]
    pub author: Option<ActorDto>,
    pub repository: SearchRepositoryDto,
    #[serde(default)]
    pub labels: Option<Connection<LabelDto>>,
    #[serde(default)]
    pub review_requests: Option<Connection<ReviewRequestDto>>,
    #[serde(default)]
    pub latest_opinionated_reviews: Option<Connection<OpinionatedReviewDto>>,
    /// `commits(last: 1)`: the head commit and its check rollup.
    #[serde(default)]
    pub commits: Option<Connection<HeadCommitNodeDto>>,
    #[serde(default)]
    pub additions: Option<u64>,
    #[serde(default)]
    pub deletions: Option<u64>,
    #[serde(default)]
    pub changed_files: Option<u64>,
    /// `PullRequestReviewDecision`: `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`; null where no
    /// review is required.
    #[serde(default)]
    pub review_decision: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HeadCommitNodeDto {
    pub commit: HeadCommitDto,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HeadCommitDto {
    /// Null while the commit has no checks or statuses.
    #[serde(default)]
    pub status_check_rollup: Option<StatusCheckRollupDto>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StatusCheckRollupDto {
    /// `StatusState`: ERROR, EXPECTED, FAILURE, PENDING, SUCCESS.
    pub state: String,
    /// The first page of the check runs and commit statuses behind `state`.
    #[serde(default)]
    pub contexts: Option<ContextsDto>,
}

/// `StatusCheckRollupContextConnection`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextsDto {
    #[serde(default)]
    pub total_count: usize,
    #[serde(default)]
    pub nodes: Vec<Option<ContextDto>>,
}

/// `StatusCheckRollupContext`: a check run or a commit status. `isRequired` is only selected where
/// the query names the pull request (it takes the pull request as an argument).
#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
pub(crate) enum ContextDto {
    #[serde(rename_all = "camelCase")]
    CheckRun {
        name: String,
        /// `CheckStatusState`: `COMPLETED`, `IN_PROGRESS`, `PENDING`, `QUEUED`, `REQUESTED`, `WAITING`.
        #[serde(default)]
        status: Option<String>,
        /// `CheckConclusionState`, once completed.
        #[serde(default)]
        conclusion: Option<String>,
        #[serde(default)]
        is_required: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    StatusContext {
        context: String,
        /// `StatusState`.
        state: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        is_required: Option<bool>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchRepositoryDto {
    pub name_with_owner: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewRequestDto {
    #[serde(default)]
    pub requested_reviewer: Option<RequestedReviewerDto>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
pub(crate) enum RequestedReviewerDto {
    User(ActorDto),
    Team(TeamNodeDto),
    /// Mannequins and bots cannot be addressed as reviewers here.
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpinionatedReviewDto {
    pub state: String,
    #[serde(default)]
    pub author: Option<ActorDto>,
}
