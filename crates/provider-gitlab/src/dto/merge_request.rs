use review_core::Timestamp;
use serde::Deserialize;

use super::{PipelineDto, UserDto};

#[derive(Debug, Clone, Deserialize)]
#[expect(clippy::struct_excessive_bools, reason = "mirrors the boolean flags of the GitLab JSON")]
pub(crate) struct MergeRequestDto {
    pub iid: u64,
    pub project_id: u64,
    /// Differs from `project_id` for merge requests from forks.
    pub source_project_id: Option<u64>,
    pub title: String,
    pub description: Option<String>,
    /// `opened`, `closed`, `locked` or `merged`.
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub work_in_progress: bool,
    pub author: UserDto,
    pub source_branch: String,
    pub target_branch: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<Timestamp>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub closed_at: Option<Timestamp>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub merged_at: Option<Timestamp>,
    pub web_url: String,
    /// Names, or objects when the request passes `with_labels_details=true`.
    #[serde(default)]
    pub labels: Vec<LabelRef>,
    #[serde(default)]
    pub reviewers: Vec<UserDto>,
    pub references: Option<ReferencesDto>,
    pub sha: Option<String>,
    pub diff_refs: Option<DiffRefsDto>,
    pub detailed_merge_status: Option<String>,
    pub merge_status: Option<String>,
    #[serde(default)]
    pub has_conflicts: bool,
    /// Comments by people (system notes left out); the live-update probe watches it.
    #[serde(default)]
    pub user_notes_count: u64,
    #[serde(default)]
    pub blocking_discussions_resolved: bool,
    pub head_pipeline: Option<PipelineDto>,
    /// True while the merge request is set to auto-merge.
    #[serde(default)]
    pub merge_when_pipeline_succeeds: bool,
    /// Who merged it or set it to auto-merge.
    pub merge_user: Option<UserDto>,
    /// `merge_user` before GitLab 14.7.
    pub merged_by: Option<UserDto>,
    #[serde(default)]
    pub squash: bool,
    /// Whether the merge squashes, the project's squash setting taken into account.
    pub squash_on_merge: Option<bool>,
}

impl MergeRequestDto {
    /// The commit every part of a merge request compares against: outdated threads, the detail,
    /// the revision probe and the head pipelines. `diff_refs` lags behind `sha` right after a push.
    pub(crate) fn head_sha(&self) -> Option<&str> {
        self.diff_refs.as_ref().and_then(|refs| refs.head_sha.as_deref()).or(self.sha.as_deref())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReferencesDto {
    /// `group/sub/project!42`.
    pub full: String,
}

/// Empty right after creation; GitLab fills it asynchronously.
#[derive(Debug, Clone, Deserialize)]
#[expect(clippy::struct_field_names, reason = "fields mirror the `diff_refs` JSON keys")]
pub(crate) struct DiffRefsDto {
    pub base_sha: Option<String>,
    pub start_sha: Option<String>,
    pub head_sha: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum LabelRef {
    Detail(LabelDto),
    Name(String),
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LabelDto {
    pub name: String,
    /// `#RRGGBB`.
    pub color: Option<String>,
    pub description: Option<String>,
}

/// `/merge_requests/:iid/reviewers` item.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReviewerDto {
    pub user: UserDto,
    /// `unreviewed`, `review_started`, `reviewed`, `requested_changes`, `approved`, `unapproved`.
    pub state: Option<String>,
}

/// `/merge_requests/:iid/approvals`; Community Edition returns only a few fields, `approved_by` among them.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ApprovalsDto {
    #[serde(default)]
    pub approved_by: Vec<ApprovedByDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ApprovedByDto {
    pub user: UserDto,
}
