use serde::{Deserialize, Serialize};
/// `key` is the provider's repository key used in every [`crate::Provider`] call:
/// GitHub `owner/repo`, GitLab `group/sub/project`, Azure DevOps `organization/project/repository`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryInfo {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub web_url: String,
    pub default_branch: Option<String>,
}

/// HTTPS clone URL plus the full `Authorization` header value (`Basic ...`), or `None` for anonymous access.
#[derive(Clone, PartialEq, Eq)]
pub struct GitRemote {
    pub clone_url: String,
    pub authorization_header: Option<String>,
}

impl std::fmt::Debug for GitRemote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitRemote")
            .field("clone_url", &self.clone_url)
            .field(
                "authorization_header",
                &self.authorization_header.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}
