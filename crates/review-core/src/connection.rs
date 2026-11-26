use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ProviderKind {
    GitHub,
    GitLab,
    AzureDevOps,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 3] =
        [ProviderKind::AzureDevOps, ProviderKind::GitHub, ProviderKind::GitLab];

    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::GitHub => "GitHub",
            ProviderKind::GitLab => "GitLab",
            ProviderKind::AzureDevOps => "Azure DevOps",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::GitHub => "https://github.com",
            ProviderKind::GitLab => "https://gitlab.com",
            ProviderKind::AzureDevOps => "https://dev.azure.com",
        }
    }
}

/// One account on one host.
///
/// `base_url` is the web root the user sees, without trailing slash
/// (`https://github.com`, `https://gitlab.example.com`, `https://dev.azure.com`,
/// or an Azure DevOps Server collection URL). Providers derive their API roots from it
/// (GitHub Enterprise: `{base}/api/v3`, GitLab: `{base}/api/v4`).
///
/// `organizations` scope listings: GitHub owners (orgs or users), GitLab group paths,
/// Azure DevOps organizations. Azure DevOps Services needs at least one; for GitHub and
/// GitLab an empty list means "everything the token sees".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub organizations: Vec<String>,
    pub token: Option<String>,
}

impl Connection {
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref().filter(|token| !token.is_empty())
    }

    pub fn base_url(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }
}
