//! Token inspection and group discovery against a mock GitLab: read-only GETs.

use review_core::Provider;
use review_core::setup::{Access, Feature};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

use crate::support::{TOKEN, provider};

fn get(route: &str) -> MockBuilder {
    Mock::given(method("GET"))
        .and(path(format!("/api/v4{route}")))
        .and(header("private-token", TOKEN))
}

async fn mount_user(server: &MockServer) {
    get("/user")
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": 7, "username": "ada", "name": "Ada"})),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn api_scope_grants_everything() {
    let server = MockServer::start().await;
    get("/personal_access_tokens/self")
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"scopes": ["api"], "expires_at": "2026-12-01"})),
        )
        .mount(&server)
        .await;
    mount_user(&server).await;
    let report = provider(&server, Some(TOKEN)).inspect_access().await.expect("report");
    assert_eq!(report.user.login.as_deref(), Some("ada"));
    assert_eq!(report.expires_at.as_deref(), Some("2026-12-01"));
    assert!(report.checks.iter().all(|check| check.access == Access::Granted));
}

#[tokio::test]
async fn read_api_token_reads_but_cannot_write() {
    let server = MockServer::start().await;
    get("/personal_access_tokens/self")
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"scopes": ["read_api"]})))
        .mount(&server)
        .await;
    mount_user(&server).await;
    let report = provider(&server, Some(TOKEN)).inspect_access().await.expect("report");
    let access = |feature| report.check(feature).map(|check| check.access);
    assert_eq!(access(Feature::ReadPullRequests), Some(Access::Granted));
    assert_eq!(access(Feature::Vote), Some(Access::Missing));
    assert_eq!(access(Feature::GitClone), Some(Access::Missing));
}

#[tokio::test]
async fn fine_grained_token_missing_merge_request_read() {
    let server = MockServer::start().await;
    get("/personal_access_tokens/self")
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"message": "403 Forbidden"})))
        .mount(&server)
        .await;
    mount_user(&server).await;
    get("/projects")
        .and(query_param("membership", "true"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{"id": 42, "path_with_namespace": "acme/app"}])),
        )
        .mount(&server)
        .await;
    let denied = ResponseTemplate::new(403).set_body_json(json!({
        "message": "Access denied: This operation requires a fine-grained personal access token with the following project permissions: [Merge Request: Read]."
    }));
    get("/projects/42/merge_requests").respond_with(denied).mount(&server).await;
    let empty = || ResponseTemplate::new(200).set_body_json(json!([]));
    get("/projects/42/labels").respond_with(empty()).mount(&server).await;
    get("/projects/42/pipelines").respond_with(empty()).mount(&server).await;
    get("/projects/42/jobs").respond_with(empty()).mount(&server).await;

    let report = provider(&server, Some(TOKEN)).inspect_access().await.expect("report");
    let access = |feature| report.check(feature).map(|check| check.access);
    assert_eq!(report.token_kind, "Fine-grained personal access token");
    assert_eq!(access(Feature::ReadPullRequests), Some(Access::Missing));
    assert_eq!(access(Feature::Pipelines), Some(Access::Granted));
    assert_eq!(access(Feature::Vote), Some(Access::Unverified));
    assert!(!report.is_usable());
    let requests = server.received_requests().await.expect("recorded");
    assert!(requests.iter().all(|request| request.method.as_str() == "GET"));
}

#[tokio::test]
async fn granular_scopes_decide_write_permissions() {
    let server = MockServer::start().await;
    get("/personal_access_tokens/self")
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "scopes": [],
            "granular_scopes": [{"access": "all_memberships", "permissions": ["read_merge_request", "approve_merge_request", "read_job"], "project_id": null, "group_id": null}]
        })))
        .mount(&server)
        .await;
    mount_user(&server).await;
    get("/projects")
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{"id": 1, "path_with_namespace": "a/b"}])),
        )
        .mount(&server)
        .await;
    let empty = || ResponseTemplate::new(200).set_body_json(json!([]));
    for route in [
        "/projects/1/merge_requests",
        "/projects/1/labels",
        "/projects/1/pipelines",
        "/projects/1/jobs",
    ] {
        get(route).respond_with(empty()).mount(&server).await;
    }
    let report = provider(&server, Some(TOKEN)).inspect_access().await.expect("report");
    let access = |feature| report.check(feature).map(|check| check.access);
    assert_eq!(access(Feature::Vote), Some(Access::Granted));
    assert_eq!(access(Feature::Merge), Some(Access::Missing));
    assert_eq!(report.scopes, ["read_merge_request", "approve_merge_request", "read_job"]);
}

#[tokio::test]
async fn discovers_top_level_groups() {
    let server = MockServer::start().await;
    get("/groups")
        .and(query_param("min_access_level", "10"))
        .and(query_param("top_level_only", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 1, "full_path": "acme", "full_name": "Acme"},
            {"id": 2, "full_path": "tools", "full_name": ""}
        ])))
        .mount(&server)
        .await;
    let scopes = provider(&server, Some(TOKEN)).discover_scopes().await.expect("groups");
    let found: Vec<(&str, &str)> =
        scopes.iter().map(|scope| (scope.key.as_str(), scope.display_name.as_str())).collect();
    assert_eq!(found, [("acme", "Acme"), ("tools", "tools")]);
}
