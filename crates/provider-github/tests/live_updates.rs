//! Change detection for an open pull request against a mock GitHub Enterprise host: the probe is a
//! conditional `GET` of the pull request (and its reviews with a token), so an unchanged pull
//! request answers `304 Not Modified`, which costs no rate limit. `.expect(n)` is verified when
//! the server drops.

use provider_github::GitHubProvider;
use review_core::{Connection, Provider, ProviderKind, http};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PULL: &str = "/api/v3/repos/contoso/app/pulls/1234";
const REVIEWS: &str = "/api/v3/repos/contoso/app/pulls/1234/reviews";

fn provider(server: &MockServer, token: Option<&str>) -> GitHubProvider {
    let connection = Connection {
        id: "live".to_owned(),
        name: "Mock GitHub Enterprise".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: token.map(str::to_owned),
    };
    GitHubProvider::new(connection, reqwest::Client::new())
}

fn pull(head: &str, updated_at: &str) -> Value {
    let mut pull: Value = serde_json::from_str(include_str!("fixtures/pull_request.json"))
        .expect("pull request fixture");
    pull["head"]["sha"] = json!(head);
    pull["updated_at"] = json!(updated_at);
    pull
}

fn tagged(body: Value, etag: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).insert_header("ETag", etag).set_body_json(body)
}

#[tokio::test]
async fn an_unchanged_pull_request_is_a_free_304() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PULL))
        .and(header("if-none-match", "\"pull-1\""))
        .respond_with(ResponseTemplate::new(304))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(PULL))
        .respond_with(tagged(pull("head111", "2024-05-02T10:00:00Z"), "\"pull-1\""))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(REVIEWS))
        .and(header("if-none-match", "\"reviews-1\""))
        .respond_with(ResponseTemplate::new(304))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(REVIEWS))
        .respond_with(tagged(
            json!([{ "id": 9, "state": "APPROVED", "user": { "id": 3, "login": "cy" } }]),
            "\"reviews-1\"",
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let provider = provider(&server, Some("test-token"));

    let first = http::for_connection("github-live", provider.revision("contoso/app", 1234))
        .await
        .expect("first probe");
    assert_eq!(first.head_sha, "head111");
    let second = http::for_connection("github-live", provider.revision("contoso/app", 1234))
        .await
        .expect("second probe");
    assert_eq!(second, first, "a 304 answers with the remembered pull request");
    assert_eq!(
        http::traffic::of("github-live").expect("counted").not_modified,
        2,
        "pull request and reviews"
    );
}

#[tokio::test]
async fn a_push_or_a_comment_changes_the_revision() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PULL))
        .respond_with(tagged(pull("head111", "2024-05-02T10:00:00Z"), "\"pull-1\""))
        .up_to_n_times(1)
        .with_priority(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(PULL))
        .respond_with(tagged(pull("head111", "2024-05-02T11:00:00Z"), "\"pull-2\""))
        .up_to_n_times(1)
        .with_priority(3)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(PULL))
        .respond_with(tagged(pull("head222", "2024-05-02T12:00:00Z"), "\"pull-3\""))
        .with_priority(4)
        .mount(&server)
        .await;
    // Anonymous: the reviews list is not asked for, its 304s would count against 60 requests an hour.
    Mock::given(method("GET"))
        .and(path(REVIEWS))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let provider = provider(&server, None);

    let opened = provider.revision("contoso/app", 1234).await.expect("probe");
    let commented = provider.revision("contoso/app", 1234).await.expect("probe after a comment");
    assert_eq!(commented.head_sha, opened.head_sha);
    assert_ne!(commented.marker, opened.marker, "updated_at moved with the comment");
    let pushed = provider.revision("contoso/app", 1234).await.expect("probe after a push");
    assert_eq!(pushed.head_sha, "head222");
}
