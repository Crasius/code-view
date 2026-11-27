//! Editing and deleting comments against a mock GitHub Enterprise host, matched on the documented
//! REST v3 (2022-11-28) contracts: review comments, issue comments and review bodies.

use review_core::{Provider, ProviderError};
use serde_json::json;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_json;

use crate::support::{self, REPO, fixture, ok_json, rest};

const NUMBER: u64 = 7;

#[tokio::test]
async fn edits_a_review_thread_comment_by_its_database_id() {
    let (server, provider) = support::start().await;
    rest("PATCH", "/pulls/comments/1001")
        .and(body_json(json!({ "body": "Use a slice here." })))
        .respond_with(ok_json(fixture("review_comment_edited")))
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
    assert_eq!(
        comment.updated_at,
        Some(time::macros::datetime!(2024-06-01 11:30 UTC)),
        "the edit shows as edited"
    );
}

#[tokio::test]
async fn edits_a_general_comment_as_an_issue_comment() {
    let (server, provider) = support::start().await;
    rest("PATCH", "/issues/comments/5")
        .and(body_json(json!({ "body": "Ready for another look." })))
        .respond_with(ok_json(fixture("issue_comment_edited")))
        .expect(1)
        .mount(&server)
        .await;
    let comment = provider
        .edit_comment(REPO, NUMBER, "issue-comment:5", "5", "Ready for another look.")
        .await
        .expect("edited");
    assert_eq!((comment.id.as_str(), comment.body.as_str()), ("5", "Ready for another look."));
    assert_eq!(comment.updated_at, Some(time::macros::datetime!(2024-06-01 11:30 UTC)));
}

#[tokio::test]
async fn edits_a_review_body_through_the_review() {
    let (server, provider) = support::start().await;
    rest("PUT", &format!("/pulls/{NUMBER}/reviews/80"))
        .and(body_json(json!({ "body": "Approved after the rename." })))
        .respond_with(ok_json(fixture("review_edited")))
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
    let (server, provider) = support::start().await;
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
    let (_server, provider) = support::start().await;
    let error = provider.delete_comment(REPO, NUMBER, "review:80", "80").await.expect_err("kept");
    assert!(matches!(error, ProviderError::Unsupported(_)), "{error:?}");
}

#[tokio::test]
async fn a_refused_edit_reports_the_status() {
    let (server, provider) = support::start().await;
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
