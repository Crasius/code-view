//! GitHub's primary and secondary rate limits and conditional requests against a mock REST API
//! (GitHub Enterprise layout, `{base}/api/v3`). Behaviour as documented on
//! docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api and
//! docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api.

use std::time::{Duration, Instant};

use provider_github::GitHubProvider;
use review_core::{Connection, Provider, ProviderError, ProviderKind, http};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn provider(server: &MockServer) -> GitHubProvider {
    let connection = Connection {
        id: "github-rate".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: Some("test-token".to_owned()),
    };
    GitHubProvider::new(connection, reqwest::Client::new())
}

fn user() -> serde_json::Value {
    json!({ "id": 1, "login": "ada", "name": "Ada", "avatar_url": null })
}

async fn mount_user(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v3/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(user()))
        .mount(server)
        .await;
}

#[tokio::test]
async fn secondary_limit_403_waits_for_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/user"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("retry-after", "1")
                .set_body_json(json!({ "message": "You have exceeded a secondary rate limit." })),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    mount_user(&server).await;

    let started = Instant::now();
    let user = provider(&server).current_user().await.expect("retried after the pause");
    assert_eq!(user.login.as_deref(), Some("ada"));
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(server.received_requests().await.expect("recorded").len(), 2);
}

#[tokio::test]
async fn plain_permission_403_is_not_a_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/user"))
        .respond_with(ResponseTemplate::new(403).set_body_json(
            json!({ "message": "Resource not accessible by personal access token" }),
        ))
        .mount(&server)
        .await;

    let error = provider(&server).current_user().await.expect_err("denied");
    assert_eq!(error.status(), Some(403));
    assert!(!matches!(error, ProviderError::RateLimited { .. }));
    assert_eq!(server.received_requests().await.expect("recorded").len(), 1, "no retry");
}

#[tokio::test]
async fn unchanged_resource_revalidates_with_if_none_match() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/user"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304).insert_header("etag", "\"v1\""))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/user"))
        .respond_with(
            ResponseTemplate::new(200).insert_header("etag", "\"v1\"").set_body_json(user()),
        )
        .mount(&server)
        .await;
    let provider = provider(&server);

    let first = http::for_connection("github-etag", provider.current_user()).await.expect("first");
    let second =
        http::for_connection("github-etag", provider.current_user()).await.expect("revalidated");
    assert_eq!(first, second);
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 2);
    assert!(requests[1].headers.get("if-none-match").is_some());
    let traffic = http::traffic::of("github-etag").expect("counted");
    assert_eq!(traffic.not_modified, 1);
}
