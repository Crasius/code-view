//! Editing and deleting merge request notes against a mock GitLab REST API v4, matched on the
//! documented discussion note endpoints (docs.gitlab.com/api/discussions).

use review_core::Provider;
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{self, TOKEN};

const REPO: &str = "group/sub/project";
const NOTE: &str =
    "/api/v4/projects/group%2Fsub%2Fproject/merge_requests/7/discussions/6a9c1750b37d/notes/302";

#[tokio::test]
async fn edits_a_note_with_its_body_only() {
    let server = MockServer::start().await;
    let edited: Value =
        serde_json::from_str(include_str!("../fixtures/note_edited.json")).expect("fixture");
    Mock::given(method("PUT"))
        .and(path(NOTE))
        .and(header("PRIVATE-TOKEN", TOKEN))
        // `body` and `resolved` exclude each other: only the body goes.
        .and(body_json(json!({ "body": "Use a slice here." })))
        .respond_with(ResponseTemplate::new(200).set_body_json(edited))
        .expect(1)
        .mount(&server)
        .await;
    let comment = support::provider(&server, Some(TOKEN))
        .edit_comment(REPO, 7, "6a9c1750b37d", "302", "Use a slice here.")
        .await
        .expect("edited");
    assert_eq!((comment.id.as_str(), comment.body.as_str()), ("302", "Use a slice here."));
    assert_eq!(comment.author.id, "9");
    assert!(comment.updated_at.is_some(), "the edit shows as edited");
}

#[tokio::test]
async fn deletes_a_note() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(NOTE))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    support::provider(&server, Some(TOKEN))
        .delete_comment(REPO, 7, "6a9c1750b37d", "302")
        .await
        .expect("deleted");
}
