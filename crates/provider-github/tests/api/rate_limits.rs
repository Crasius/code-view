//! GitHub's primary and secondary rate limits against a mock REST API (GitHub Enterprise layout,
//! `{base}/api/v3`). Behaviour as documented on
//! docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api.

use std::time::{Duration, Instant};

use review_core::{Provider, ProviderError};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{TOKEN, ok_json, provider};

fn user() -> serde_json::Value {
    json!({ "id": 1, "login": "ada", "name": "Ada", "avatar_url": null })
}

async fn mount_user(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v3/user"))
        .respond_with(ok_json(user()))
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
    let user =
        provider(&server, Some(TOKEN)).current_user().await.expect("retried after the pause");
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

    let error = provider(&server, Some(TOKEN)).current_user().await.expect_err("denied");
    assert_eq!(error.status(), Some(403));
    assert!(!matches!(error, ProviderError::RateLimited { .. }));
    assert_eq!(server.received_requests().await.expect("recorded").len(), 1, "no retry");
}
