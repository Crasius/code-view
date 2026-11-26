//! The merge policy against a mocked Azure DevOps REST API (7.1): `Repositories - Get` for the
//! repository GUID, then `Policy Configurations - Get` scoped by `repositoryId`, `refName` and the
//! "Require a merge strategy" `policyType`, as documented on learn.microsoft.com. Denied reads fall
//! back to every strategy, marked as not reported.

use provider_azure::AzureDevOpsProvider;
use review_core::{
    Connection, MergeOptions, MergeStrategy, PolicySource, Provider, ProviderError, ProviderKind,
    Unreported, http,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const COLLECTION: &str = "coll";
const REPO: &str = "coll/My Project/web";
const REPO_ID: &str = "2459d599-fdb2-4d28-9810-daeec061cf90";
const POLICY_TYPE: &str = "fa4e907d-c16b-4a4c-9dfa-4916e5d171ab";

fn provider(server: &MockServer, token: Option<&str>) -> AzureDevOpsProvider {
    let connection = Connection {
        id: "test".to_owned(),
        name: "test".to_owned(),
        kind: ProviderKind::AzureDevOps,
        base_url: server.uri(),
        organizations: vec![COLLECTION.to_owned()],
        token: token.map(str::to_owned),
    };
    AzureDevOpsProvider::new(connection, http::build_client())
}

async fn mount_repository(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(format!("/{COLLECTION}/My%20Project/_apis/git/repositories/web")))
        .and(query_param("api-version", "7.1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "id": REPO_ID, "name": "web", "project": { "name": "My Project" } }),
        ))
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_policies(server: &MockServer, branch: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(format!("/{COLLECTION}/My%20Project/_apis/git/policy/configurations")))
        .and(query_param("api-version", "7.1"))
        .and(query_param("repositoryId", REPO_ID))
        .and(query_param("refName", format!("refs/heads/{branch}")))
        .and(query_param("policyType", POLICY_TYPE))
        .respond_with(response)
        .expect(1)
        .mount(server)
        .await;
}

fn merge_types(settings: &Value) -> Value {
    json!({ "id": 3, "isEnabled": true, "isBlocking": true, "isDeleted": false,
        "type": { "id": POLICY_TYPE, "displayName": "Require a merge strategy" },
        "settings": settings })
}

#[tokio::test]
async fn reads_the_branch_merge_type_policy() {
    let server = MockServer::start().await;
    mount_repository(&server).await;
    let settings = json!({ "allowNoFastForward": false, "allowSquash": true, "allowRebase": false, "allowRebaseMerge": true,
        "scope": [{ "repositoryId": REPO_ID, "refName": "refs/heads/release/2026", "matchKind": "exact" }] });
    let body = json!({ "count": 1, "value": [merge_types(&settings)] });
    mount_policies(&server, "release/2026", ResponseTemplate::new(200).set_body_json(body)).await;
    let policy = provider(&server, Some("pat")).merge_policy(REPO, "release/2026").await.unwrap();
    assert_eq!(policy.source, PolicySource::Reported);
    assert_eq!(policy.allowed, [MergeStrategy::Squash, MergeStrategy::SemiLinear]);
    assert_eq!(policy.default, Some(MergeStrategy::Squash));
    let url = policy.settings_url.expect("the branch policies page");
    assert!(url.ends_with(&format!("/My%20Project/_settings/repositories?repo={REPO_ID}&_a=policiesMid&refs=refs%2Fheads%2Frelease%2F2026")), "{url}");
}

#[tokio::test]
async fn without_a_policy_every_type_is_allowed() {
    let server = MockServer::start().await;
    mount_repository(&server).await;
    mount_policies(
        &server,
        "main",
        ResponseTemplate::new(200).set_body_json(json!({ "count": 0, "value": [] })),
    )
    .await;
    let policy = provider(&server, Some("pat")).merge_policy(REPO, "main").await.unwrap();
    assert!(policy.is_reported());
    assert_eq!(policy.allowed, MergeStrategy::ALL);
    assert_eq!(
        policy.default,
        Some(MergeStrategy::Merge),
        "no-fast-forward is Azure DevOps' default"
    );
}

#[tokio::test]
async fn a_denied_policy_read_falls_back_to_every_type() {
    let server = MockServer::start().await;
    mount_repository(&server).await;
    let denied = ResponseTemplate::new(403)
        .set_body_json(json!({ "message": "TF401027: You need the Git 'PolicyRead' permission." }));
    mount_policies(&server, "main", denied).await;
    let policy = provider(&server, Some("pat")).merge_policy(REPO, "main").await.unwrap();
    assert_eq!(policy.source, PolicySource::NotReported(Unreported::NoPermission));
    assert_eq!(policy.allowed, MergeStrategy::ALL);
    assert_eq!(policy.default, None);
    assert!(
        policy.settings_url.is_some_and(|url| url.contains("_a=policiesMid")),
        "the hint still links the policies"
    );
}

#[tokio::test]
async fn an_anonymous_private_repository_falls_back_to_the_branches_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/{COLLECTION}/My%20Project/_apis/git/repositories/web")))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    let policy = provider(&server, None).merge_policy(REPO, "main").await.unwrap();
    assert_eq!(policy.source, PolicySource::NotReported(Unreported::NoPermission));
    assert!(
        policy.settings_url.is_some_and(|url| url.ends_with("/My%20Project/_git/web/branches"))
    );
}

#[tokio::test]
async fn other_failures_are_errors() {
    let server = MockServer::start().await;
    mount_repository(&server).await;
    mount_policies(&server, "main", ResponseTemplate::new(500)).await;
    let result = provider(&server, Some("pat")).merge_policy(REPO, "main").await;
    assert!(matches!(result, Err(ProviderError::Status { status: 500, .. })), "{result:?}");
}

#[tokio::test]
async fn semi_linear_completes_with_rebase_merge_and_auto_complete_is_the_detail_default() {
    let server = MockServer::start().await;
    let pr = json!({
        "pullRequestId": 7, "status": "active",
        "repository": { "id": REPO_ID, "name": "web", "project": { "name": "My Project" } },
        "createdBy": { "id": "author", "displayName": "Author" },
        "creationDate": "2026-10-01T08:00:00Z", "title": "T",
        "sourceRefName": "refs/heads/feature", "targetRefName": "refs/heads/main",
        "lastMergeSourceCommit": { "commitId": "head-sha" }, "lastMergeTargetCommit": { "commitId": "target-sha" },
        "autoCompleteSetBy": { "id": "author", "displayName": "Author" },
        "completionOptions": { "mergeStrategy": "rebaseMerge", "deleteSourceBranch": true }
    });
    let pr_path = format!("/{COLLECTION}/My%20Project/_apis/git/repositories/web/pullrequests/7");
    Mock::given(method("GET"))
        .and(path(&pr_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(&pr))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/{COLLECTION}/My%20Project/_apis/git/repositories/web/refs")))
        .and(query_param("filter", "heads/feature"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "value": [{ "name": "refs/heads/feature", "objectId": "head-sha" }] }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(&pr_path))
        .and(body_partial_json(json!({ "completionOptions": { "mergeStrategy": "rebaseMerge" } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(&pr))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, Some("pat"));
    let detail = provider.pull_request(REPO, 7).await.unwrap();
    assert_eq!(
        detail.auto_complete.and_then(|auto| auto.strategy),
        Some(MergeStrategy::SemiLinear)
    );
    let options = MergeOptions {
        strategy: MergeStrategy::SemiLinear,
        delete_source_branch: false,
        commit_message: None,
        head_sha: detail.head_sha,
    };
    provider.merge(REPO, 7, &options).await.unwrap();
}
