use serde::Deserialize;
use time::OffsetDateTime;

use super::user::UserDto;

/// `GET /pulls/{n}/comments` row: an inline review comment. Replies point at the thread's root via `in_reply_to_id`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReviewCommentDto {
    pub id: u64,
    #[serde(default)]
    pub in_reply_to_id: Option<u64>,
    pub path: String,
    /// `null` when the comment is outdated or file-level.
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub original_line: Option<u32>,
    /// `LEFT` (base) or `RIGHT` (head).
    #[serde(default)]
    pub side: Option<String>,
    /// First line of a multi-line comment; `null` for a single line.
    #[serde(default)]
    pub start_line: Option<u32>,
    #[serde(default)]
    pub original_start_line: Option<u32>,
    #[serde(default)]
    pub start_side: Option<String>,
    /// `line` or `file`.
    #[serde(default)]
    pub subject_type: Option<String>,
    #[serde(default)]
    pub user: Option<UserDto>,
    #[serde(default)]
    pub body: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
    /// The head commit the comment was written on.
    #[serde(default)]
    pub original_commit_id: Option<String>,
    /// The diff lines up to the comment's line, as the comment was written.
    #[serde(default)]
    pub diff_hunk: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
}

/// `GET /issues/{n}/comments` row: a general pull request comment.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct IssueCommentDto {
    pub id: u64,
    #[serde(default)]
    pub user: Option<UserDto>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
}
