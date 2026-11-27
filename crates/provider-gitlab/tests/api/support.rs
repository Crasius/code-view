//! The provider every test module builds: a GitLab connection to the mock host on the app's own
//! HTTP client, so the tests send the user agent, compression and timeouts the app sends.

use provider_gitlab::GitLabProvider;
use review_core::{Connection, ProviderKind, http};
use wiremock::MockServer;

pub const TOKEN: &str = "test-token";

/// The connection `mock` to `server`, without groups.
pub fn connection(server: &MockServer, token: Option<&str>) -> Connection {
    Connection {
        id: "mock".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::GitLab,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: token.map(str::to_owned),
    }
}

/// The provider on `connection`. The rate-limit pauses of `review_core::http` last for the whole
/// test process and wiremock hands a finished test's port out again, so the host's pauses end
/// first.
pub fn provider_on(connection: Connection) -> GitLabProvider {
    http::resume(connection.base_url());
    GitLabProvider::new(connection, http::build_client())
}

pub fn provider(server: &MockServer, token: Option<&str>) -> GitLabProvider {
    provider_on(connection(server, token))
}
