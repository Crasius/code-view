//! The provider factory: which client talks to a connection's platform. It is its own crate so
//! that only the bin (and the tests that run against the mock hosts) link the three provider
//! crates; the libraries take the factory as a `ProviderFactory` value instead of naming them.

#![forbid(unsafe_code)]

use std::sync::Arc;

use review_core::{Connection, Provider, ProviderKind};

/// The client for `connection`'s platform, sending through `http`.
pub fn create_provider(connection: Connection, http: reqwest::Client) -> Arc<dyn Provider> {
    match connection.kind {
        ProviderKind::GitHub => Arc::new(provider_github::GitHubProvider::new(connection, http)),
        ProviderKind::GitLab => Arc::new(provider_gitlab::GitLabProvider::new(connection, http)),
        ProviderKind::AzureDevOps => {
            Arc::new(provider_azure::AzureDevOpsProvider::new(connection, http))
        }
    }
}
