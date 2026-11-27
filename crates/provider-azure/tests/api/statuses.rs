//! Pull request statuses against a mock Azure DevOps REST API (api-version 7.1): a status posted
//! on an earlier iteration is an older run; the iterations are read only when a status names one.

use provider_azure::AzureDevOpsProvider;
use review_core::{Connection, Provider, ProviderKind};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PR: &str = "/coll/Web/_apis/git/repositories/site/pullrequests/7";

fn provider(server: &MockServer) -> AzureDevOpsProvider {
    let connection = Connection {
        id: "azure-statuses".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::AzureDevOps,
        base_url: server.uri(),
        organizations: vec!["coll".to_owned()],
        token: Some("dummy-pat".to_owned()),
    };
    AzureDevOpsProvider::new(connection, review_core::http::build_client())
}

async fn mount_without_builds(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/coll/Web/_apis/build/builds"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "count": 0, "value": [] })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn statuses_of_an_earlier_iteration_are_older_runs() {
    let server = MockServer::start().await;
    mount_without_builds(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("{PR}/statuses")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "count": 2, "value": [
            { "id": 1, "state": "failed", "iterationId": 1, "context": { "name": "sonar" } },
            { "id": 2, "state": "succeeded", "iterationId": 2, "context": { "name": "lint" } }
        ] })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{PR}/iterations")))
        .and(query_param("includeCommits", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "count": 2, "value": [
            { "id": 1 }, { "id": 2 }
        ] })))
        .expect(1)
        .mount(&server)
        .await;
    let runs = provider(&server).pipeline_runs("coll/Web/site", 7).await.expect("runs");
    let heads: Vec<(&str, bool)> =
        runs.iter().map(|run| (run.name.as_str(), run.is_head)).collect();
    assert_eq!(heads, [("lint", true), ("sonar", false)]);
}

#[tokio::test]
async fn statuses_of_the_pull_request_need_no_iterations() {
    let server = MockServer::start().await;
    mount_without_builds(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("{PR}/statuses")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "count": 1, "value": [
            { "id": 1, "state": "failed", "context": { "name": "bot" } }
        ] })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{PR}/iterations")))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let runs = provider(&server).pipeline_runs("coll/Web/site", 7).await.expect("runs");
    assert!(runs.iter().all(|run| run.is_head), "{runs:?}");
}
