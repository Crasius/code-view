//! Editing and deleting comments against a mock GitHub Enterprise host, matched on the documented
//! REST v3 (2022-11-28) contracts: review comments, issue comments and review bodies.

use provider_github::GitHubProvider;
use review_core::{Connection, Provider, ProviderError, ProviderKind};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

const REPO: &str = "cli/cli";
const NUMBER: u64 = 7;

async fn setup() -> (MockServer, GitHubProvider) {
    let server = MockServer::start().await;
    let connection = Connection {
        id: "mock".to_owned(),
        name: "Mock GitHub Enterprise".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: Some("test-token".to_owned()),
    };
    (server, GitHubProvider::new(connection, reqwest::Client::new()))
}

fn rest(verb: &str, repo_path: &str) -> MockBuilder {
    Mock::given(method(verb))
        .and(path(format!("/api/v3/repos/{REPO}{repo_path}")))
        .and(header("authorization", "Bearer test-token"))
        .and(header("accept", "application/vnd.github+json"))
        .and(header("x-github-api-version", "2022-11-28"))
}

fn fixture(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture")
}

#[tokio::test]
async fn edits_a_review_thread_comment_by_its_database_id() {
    let (server, provider) = setup().await;
    rest("PATCH", "/pulls/comments/1001")
        .and(body_json(json!({ "body": "Use a slice here." })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(fixture(include_str!("fixtures/review_comment_edited.json"))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let comment = provider
        .edit_comment(REPO, NUMBER, "PRRT_kwDOA", "1001", "Use a slice here.")
        .await
        .expect("edited");
    assert_eq!(comment.id, "1001");
    assert_eq!(comment.body, "Use a slice here.");
    assert_eq!(comment.author.id, "9");
    assert!(comment.updated_at.is_some(), "the edit shows as edited");
}

#[tokio::test]
async fn edits_a_general_comment_as_an_issue_comment() {
    let (server, provider) = setup().await;
    rest("PATCH", "/issues/comments/5")
        .and(body_json(json!({ "body": "Ready for another look." })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(fixture(include_str!("fixtures/issue_comment_edited.json"))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let comment = provider
        .edit_comment(REPO, NUMBER, "issue-comment:5", "5", "Ready for another look.")
        .await
        .expect("edited");
    assert_eq!((comment.id.as_str(), comment.body.as_str()), ("5", "Ready for another look."));
    assert!(comment.updated_at.is_some());
}

#[tokio::test]
async fn edits_a_review_body_through_the_review() {
    let (server, provider) = setup().await;
    rest("PUT", &format!("/pulls/{NUMBER}/reviews/80"))
        .and(body_json(json!({ "body": "Approved after the rename." })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(fixture(include_str!("fixtures/review_edited.json"))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let comment = provider
        .edit_comment(REPO, NUMBER, "review:80", "80", "Approved after the rename.")
        .await
        .expect("edited");
    assert_eq!((comment.id.as_str(), comment.body.as_str()), ("80", "Approved after the rename."));
}

#[tokio::test]
async fn deletes_review_and_issue_comments() {
    let (server, provider) = setup().await;
    for comment_path in ["/pulls/comments/1001", "/issues/comments/5"] {
        rest("DELETE", comment_path)
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
    }
    provider.delete_comment(REPO, NUMBER, "PRRT_kwDOA", "1001").await.expect("review comment");
    provider.delete_comment(REPO, NUMBER, "issue-comment:5", "5").await.expect("issue comment");
}

#[tokio::test]
async fn refuses_to_delete_a_submitted_review() {
    let (_server, provider) = setup().await;
    let error = provider.delete_comment(REPO, NUMBER, "review:80", "80").await.expect_err("kept");
    assert!(matches!(error, ProviderError::Unsupported(_)), "{error:?}");
}

#[tokio::test]
async fn a_refused_edit_reports_the_status() {
    let (server, provider) = setup().await;
    rest("PATCH", "/pulls/comments/1001")
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({ "message": "Must have admin rights" })),
        )
        .mount(&server)
        .await;
    let error =
        provider.edit_comment(REPO, NUMBER, "PRRT_kwDOA", "1001", "x").await.expect_err("refused");
    assert_eq!(error.status(), Some(403), "{error:?}");
}
