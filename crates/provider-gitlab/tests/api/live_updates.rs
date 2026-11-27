//! Change detection for an open merge request against a mock GitLab REST API v4: the probe reads
//! the merge request, its reviewers and approvals, never the discussions. `.expect(n)` is
//! verified when the server drops.

use provider_gitlab::GitLabProvider;
use review_core::{Connection, Provider, ProviderKind};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "group/sub/project";
const MR: &str = "/api/v4/projects/group%2Fsub%2Fproject/merge_requests/7";

fn provider(server: &MockServer) -> GitLabProvider {
    let connection = Connection {
        id: "live".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::GitLab,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: Some("test-token".to_owned()),
    };
    GitLabProvider::new(connection, reqwest::Client::new())
}

fn merge_request(head: &str, notes: u64) -> Value {
    json!({
        "id": 1, "iid": 7, "project_id": 42, "title": "Fix", "description": "", "state": "opened",
        "author": { "id": 1, "username": "ann" }, "source_branch": "feature", "target_branch": "main",
        "created_at": "2026-01-01T10:00:00Z", "updated_at": "2026-01-02T10:00:00Z",
        "web_url": "https://gitlab.example.com/group/sub/project/-/merge_requests/7",
        "sha": head, "user_notes_count": notes,
        "diff_refs": { "base_sha": "base1", "start_sha": "base1", "head_sha": head }
    })
}

async fn mount_answers(server: &MockServer, answers: &[Value]) {
    for (index, answer) in answers.iter().enumerate() {
        Mock::given(method("GET"))
            .and(path(MR))
            .and(query_param("with_labels_details", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .up_to_n_times(1)
            .with_priority(u8::try_from(index + 1).expect("fewer than 255 answers"))
            .expect(1)
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(format!("{MR}/reviewers")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(answers.len() as u64)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{MR}/approvals")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "approved_by": [] })))
        .expect(answers.len() as u64)
        .mount(server)
        .await;
    Mock::given(path_regex("discussions|notes|graphql"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_new_comment_changes_the_marker_and_a_push_the_head() {
    let server = MockServer::start().await;
    mount_answers(
        &server,
        &[
            merge_request("head1", 0),
            merge_request("head1", 0),
            merge_request("head1", 1),
            merge_request("head2", 1),
        ],
    )
    .await;
    let provider = provider(&server);

    let opened = provider.revision(REPO, 7).await.expect("probe");
    assert_eq!(opened.head_sha, "head1");
    assert_eq!(provider.revision(REPO, 7).await.expect("probe"), opened, "nothing changed");
    let commented = provider.revision(REPO, 7).await.expect("probe after a comment");
    assert_eq!(commented.head_sha, opened.head_sha);
    assert_ne!(commented.marker, opened.marker, "user_notes_count moved");
    let pushed = provider.revision(REPO, 7).await.expect("probe after a push");
    assert_eq!(pushed.head_sha, "head2");
}
