use serde::{Deserialize, Serialize};
/// A user or group as the platform reports it. `id` is the platform's stable id
/// (GitHub numeric id, GitLab numeric id, Azure DevOps identity GUID).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UserIdentity {
    pub id: String,
    pub display_name: String,
    /// Login or unique name (GitHub login, GitLab username, Azure DevOps uniqueName/e-mail).
    pub login: Option<String>,
    pub avatar_url: Option<String>,
}
