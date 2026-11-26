//! "Set auto-complete" and "Cancel auto-complete" against a mocked Azure DevOps REST API (7.1):
//! `Pull Requests - Update` with `autoCompleteSetBy` and `completionOptions`, documented at
//! learn.microsoft.com/rest/api/azure/devops/git/pull-requests/update, and the state the detail reads.

use provider_azure::AzureDevOpsProvider;
use review_core::{
    Connection, MergeOptions, MergeStrategy, Provider, ProviderError, ProviderKind, http,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

const COLLECTION: &str = "coll";
const REPO: &str = "coll/My Project/web";
const PR: u64 = 7;
/// `base64(":dummy-pat")`.
const AUTHORIZATION: &str = "Basic OmR1bW15LXBhdA==";

fn provider(server: &MockServer) -> AzureDevOpsProvider {
    let connection = Connection {
        id: "test".to_owned(),
        name: "test".to_owned(),
        kind: ProviderKind::AzureDevOps,
        base_url: server.uri(),
        organizations: vec![COLLECTION.to_owned()],
        token: Some("dummy-pat".to_owned()),
    };
    AzureDevOpsProvider::new(connection, http::build_client())
}

fn pr_path() -> String {
    format!("/{COLLECTION}/My%20Project/_apis/git/repositories/web/pullrequests/{PR}")
}

fn api(verb: &str, url_path: &str) -> MockBuilder {
    Mock::given(method(verb))
        .and(path(url_path))
        .and(query_param("api-version", "7.1"))
        .and(header("authorization", AUTHORIZATION))
}

fn ok(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

fn pull_request(extra: &Value) -> Value {
    let mut value = json!({
        "pullRequestId": PR, "status": "active",
        "repository": { "name": "web", "project": { "name": "My Project" } },
        "createdBy": { "id": "author", "displayName": "Author" },
        "creationDate": "2026-10-01T08:00:00Z", "title": "T",
        "sourceRefName": "refs/heads/feature", "targetRefName": "refs/heads/main",
        "lastMergeSourceCommit": { "commitId": "head-sha" }, "lastMergeTargetCommit": { "commitId": "target-sha" }
    });
    value.as_object_mut().expect("an object").extend(extra.as_object().expect("an object").clone());
    value
}

async fn mount_source_tip(server: &MockServer, tip: &str) {
    api("GET", &format!("/{COLLECTION}/My%20Project/_apis/git/repositories/web/refs"))
        .and(query_param("filter", "heads/feature"))
        .respond_with(ok(json!({ "value": [{ "name": "refs/heads/feature", "objectId": tip }] })))
        .mount(server)
        .await;
}

fn options(head_sha: &str) -> MergeOptions {
    MergeOptions {
        strategy: MergeStrategy::Squash,
        delete_source_branch: true,
        commit_message: Some("Merged PR 7: T".to_owned()),
        head_sha: head_sha.to_owned(),
    }
}

/// The signed-in user of the organization sets it, with the completion the user chose.
#[tokio::test]
async fn set_auto_complete_names_the_signed_in_user_and_the_completion() {
    let server = MockServer::start().await;
    api("GET", &pr_path()).respond_with(ok(pull_request(&json!({})))).mount(&server).await;
    mount_source_tip(&server, "head-sha").await;
    Mock::given(method("GET"))
        .and(path(format!("/{COLLECTION}/_apis/connectionData")))
        .respond_with(ok(
            json!({ "authenticatedUser": { "id": "me-id", "providerDisplayName": "Me" } }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    api("PATCH", &pr_path())
        .and(body_json(json!({
            "autoCompleteSetBy": { "id": "me-id" },
            "completionOptions": {
                "mergeStrategy": "squash", "deleteSourceBranch": true,
                "mergeCommitMessage": "Merged PR 7: T"
            }
        })))
        .respond_with(ok(pull_request(&json!({}))))
        .expect(1)
        .mount(&server)
        .await;
    provider(&server).set_auto_complete(REPO, PR, &options("head-sha")).await.unwrap();
}

/// The update takes no guard commit: a source branch past the reviewed commit is refused first.
#[tokio::test]
async fn set_auto_complete_refuses_a_source_branch_pushed_past_the_review() {
    let server = MockServer::start().await;
    api("GET", &pr_path()).respond_with(ok(pull_request(&json!({})))).mount(&server).await;
    mount_source_tip(&server, "pushed-sha").await;
    api("PATCH", &pr_path())
        .respond_with(ok(pull_request(&json!({}))))
        .expect(0)
        .mount(&server)
        .await;
    let error = provider(&server)
        .set_auto_complete(REPO, PR, &options("head-sha"))
        .await
        .expect_err("the branch moved");
    assert!(matches!(error, ProviderError::HeadMoved), "{error:?}");
}

#[tokio::test]
async fn cancel_auto_complete_clears_who_set_it() {
    let server = MockServer::start().await;
    api("PATCH", &pr_path())
        .and(body_partial_json(json!({
            "autoCompleteSetBy": { "id": "00000000-0000-0000-0000-000000000000" }
        })))
        .respond_with(ok(pull_request(&json!({}))))
        .expect(1)
        .mount(&server)
        .await;
    provider(&server).cancel_auto_complete(REPO, PR).await.unwrap();
}

#[tokio::test]
async fn the_detail_names_who_set_auto_complete_and_its_strategy() {
    let server = MockServer::start().await;
    let set = pull_request(&json!({
        "autoCompleteSetBy": { "id": "u9", "displayName": "Ada Lovelace" },
        "completionOptions": { "mergeStrategy": "rebase" }
    }));
    api("GET", &pr_path()).respond_with(ok(set)).mount(&server).await;
    let detail = provider(&server).pull_request(REPO, PR).await.unwrap();
    let auto_complete = detail.auto_complete.expect("auto-complete is set");
    assert_eq!(auto_complete.strategy, Some(MergeStrategy::Rebase));
    assert_eq!(auto_complete.set_by.map(|user| user.display_name).as_deref(), Some("Ada Lovelace"));
}
