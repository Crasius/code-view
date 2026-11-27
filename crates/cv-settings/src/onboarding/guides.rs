use review_core::{Connection, ProviderKind, TokenGuide};

/// The token recipe for a platform and host, from the provider crate that knows the platform.
pub(crate) fn token_guide(
    kind: ProviderKind,
    base_url: &str,
    organization: Option<&str>,
) -> TokenGuide {
    match kind {
        ProviderKind::GitHub => provider_github::setup::token_guide(base_url, organization),
        ProviderKind::GitLab => provider_gitlab::setup::token_guide(base_url),
        ProviderKind::AzureDevOps => provider_azure::setup::token_guide(base_url, organization),
    }
}

/// The page where a new token for this connection is created, pre-filled where the platform allows.
pub(crate) fn token_page(connection: &Connection) -> String {
    let organization = connection.organizations.first().map(String::as_str);
    token_guide(connection.kind, connection.base_url(), organization).recommended.create_url
}
