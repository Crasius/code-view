use serde::Deserialize;
use time::OffsetDateTime;

use super::label::LabelDto;
use super::user::{TeamDto, UserDto};

/// `GET /repos/{o}/{r}/pulls` rows and `GET /repos/{o}/{r}/pulls/{n}`.
/// `mergeable*` and `node_id` usage is detail-only; list rows leave them empty.
#[derive(Debug, Deserialize)]
pub(crate) struct PullRequestDto {
    pub number: u64,
    #[serde(default)]
    pub node_id: String,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    pub user: Option<UserDto>,
    pub html_url: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub closed_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub merged_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub labels: Vec<LabelDto>,
    #[serde(default)]
    pub requested_reviewers: Vec<UserDto>,
    #[serde(default)]
    pub requested_teams: Vec<TeamDto>,
    pub head: BranchDto,
    pub base: BranchDto,
    pub mergeable: Option<bool>,
    pub mergeable_state: Option<String>,
    /// Set while auto-merge is enabled.
    pub auto_merge: Option<AutoMergeDto>,
    /// Line and file counts: in a single pull request's answer, never in list rows.
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub changed_files: Option<u64>,
}

/// `auto_merge`: `merge_method` is `merge`, `squash` or `rebase`; `enabled_by` who turned it on.
#[derive(Debug, Deserialize)]
pub(crate) struct AutoMergeDto {
    pub merge_method: String,
    pub enabled_by: Option<UserDto>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BranchDto {
    #[serde(rename = "ref")]
    pub name: String,
    pub sha: String,
    /// `null` when the fork was deleted.
    pub repo: Option<BranchRepositoryDto>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BranchRepositoryDto {
    pub full_name: String,
    pub name: String,
}
