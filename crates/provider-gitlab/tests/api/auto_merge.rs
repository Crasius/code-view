//! "Set to auto-merge" and "Cancel auto-merge" against a mock GitLab REST API v4, as documented
//! on <https://docs.gitlab.com/api/merge_requests/>: `PUT …/merge` with `auto_merge` (and its
//! deprecated name), `POST …/cancel_auto_merge`, and `cancel_merge_when_pipeline_succeeds` where
//! the host predates it.

use review_core::{MergeOptions, MergeStrategy, Provider};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{self, REPO, TOKEN};

const MR: &str = "/api/v4/projects/group%2Fsub%2Fproject/merge_requests/7";

#[tokio::test]
async fn set_auto_merge_sends_the_merge_with_both_auto_merge_names() {
    let server = MockServer::start().await;
    let settings = json!({ "merge_method": "merge", "squash_option": "default_off" });
    support::mount_project(&server, support::project(settings)).await;
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
    support::provider(&server, Some(TOKEN))
        .set_auto_complete(REPO, 7, &options)
        .await
        .expect("auto-merge set");
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
    support::provider(&server, Some(TOKEN)).cancel_auto_complete(REPO, 7).await.expect("cancelled");
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
    support::provider(&server, Some(TOKEN)).cancel_auto_complete(REPO, 7).await.expect("cancelled");
}
