//! "Enable auto-merge" and "Disable auto-merge" against a mock GitHub Enterprise host: the
//! GraphQL mutations `enablePullRequestAutoMerge` and `disablePullRequestAutoMerge` as documented
//! at docs.github.com/graphql/reference/pulls, on the node id the REST pull request names.

use provider_github::GitHubProvider;
use review_core::{Connection, MergeOptions, MergeStrategy, Provider, ProviderError, ProviderKind};
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, body_string_contains, header, method, path};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

const REPO: &str = "contoso/app";
const NUMBER: u64 = 7;
const PULL_NODE_ID: &str = "PR_kwDOAbc";

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
    let provider = GitHubProvider::new(connection, reqwest::Client::new());
    let pull: Value =
        serde_json::from_str(include_str!("../fixtures/pull_request.json")).expect("fixture");
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}/pulls/{NUMBER}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .mount(&server)
        .await;
    (server, provider)
}

fn graphql() -> MockBuilder {
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(header("authorization", "Bearer test-token"))
}

fn squash() -> MergeOptions {
    MergeOptions {
        strategy: MergeStrategy::Squash,
        delete_source_branch: true,
        commit_message: Some("Add feature (#7)\n\nDetails".to_owned()),
        head_sha: "reviewed000".to_owned(),
    }
}

#[tokio::test]
async fn enable_sends_the_method_message_and_reviewed_head() {
    let (server, provider) = setup().await;
    graphql()
        .and(body_string_contains("enablePullRequestAutoMerge(input:"))
        .and(body_partial_json(json!({ "variables": {
            "id": PULL_NODE_ID, "method": "SQUASH", "head": "reviewed000",
            "headline": "Add feature (#7)", "body": "Details"
        } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enablePullRequestAutoMerge": { "pullRequest": { "id": PULL_NODE_ID } } }
        })))
        .expect(1)
        .mount(&server)
        .await;
    provider.set_auto_complete(REPO, NUMBER, &squash()).await.expect("auto-merge enabled");
}

/// A repository that does not allow auto-merge: the error names the settings page that fixes it.
#[tokio::test]
async fn enable_on_a_repository_without_auto_merge_points_to_its_settings() {
    let (server, provider) = setup().await;
    graphql()
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enablePullRequestAutoMerge": null },
            "errors": [{ "type": "UNPROCESSABLE", "message": "Pull request Auto merge is not allowed for this repository" }]
        })))
        .mount(&server)
        .await;
    let error = provider.set_auto_complete(REPO, NUMBER, &squash()).await.expect_err("refused");
    let ProviderError::Unsupported(message) = error else {
        panic!("unexpected {error:?}");
    };
    assert!(message.contains(&format!("{}/contoso/app/settings", server.uri())), "{message}");
}

#[tokio::test]
async fn disable_sends_the_node_id() {
    let (server, provider) = setup().await;
    graphql()
        .and(body_string_contains("disablePullRequestAutoMerge(input: { pullRequestId: $id })"))
        .and(body_partial_json(json!({ "variables": { "id": PULL_NODE_ID } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "disablePullRequestAutoMerge": { "pullRequest": { "id": PULL_NODE_ID } } }
        })))
        .expect(1)
        .mount(&server)
        .await;
    provider.cancel_auto_complete(REPO, NUMBER).await.expect("auto-merge disabled");
}

#[tokio::test]
async fn auto_merge_on_the_detail_names_method_and_author() {
    let (server, provider) = setup().await;
    let mut pull: Value =
        serde_json::from_str(include_str!("../fixtures/pull_request.json")).expect("fixture");
    pull["auto_merge"] = json!({ "enabled_by": { "login": "mona", "id": 1 }, "merge_method": "squash", "commit_title": "T", "commit_message": "" });
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}/pulls/{NUMBER}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}/pulls/{NUMBER}/reviews")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let detail = provider.pull_request(REPO, NUMBER).await.expect("detail");
    let auto_complete = detail.auto_complete.expect("auto-merge is on");
    assert_eq!(auto_complete.strategy, Some(MergeStrategy::Squash));
    assert_eq!(auto_complete.set_by.and_then(|user| user.login).as_deref(), Some("mona"));
}
