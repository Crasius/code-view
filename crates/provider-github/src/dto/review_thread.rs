use serde::Deserialize;
use time::OffsetDateTime;

use super::actor::ActorDto;
use crate::graphql::Connection;

/// `repository { pullRequest { ... } }` wrapper.
#[derive(Debug, Deserialize)]
pub(crate) struct RepositoryData<T> {
    pub repository: Option<PullRequestData<T>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRequestData<T> {
    pub pull_request: Option<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewThreadsField {
    pub review_threads: Connection<ReviewThreadDto>,
}

/// `node(id:) { ... on PullRequestReviewThread { comments } }`
#[derive(Debug, Deserialize)]
pub(crate) struct NodeData<T> {
    pub node: Option<T>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ThreadCommentsField {
    pub comments: Connection<ThreadCommentDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewThreadDto {
    pub id: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub path: String,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub original_line: Option<u32>,
    /// `LEFT` or `RIGHT`.
    #[serde(default)]
    pub diff_side: Option<String>,
    /// First line of a multi-line thread; `null` for a single line.
    #[serde(default)]
    pub start_line: Option<u32>,
    #[serde(default)]
    pub original_start_line: Option<u32>,
    #[serde(default)]
    pub start_diff_side: Option<String>,
    pub comments: Connection<ThreadCommentDto>,
    /// The first comment again, with what it was written on (`origin: comments(first: 1)`).
    #[serde(default)]
    pub origin: Option<Connection<OriginCommentDto>>,
}

/// Where a review comment was written: the commit, its hunk and its page.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OriginCommentDto {
    #[serde(default)]
    pub original_commit: Option<CommitDto>,
    #[serde(default)]
    pub diff_hunk: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommitDto {
    pub oid: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadCommentDto {
    pub id: String,
    #[serde(default)]
    pub database_id: Option<u64>,
    #[serde(default)]
    pub author: Option<ActorDto>,
    #[serde(default)]
    pub body: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_edited_at: Option<OffsetDateTime>,
}

/// `addPullRequestReviewThreadReply { comment }`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddReplyData {
    pub add_pull_request_review_thread_reply: Option<ReplyPayload>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReplyPayload {
    pub comment: Option<ThreadCommentDto>,
}
