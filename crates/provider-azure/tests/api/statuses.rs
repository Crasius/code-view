//! Pull request statuses against a mock Azure DevOps REST API (api-version 7.1): a status posted
//! on an earlier iteration is an older run; the iterations are read only when a status names one.

use review_core::Provider;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{COLLECTION, TOKEN, provider};

const PR: &str = "/coll/Web/_apis/git/repositories/site/pullrequests/7";

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
    let runs = provider(&server, &[COLLECTION], Some(TOKEN))
        .pipeline_runs("coll/Web/site", 7)
        .await
        .expect("runs");
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
    let runs = provider(&server, &[COLLECTION], Some(TOKEN))
        .pipeline_runs("coll/Web/site", 7)
        .await
        .expect("runs");
    assert!(runs.iter().all(|run| run.is_head), "{runs:?}");
}
