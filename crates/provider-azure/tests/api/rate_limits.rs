//! Azure DevOps throttling against a mock REST API: blocked requests (429 + `Retry-After`) pause
//! the host and are retried once; delayed ones (200 + `Retry-After`) slow the next request down.
//! Headers as documented on learn.microsoft.com/azure/devops/integrate/concepts/rate-limits.

use std::time::{Duration, Instant};

use provider_azure::AzureDevOpsProvider;
use review_core::{Connection, Provider, ProviderError, ProviderKind, http};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPOSITORIES: &str = "/coll/_apis/git/repositories";

fn provider(server: &MockServer) -> AzureDevOpsProvider {
    let connection = Connection {
        id: "azure-rate".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::AzureDevOps,
        base_url: server.uri(),
        organizations: vec!["coll".to_owned()],
        token: Some("dummy-pat".to_owned()),
    };
    AzureDevOpsProvider::new(connection, http::build_client())
}

fn blocked() -> ResponseTemplate {
    ResponseTemplate::new(429)
        .insert_header("Retry-After", "1")
        .insert_header("X-RateLimit-Resource", "Core")
        .insert_header("X-RateLimit-Remaining", "0")
        .set_body_json(json!({ "message": "TF400733: The request has been canceled: Request was blocked due to exceeding usage of resource 'Core'." }))
}

async fn mount_repositories(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET")).and(path(REPOSITORIES)).respond_with(response).mount(server).await;
}

#[tokio::test]
async fn still_blocked_ends_in_rate_limited_error() {
    let server = MockServer::start().await;
    mount_repositories(&server, blocked()).await;

    let error = provider(&server).list_repositories(None).await.expect_err("still blocked");
    assert!(matches!(error, ProviderError::RateLimited { .. }), "{error:?}");
    assert_eq!(server.received_requests().await.expect("recorded").len(), 2);
}

#[tokio::test]
async fn delayed_answer_pauses_the_next_request() {
    let server = MockServer::start().await;
    mount_repositories(
        &server,
        ResponseTemplate::new(200)
            .insert_header("Retry-After", "1")
            .insert_header("X-RateLimit-Delay", "0.8")
            .set_body_json(json!({ "count": 0, "value": [] })),
    )
    .await;
    let provider = provider(&server);

    provider.list_repositories(None).await.expect("delayed but answered");
    assert!(
        http::rate_limits().iter().any(|notice| notice.origin == server.uri()),
        "the host asked to slow down"
    );
    let started = Instant::now();
    provider.list_repositories(None).await.expect("second");
    assert!(started.elapsed() >= Duration::from_millis(800), "the next request waited");
}
