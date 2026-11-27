use serde::Deserialize;
use time::OffsetDateTime;

use super::user::UserDto;

/// `GET /pulls/{n}/reviews` row. `state`: `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`,
/// `DISMISSED`, `PENDING`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReviewDto {
    pub id: u64,
    pub user: Option<UserDto>,
    pub body: Option<String>,
    pub state: String,
    /// `null` while the review is pending.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub submitted_at: Option<OffsetDateTime>,
}
