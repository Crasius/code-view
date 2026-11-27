use serde::Deserialize;

/// REST user object (`user`, `requested_reviewers[]`, ...).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UserDto {
    pub id: u64,
    pub login: String,
    pub avatar_url: Option<String>,
    /// Only present on `/user` and `/users/{login}`.
    pub name: Option<String>,
}

/// REST team object (`requested_teams[]`).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TeamDto {
    pub id: u64,
    pub name: String,
    pub slug: String,
}

/// `GET /pulls/{n}/requested_reviewers`
#[derive(Debug, Deserialize)]
pub(crate) struct RequestedReviewersDto {
    #[serde(default)]
    pub users: Vec<UserDto>,
    #[serde(default)]
    pub teams: Vec<TeamDto>,
}
