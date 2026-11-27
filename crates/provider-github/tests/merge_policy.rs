//! The merge policy against a mock GitHub Enterprise host: the repository's `allow_*_merge`
//! settings (`GET /repos/{owner}/{repo}`, write access only) and the branch's `pull_request`
//! rules (`GET /repos/{owner}/{repo}/rules/branches/{branch}`), as documented for REST 2022-11-28.

use provider_github::GitHubProvider;
use review_core::{
    Connection, MergeStrategy, PolicySource, Provider, ProviderError, ProviderKind, Unreported,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "contoso/app";

async fn setup(token: Option<&str>) -> (MockServer, GitHubProvider) {
    let server = MockServer::start().await;
    let connection = Connection {
        id: "mock".to_owned(),
        name: "Mock GitHub Enterprise".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: token.map(str::to_owned),
    };
    (server, GitHubProvider::new(connection, reqwest::Client::new()))
}

fn repository(settings: Value) -> Value {
    let mut body = json!({ "full_name": REPO, "name": "app", "html_url": "https://ghe.example/contoso/app", "default_branch": "trunk" });
    let (Value::Object(fields), Value::Object(settings)) = (&mut body, settings) else {
        unreachable!("the body and the settings are JSON objects");
    };
    fields.extend(settings);
    body
}

async fn mount(server: &MockServer, repository: Value, branch: &str, rules: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(repository))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}/rules/branches/{branch}")))
        .and(query_param("per_page", "100"))
        .respond_with(rules)
        .expect(1)
        .mount(server)
        .await;
}

fn rules(methods: &[&str]) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!([
        { "type": "deletion", "ruleset_source_type": "Repository", "ruleset_source": REPO, "ruleset_id": 1 },
        { "type": "pull_request", "parameters": { "required_approving_review_count": 1, "allowed_merge_methods": methods },
          "ruleset_source_type": "Organization", "ruleset_source": "contoso", "ruleset_id": 2 },
    ]))
}

#[tokio::test]
async fn repository_settings_and_branch_rules_both_apply() {
    let (server, provider) = setup(Some("token")).await;
    let settings = json!({ "allow_merge_commit": true, "allow_squash_merge": true, "allow_rebase_merge": false });
    mount(&server, repository(settings), "release/v2", rules(&["squash", "rebase"])).await;
    let policy = provider.merge_policy(REPO, "release/v2").await.unwrap();
    assert_eq!(policy.source, PolicySource::Reported);
    assert_eq!(policy.allowed, [MergeStrategy::Squash]);
    assert_eq!(
        policy.default,
        Some(MergeStrategy::Squash),
        "the only allowed method is the default"
    );
    assert_eq!(
        policy.settings_url.as_deref(),
        Some(format!("{}/contoso/app/settings", server.uri()).as_str())
    );
}

#[tokio::test]
async fn several_allowed_methods_have_no_default() {
    let (server, provider) = setup(Some("token")).await;
    let settings = json!({ "allow_merge_commit": true, "allow_squash_merge": true, "allow_rebase_merge": true });
    mount(
        &server,
        repository(settings),
        "trunk",
        ResponseTemplate::new(200).set_body_json(json!([])),
    )
    .await;
    let policy = provider.merge_policy(REPO, "trunk").await.unwrap();
    assert!(policy.is_reported());
    assert_eq!(
        policy.allowed,
        [MergeStrategy::Merge, MergeStrategy::Squash, MergeStrategy::Rebase]
    );
    assert_eq!(policy.default, None, "GitHub stores no default method");
}

/// Without write access (or anonymously) the settings are absent: every method, narrowed by the
/// rules everybody can read, marked as not reported.
#[tokio::test]
async fn settings_without_write_access_fall_back_narrowed_by_rules() {
    let (server, provider) = setup(None).await;
    mount(&server, repository(json!({})), "trunk", rules(&["merge", "squash"])).await;
    let policy = provider.merge_policy(REPO, "trunk").await.unwrap();
    assert_eq!(policy.source, PolicySource::NotReported(Unreported::NoPermission));
    assert_eq!(policy.allowed, [MergeStrategy::Merge, MergeStrategy::Squash]);
    assert_eq!(policy.default, None);
}

#[tokio::test]
async fn hosts_without_rulesets_use_the_settings_alone() {
    let (server, provider) = setup(Some("token")).await;
    let settings = json!({ "allow_merge_commit": false, "allow_squash_merge": true, "allow_rebase_merge": true });
    mount(
        &server,
        repository(settings),
        "trunk",
        ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
    )
    .await;
    let policy = provider.merge_policy(REPO, "trunk").await.unwrap();
    assert_eq!(policy.allowed, [MergeStrategy::Squash, MergeStrategy::Rebase]);
}

/// GitHub Free has no rulesets for private repositories and answers the rules read with 403
/// (observed live on github.com, 2026-10-24); no rules apply there, the token is fine.
#[tokio::test]
async fn private_repositories_without_rulesets_use_the_settings_alone() {
    let (server, provider) = setup(Some("token")).await;
    let settings = json!({ "allow_merge_commit": true, "allow_squash_merge": false, "allow_rebase_merge": true });
    let plan = json!({
        "message": "Upgrade to GitHub Pro or make this repository public to enable this feature.",
        "documentation_url": "https://docs.github.com/rest/repos/rules#get-rules-for-a-branch",
        "status": "403"
    });
    mount(&server, repository(settings), "trunk", ResponseTemplate::new(403).set_body_json(plan))
        .await;
    let policy = provider.merge_policy(REPO, "trunk").await.unwrap();
    assert!(policy.is_reported());
    assert_eq!(policy.allowed, [MergeStrategy::Merge, MergeStrategy::Rebase]);
}

#[tokio::test]
async fn a_failed_repository_read_is_an_error() {
    let (server, provider) = setup(Some("token")).await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}")))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let result = provider.merge_policy(REPO, "trunk").await;
    assert!(matches!(result, Err(ProviderError::Status { status: 500, .. })), "{result:?}");
}
