use serde::Deserialize;
use time::OffsetDateTime;

/// `GET /repos/{o}/{r}/actions/runs?head_sha=`
#[derive(Debug, Deserialize)]
pub(crate) struct WorkflowRunsPage {
    #[serde(default)]
    pub workflow_runs: Vec<WorkflowRunDto>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkflowRunDto {
    pub id: u64,
    #[serde(default)]
    pub workflow_id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub display_title: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    pub html_url: String,
    /// The workflow file, e.g. `.github/workflows/ci.yml`.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub run_started_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
}

/// `GET /repos/{o}/{r}/commits/{sha}/check-runs`
#[derive(Debug, Deserialize)]
pub(crate) struct CheckRunsPage {
    #[serde(default)]
    pub check_runs: Vec<CheckRunDto>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CheckRunDto {
    pub id: u64,
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub details_url: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub completed_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub output: Option<CheckOutputDto>,
    #[serde(default)]
    pub app: Option<AppDto>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CheckOutputDto {
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AppDto {
    #[serde(default)]
    pub slug: Option<String>,
}

/// `GET /repos/{o}/{r}/commits/{sha}/status`: latest status per context.
#[derive(Debug, Deserialize)]
pub(crate) struct CombinedStatusDto {
    #[serde(default)]
    pub statuses: Vec<CommitStatusDto>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommitStatusDto {
    pub id: u64,
    pub context: String,
    /// error, failure, pending, success
    pub state: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_url: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub created_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
}

/// `GET /repos/{o}/{r}/actions/runs/{id}/jobs`
#[derive(Debug, Deserialize)]
pub(crate) struct JobsPage {
    #[serde(default)]
    pub jobs: Vec<JobDto>,
}

/// `GET /repos/{o}/{r}/contents/{path}?ref=…` for a file: base64 content (files up to 1 MB).
#[derive(Debug, Deserialize)]
pub(crate) struct FileContentDto {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub encoding: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct JobDto {
    pub id: u64,
    pub name: String,
    /// The run attempt this execution of the job belongs to, from 1.
    #[serde(default)]
    pub run_attempt: Option<u32>,
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub completed_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub steps: Vec<StepDto>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StepDto {
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    pub number: u64,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub completed_at: Option<OffsetDateTime>,
}
