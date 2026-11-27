//! The merge policy against a mock GitLab REST API v4: the project's `merge_method` and
//! `squash_option` (`GET /projects/:id`), and merges checked against them before anything is sent.

use review_core::{MergeOptions, MergeStrategy, PolicySource, Provider, ProviderError, Unreported};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{self, TOKEN};

const REPO: &str = "group/sub/project";
const PROJECT: &str = "/api/v4/projects/group%2Fsub%2Fproject";

fn project(settings: Value) -> Value {
    let mut body = json!({
        "id": 42, "path_with_namespace": REPO, "name": "project", "web_url": "https://gitlab.example.com/group/sub/project",
        "default_branch": "main", "http_url_to_repo": "https://gitlab.example.com/group/sub/project.git",
    });
    let (Value::Object(fields), Value::Object(settings)) = (&mut body, settings) else {
        unreachable!("the body and the settings are JSON objects");
    };
    fields.extend(settings);
    body
}

async fn mount_project(server: &MockServer, body: Value) {
    Mock::given(method("GET"))
        .and(path(PROJECT))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn semi_linear_project_with_squash_on_by_default() {
    let server = MockServer::start().await;
    mount_project(
        &server,
        project(json!({ "merge_method": "rebase_merge", "squash_option": "default_on" })),
    )
    .await;
    let policy = support::provider(&server, Some(TOKEN)).merge_policy(REPO, "main").await.unwrap();
    assert_eq!(policy.source, PolicySource::Reported);
    assert_eq!(policy.allowed, [MergeStrategy::SemiLinear, MergeStrategy::Squash]);
    assert_eq!(policy.default, Some(MergeStrategy::Squash), "default_on pre-ticks squash");
    assert_eq!(policy.squash_lands_as, Some(MergeStrategy::SemiLinear));
    assert!(policy.needs_rebased_source);
    assert_eq!(
        policy.settings_url,
        Some(format!("{}/{REPO}/-/settings/merge_requests", server.uri()))
    );
}

/// Anonymous answers (and low roles) leave the merge settings out: merge commit or squash, not reported.
#[tokio::test]
async fn absent_settings_fall_back() {
    let server = MockServer::start().await;
    mount_project(&server, project(json!({}))).await;
    let policy = support::provider(&server, None).merge_policy(REPO, "main").await.unwrap();
    assert_eq!(policy.source, PolicySource::NotReported(Unreported::NoPermission));
    assert_eq!(policy.allowed, [MergeStrategy::Merge, MergeStrategy::Squash]);
    assert!(policy.settings_url.is_some());
}

#[tokio::test]
async fn failed_project_read_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PROJECT))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let result = support::provider(&server, Some(TOKEN)).merge_policy(REPO, "main").await;
    assert!(matches!(result, Err(ProviderError::Status { status: 500, .. })), "{result:?}");
}

#[tokio::test]
async fn fast_forward_project_merges_rebase_and_refuses_a_merge_commit() {
    let server = MockServer::start().await;
    mount_project(
        &server,
        project(json!({ "merge_method": "ff", "squash_option": "default_off" })),
    )
    .await;
    Mock::given(method("PUT"))
        .and(path(format!("{PROJECT}/merge_requests/7/merge")))
        .and(body_json(
            json!({ "sha": "head1", "squash": false, "should_remove_source_branch": true }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "state": "merged" })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = support::provider(&server, Some(TOKEN));
    let rebase = MergeOptions {
        strategy: MergeStrategy::Rebase,
        delete_source_branch: true,
        commit_message: Some("no message for a fast-forward".into()),
        head_sha: "head1".into(),
    };
    provider.merge(REPO, 7, &rebase).await.expect("fast-forward");
    let merge = MergeOptions {
        strategy: MergeStrategy::Merge,
        delete_source_branch: true,
        commit_message: None,
        head_sha: "head1".into(),
    };
    assert!(matches!(provider.merge(REPO, 7, &merge).await, Err(ProviderError::Unsupported(_))));
}

/// A page opening a merge request asks for the git remote, then (once the detail is in) for the
/// merge policy: both read `GET /projects/:project`, which goes out once.
#[tokio::test]
async fn the_git_remote_and_the_merge_policy_share_one_project_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PROJECT))
        .respond_with(ResponseTemplate::new(200).set_body_json(project(
            json!({ "merge_method": "ff", "squash_option": "default_off" }),
        )))
        .expect(1)
        .mount(&server)
        .await;
    let provider = support::provider(&server, Some(TOKEN));
    let remote = provider.git_remote(REPO).await.expect("the remote");
    assert_eq!(remote.clone_url, "https://gitlab.example.com/group/sub/project.git");
    let policy = provider.merge_policy(REPO, "main").await.expect("the policy");
    assert_eq!(policy.allowed, [MergeStrategy::Rebase, MergeStrategy::Squash]);
    server.verify().await;
}
