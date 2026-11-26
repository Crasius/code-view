//! "Set to auto-merge" and "Cancel auto-merge" against a mock GitLab REST API v4, as documented
//! on <https://docs.gitlab.com/api/merge_requests/>: `PUT …/merge` with `auto_merge` (and its
//! deprecated name), `POST …/cancel_auto_merge`, and `cancel_merge_when_pipeline_succeeds` where
//! the host predates it.

use provider_gitlab::GitLabProvider;
use review_core::{Connection, MergeOptions, MergeStrategy, Provider, ProviderKind};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "group/sub/project";
const MR: &str = "/api/v4/projects/group%2Fsub%2Fproject/merge_requests/7";
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
async fn set_auto_merge_sends_the_merge_with_both_auto_merge_names() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/group%2Fsub%2Fproject"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "path_with_namespace": REPO, "name": "project", "web_url": "https://gitlab.example.com/group/sub/project",
            "http_url_to_repo": "https://gitlab.example.com/group/sub/project.git", "merge_method": "merge", "squash_option": "default_off"
        })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("{MR}/merge")))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .and(body_partial_json(json!({
            "auto_merge": true, "merge_when_pipeline_succeeds": true, "sha": "reviewed1",
            "squash": true, "should_remove_source_branch": true, "squash_commit_message": "Squash it"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let options = MergeOptions {
        strategy: MergeStrategy::Squash,
        delete_source_branch: true,
        commit_message: Some("Squash it".into()),
        head_sha: "reviewed1".into(),
    };
    provider(&server).set_auto_complete(REPO, 7, &options).await.expect("auto-merge set");
}

#[tokio::test]
async fn cancel_uses_cancel_auto_merge() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{MR}/cancel_auto_merge")))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{MR}/cancel_merge_when_pipeline_succeeds")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "status": "success" })))
        .expect(0)
        .mount(&server)
        .await;
    provider(&server).cancel_auto_complete(REPO, 7).await.expect("cancelled");
}

/// A host before GitLab 19.5 does not know `cancel_auto_merge`; its older endpoint cancels.
#[tokio::test]
async fn cancel_falls_back_to_the_older_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{MR}/cancel_auto_merge")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "error": "404 Not Found" })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{MR}/cancel_merge_when_pipeline_succeeds")))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "status": "success" })))
        .expect(1)
        .mount(&server)
        .await;
    provider(&server).cancel_auto_complete(REPO, 7).await.expect("cancelled");
}
