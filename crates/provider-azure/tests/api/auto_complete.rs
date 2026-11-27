//! "Set auto-complete" and "Cancel auto-complete" against a mocked Azure DevOps REST API (7.1):
//! `Pull Requests - Update` with `autoCompleteSetBy` and `completionOptions`, documented at
//! learn.microsoft.com/rest/api/azure/devops/git/pull-requests/update, and the state the detail reads.

use review_core::{MergeOptions, MergeStrategy, Provider, ProviderError};
use serde_json::json;
use wiremock::matchers::{body_json, body_partial_json, method, path};
use wiremock::{Mock, MockServer};

use crate::support::{
    COLLECTION, PR, REPO, TOKEN, api, mount_source_tip, ok, pr_path, provider, pull_request,
};

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
    api("GET", &pr_path("")).respond_with(ok(pull_request(&json!({})))).mount(&server).await;
    mount_source_tip(&server, "head-sha", None).await;
    Mock::given(method("GET"))
        .and(path(format!("/{COLLECTION}/_apis/connectionData")))
        .respond_with(ok(
            json!({ "authenticatedUser": { "id": "me-id", "providerDisplayName": "Me" } }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    api("PATCH", &pr_path(""))
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
    provider(&server, &[COLLECTION], Some(TOKEN))
        .set_auto_complete(REPO, PR, &options("head-sha"))
        .await
        .unwrap();
}

/// The update takes no guard commit: a source branch past the reviewed commit is refused first.
#[tokio::test]
async fn set_auto_complete_refuses_a_source_branch_pushed_past_the_review() {
    let server = MockServer::start().await;
    api("GET", &pr_path("")).respond_with(ok(pull_request(&json!({})))).mount(&server).await;
    mount_source_tip(&server, "pushed-sha", None).await;
    api("PATCH", &pr_path(""))
        .respond_with(ok(pull_request(&json!({}))))
        .expect(0)
        .mount(&server)
        .await;
    let error = provider(&server, &[COLLECTION], Some(TOKEN))
        .set_auto_complete(REPO, PR, &options("head-sha"))
        .await
        .expect_err("the branch moved");
    assert!(matches!(error, ProviderError::HeadMoved), "{error:?}");
}

#[tokio::test]
async fn cancel_auto_complete_clears_who_set_it() {
    let server = MockServer::start().await;
    api("PATCH", &pr_path(""))
        .and(body_partial_json(json!({
            "autoCompleteSetBy": { "id": "00000000-0000-0000-0000-000000000000" }
        })))
        .respond_with(ok(pull_request(&json!({}))))
        .expect(1)
        .mount(&server)
        .await;
    provider(&server, &[COLLECTION], Some(TOKEN)).cancel_auto_complete(REPO, PR).await.unwrap();
}

#[tokio::test]
async fn the_detail_names_who_set_auto_complete_and_its_strategy() {
    let server = MockServer::start().await;
    let set = pull_request(&json!({
        "autoCompleteSetBy": { "id": "u9", "displayName": "Ada Lovelace" },
        "completionOptions": { "mergeStrategy": "rebase" }
    }));
    api("GET", &pr_path("")).respond_with(ok(set)).mount(&server).await;
    let detail =
        provider(&server, &[COLLECTION], Some(TOKEN)).pull_request(REPO, PR).await.unwrap();
    let auto_complete = detail.auto_complete.expect("auto-complete is set");
    assert_eq!(auto_complete.strategy, Some(MergeStrategy::Rebase));
    assert_eq!(auto_complete.set_by.map(|user| user.display_name).as_deref(), Some("Ada Lovelace"));
}
