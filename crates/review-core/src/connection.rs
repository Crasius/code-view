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
#[derive(Clone, PartialEq, Eq)]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub organizations: Vec<String>,
    pub token: Option<String>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("organizations", &self.organizations)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Connection {
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref().filter(|token| !token.is_empty())
    }

    pub fn base_url(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_debug_never_prints_the_token() {
        let connection = Connection {
            id: "c1".to_owned(),
            name: "Work".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: "https://github.example.test".to_owned(),
            organizations: vec!["acme".to_owned()],
            token: Some("secret-token".to_owned()),
        };

        let printed = format!("{connection:?}");

        assert!(!printed.contains("secret-token"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
    }
}
