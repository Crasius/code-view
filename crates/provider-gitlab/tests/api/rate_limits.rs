//! GitLab's rate-limit answers against a mock REST API: a 429 with `Retry-After` pauses the host,
//! the request goes out once more after the pause, and a second 429 ends in a calm error.
//! Headers as documented on
//! <https://docs.gitlab.com/administration/settings/user_and_ip_rate_limits/>.

use std::time::{Duration, Instant};

use review_core::{Provider, ProviderError, http};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{self, TOKEN};

fn too_many_requests() -> ResponseTemplate {
    ResponseTemplate::new(429)
        .insert_header("Retry-After", "1")
        .insert_header("RateLimit-Name", "throttle_authenticated_api")
        .insert_header("RateLimit-Remaining", "0")
        .set_body_string("Retry later")
}

/// Waits for the 429 to arrive rather than a fixed time, so a busy test machine cannot outrun it.
async fn paused(server: &MockServer) {
    let is_paused = || http::rate_limits().iter().any(|notice| notice.origin == server.uri());
    tokio::time::timeout(Duration::from_secs(5), async {
        while !is_paused() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the host pauses after the 429");
}

#[tokio::test]
async fn waits_for_retry_after_then_retries_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(too_many_requests())
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "id": 1, "username": "ada", "name": "Ada" })),
        )
        .mount(&server)
        .await;
    let provider = support::provider(&server, Some(TOKEN));

    let started = Instant::now();
    let call = tokio::spawn(http::for_connection("gitlab-rate-retry", async move {
        provider.current_user().await
    }));
    paused(&server).await;
    let notices = http::rate_limits();
    let notice =
        notices.iter().find(|notice| notice.origin == server.uri()).expect("the host is paused");
    assert_eq!(notice.connection_ids, ["gitlab-rate-retry"]);
    assert!(notice.resumes_in <= Duration::from_secs(1));

    let user = call.await.expect("task").expect("retried after the pause");
    assert_eq!(user.display_name, "Ada");
    assert!(started.elapsed() >= Duration::from_millis(900), "waited for Retry-After");
    assert_eq!(server.received_requests().await.expect("recorded").len(), 2);
    assert!(http::rate_limits().iter().all(|notice| notice.origin != server.uri()), "pause ended");
    let traffic = http::traffic::of("gitlab-rate-retry").expect("counted");
    assert_eq!((traffic.sent, traffic.rate_limited), (2, 1));
}

#[tokio::test]
async fn second_limit_ends_in_rate_limited_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(too_many_requests())
        .mount(&server)
        .await;

    let error =
        support::provider(&server, Some(TOKEN)).current_user().await.expect_err("still limited");
    assert!(matches!(error, ProviderError::RateLimited { retry_after_secs: 1, .. }), "{error:?}");
    assert_eq!(server.received_requests().await.expect("recorded").len(), 2, "one retry, no loop");
}

#[tokio::test]
async fn retry_now_ends_the_pause_early() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "600"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "id": 1, "username": "ada", "name": "Ada" })),
        )
        .mount(&server)
        .await;
    let provider = support::provider(&server, Some(TOKEN));
    let call = tokio::spawn(async move { provider.current_user().await });
    paused(&server).await;

    http::resume(&server.uri());
    let user = tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .expect("woken by resume")
        .expect("task");
    user.expect("retried");
}
