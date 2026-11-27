//! Editing and deleting pull request thread comments against a mocked Azure DevOps REST API (7.1),
//! as documented under learn.microsoft.com/rest/api/azure/devops/git/pull-request-thread-comments.

use review_core::Provider;
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{AUTHORIZATION, COLLECTION, REPO, TOKEN, provider};

const COMMENT: &str =
    "/coll/My%20Project/_apis/git/repositories/web/pullrequests/7/threads/41/comments/2";

#[tokio::test]
async fn edits_a_comment_by_its_content() {
    let server = MockServer::start().await;
    let edited: Value =
        serde_json::from_str(include_str!("../fixtures/comment_edited.json")).expect("fixture");
    Mock::given(method("PATCH"))
        .and(path(COMMENT))
        .and(query_param("api-version", "7.1"))
        .and(header("authorization", AUTHORIZATION))
        .and(body_json(json!({ "content": "Use a slice here." })))
        .respond_with(ResponseTemplate::new(200).set_body_json(edited))
        .expect(1)
        .mount(&server)
        .await;
    let comment = provider(&server, &[COLLECTION], Some(TOKEN))
        .edit_comment(REPO, 7, "41", "2", "Use a slice here.")
        .await
        .expect("edited");
    assert_eq!((comment.id.as_str(), comment.body.as_str()), ("2", "Use a slice here."));
    assert_eq!(comment.author.id, "me-id");
    assert!(comment.updated_at.is_some(), "the edit shows as edited");
}

#[tokio::test]
async fn deletes_a_comment() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(COMMENT))
        .and(query_param("api-version", "7.1"))
        .and(header("authorization", AUTHORIZATION))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    provider(&server, &[COLLECTION], Some(TOKEN))
        .delete_comment(REPO, 7, "41", "2")
        .await
        .expect("deleted");
}

/// Azure DevOps keeps a deleted comment with `isDeleted: true`: the listing hides it, and the
/// thread with it once every comment is deleted.
#[tokio::test]
async fn deleted_comments_stay_hidden() {
    let server = MockServer::start().await;
    let comment = |id: u32, is_deleted: bool| {
        json!({ "id": id, "author": { "id": "me-id", "displayName": "Me" }, "content": format!("c{id}"),
                "publishedDate": "2026-01-01T10:00:00Z", "commentType": "text", "isDeleted": is_deleted })
    };
    let threads = json!({ "count": 2, "value": [
        { "id": 41, "status": "active", "publishedDate": "2026-01-01T10:00:00Z",
          "comments": [comment(1, false), comment(2, true)] },
        { "id": 42, "status": "active", "publishedDate": "2026-01-01T10:00:00Z",
          "comments": [comment(3, true)] }
    ] });
    Mock::given(method("GET"))
        .and(path("/coll/My%20Project/_apis/git/repositories/web/pullrequests/7/threads"))
        .respond_with(ResponseTemplate::new(200).set_body_json(threads))
        .mount(&server)
        .await;
    let listed =
        provider(&server, &[COLLECTION], Some(TOKEN)).threads(REPO, 7).await.expect("threads");
    assert_eq!(listed.len(), 1, "the thread without comments left is gone");
    let ids: Vec<&str> = listed[0].comments.iter().map(|comment| comment.id.as_str()).collect();
    assert_eq!(ids, ["1"]);
}
