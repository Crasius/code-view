//! The mock GitHub Enterprise host every API test talks to (`{base}/api/v3`, `{base}/api/graphql`):
//! the provider on the app's HTTP client, matchers with GitHub's required headers, JSON answers
//! and the fixtures in `tests/fixtures/`.

use provider_github::GitHubProvider;
use review_core::{Connection, ProviderKind, http};
use serde_json::Value;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

pub(crate) const TOKEN: &str = "test-token";
/// The repository [`rest`] addresses.
pub(crate) const REPO: &str = "contoso/app";

/// A mock host and the provider on it, signed in with [`TOKEN`].
pub(crate) async fn start() -> (MockServer, GitHubProvider) {
    let server = MockServer::start().await;
    let provider = provider(&server, Some(TOKEN));
    (server, provider)
}

pub(crate) fn provider(server: &MockServer, token: Option<&str>) -> GitHubProvider {
    provider_for(connection(server, token))
}

pub(crate) fn connection(server: &MockServer, token: Option<&str>) -> Connection {
    Connection {
        id: "mock".to_owned(),
        name: "Mock GitHub Enterprise".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: token.map(str::to_owned),
    }
}

/// The provider on the app's HTTP client. review-core keeps rate-limit pauses per origin for the
/// whole process, and wiremock hands a finished test's port out again: the origin starts unpaused.
pub(crate) fn provider_for(connection: Connection) -> GitHubProvider {
    http::resume(&connection.base_url);
    GitHubProvider::new(connection, http::build_client())
}

/// A REST call below `/api/v3/repos/{REPO}` with GitHub's required headers.
pub(crate) fn rest(verb: &str, repo_path: &str) -> MockBuilder {
    Mock::given(method(verb))
        .and(path(format!("/api/v3/repos/{REPO}{repo_path}")))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(header("accept", "application/vnd.github+json"))
        .and(header("x-github-api-version", "2022-11-28"))
}

pub(crate) fn graphql() -> MockBuilder {
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
}

pub(crate) fn ok_json(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

/// `tests/fixtures/{name}.json`.
pub(crate) fn fixture(name: &str) -> Value {
    let file = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(file).expect("the fixture file reads");
    serde_json::from_str(&text).expect("the fixture is JSON")
}
