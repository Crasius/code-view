//! Editing and deleting merge request notes against a mock GitLab REST API v4, matched on the
//! documented discussion note endpoints (docs.gitlab.com/api/discussions).

use provider_gitlab::GitLabProvider;
use review_core::provider::CommentActions;
use review_core::{Connection, Provider, ProviderKind, ReviewThread, ThreadStatus};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "group/sub/project";
const NOTE: &str =
    "/api/v4/projects/group%2Fsub%2Fproject/merge_requests/7/discussions/6a9c1750b37d/notes/302";
const TOKEN: &str = "test-token";

fn provider(server: &MockServer) -> GitLabProvider {
    let connection = Connection {
        id: "mock".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::GitLab,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: Some(TOKEN.to_owned()),
    };
    GitLabProvider::new(connection, reqwest::Client::new())
}

#[tokio::test]
async fn edits_a_note_with_its_body_only() {
    let server = MockServer::start().await;
    let edited: Value =
        serde_json::from_str(include_str!("fixtures/note_edited.json")).expect("fixture");
    Mock::given(method("PUT"))
        .and(path(NOTE))
        .and(header("PRIVATE-TOKEN", TOKEN))
        // `body` and `resolved` exclude each other: only the body goes.
        .and(body_json(json!({ "body": "Use a slice here." })))
        .respond_with(ResponseTemplate::new(200).set_body_json(edited))
        .expect(1)
        .mount(&server)
        .await;
    let comment = provider(&server)
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
    provider(&server).delete_comment(REPO, 7, "6a9c1750b37d", "302").await.expect("deleted");
}

#[tokio::test]
async fn system_notes_offer_nothing() {
    let server = MockServer::start().await;
    let mut thread = ReviewThread {
        id: "6a9c1750b37d".to_owned(),
        status: ThreadStatus::Active,
        anchor: None,
        is_outdated: false,
        origin: None,
        is_system: false,
        can_reply: true,
        can_resolve: true,
        comments: Vec::new(),
    };
    let provider = provider(&server);
    assert_eq!(provider.comment_actions(&thread), CommentActions::EDIT_AND_DELETE);
    thread.is_system = true;
    assert_eq!(provider.comment_actions(&thread), CommentActions::NONE);
}
