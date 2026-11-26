use review_core::Timestamp;
use serde::Deserialize;

/// Pipeline as listed by `/merge_requests/:iid/pipelines` (no start/finish times there)
/// or embedded as `head_pipeline` in a merge request (with times).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PipelineDto {
    pub id: u64,
    pub project_id: u64,
    pub sha: String,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    pub status: String,
    #[serde(default)]
    pub source: Option<String>,
    /// `workflow:name`; REST leaves it out of merge request pipelines, GraphQL fills it in
    /// (`crate::pipeline_names`).
    #[serde(default)]
    pub name: Option<String>,
    /// `PipelineMergeRequestEventType` (`DETACHED`, `MERGED_RESULT`, `MERGE_TRAIN`), from GraphQL
    /// only.
    #[serde(skip)]
    pub merge_request_event_type: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub created_at: Option<Timestamp>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<Timestamp>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub started_at: Option<Timestamp>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub finished_at: Option<Timestamp>,
    #[serde(default)]
    pub web_url: Option<String>,
}

/// `/pipelines/:id/jobs` and `/pipelines/:id/bridges` item.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct JobDto {
    pub id: u64,
    pub name: String,
    pub stage: String,
    pub status: String,
    #[serde(default)]
    pub allow_failure: bool,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub started_at: Option<Timestamp>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub finished_at: Option<Timestamp>,
    #[serde(default)]
    pub web_url: Option<String>,
    /// Set on bridges (trigger jobs) only.
    #[serde(default)]
    pub downstream_pipeline: Option<DownstreamPipelineDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DownstreamPipelineDto {
    #[serde(default)]
    pub web_url: Option<String>,
}

/// `/repository/commits/:sha/statuses` item: CI jobs and external statuses alike.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CommitStatusDto {
    pub id: u64,
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_url: Option<String>,
    #[serde(default)]
    pub allow_failure: bool,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub started_at: Option<Timestamp>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub finished_at: Option<Timestamp>,
}
