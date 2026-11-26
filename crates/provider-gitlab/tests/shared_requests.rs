//! Identical GET requests in flight at the same time are sent once and the answer is shared. When
//! the caller that started it goes away before the answer (a screen dropped or restarted its load),
//! the callers waiting for it still get that answer, without a second request.

use std::time::Duration;

use review_core::http;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn a_waiting_caller_gets_the_answer_when_the_starting_caller_goes_away() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/7/pipelines"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{ "id": 1 }]))
                .set_delay(Duration::from_millis(300)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = reqwest::Client::new();
    let url = format!("{}/api/v4/projects/7/pipelines", server.uri());
    let sender = tokio::spawn(http::fetch(client.get(&url)));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let waiter = tokio::spawn(http::fetch(client.get(&url)));
    tokio::time::sleep(Duration::from_millis(50)).await;
    sender.abort();
    let fetched = waiter
        .await
        .expect("the waiting task ran")
        .expect("an answer, not a stopped shared request");
    assert_eq!(fetched.json::<Value>().expect("json"), json!([{ "id": 1 }]));
}
