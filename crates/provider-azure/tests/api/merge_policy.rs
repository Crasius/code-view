//! The merge policy against a mocked Azure DevOps REST API (7.1): `Repositories - Get` for the
//! repository GUID, then `Policy Configurations - Get` scoped by `repositoryId`, `refName` and the
//! "Require a merge strategy" `policyType`, as documented on learn.microsoft.com. Denied reads fall
//! back to every strategy, marked as not reported.

use review_core::{MergeStrategy, PolicySource, Provider, ProviderError, Unreported};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{COLLECTION, REPO, provider};

const REPO_ID: &str = "2459d599-fdb2-4d28-9810-daeec061cf90";
const POLICY_TYPE: &str = "fa4e907d-c16b-4a4c-9dfa-4916e5d171ab";

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
    let policy = provider(&server, &[COLLECTION], Some("pat"))
        .merge_policy(REPO, "release/2026")
        .await
        .unwrap();
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
    let policy =
        provider(&server, &[COLLECTION], Some("pat")).merge_policy(REPO, "main").await.unwrap();
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
    let policy =
        provider(&server, &[COLLECTION], Some("pat")).merge_policy(REPO, "main").await.unwrap();
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
    let policy = provider(&server, &[COLLECTION], None).merge_policy(REPO, "main").await.unwrap();
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
    let result = provider(&server, &[COLLECTION], Some("pat")).merge_policy(REPO, "main").await;
    assert!(matches!(result, Err(ProviderError::Status { status: 500, .. })), "{result:?}");
}
