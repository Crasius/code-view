//! Retried stages and jobs against a mocked Azure DevOps REST API (7.1), answering with the
//! public timelines of dnceng-public/public build 1639558 (fetched anonymously on 2026-10-19):
//! the run's timeline lists earlier attempts by id only, and an attempt's own timeline
//! (`GET build/builds/{id}/timeline/{timelineId}`) is read only when that attempt is opened.

use provider_azure::AzureDevOpsProvider;
use review_core::{AttemptSource, Connection, Provider, ProviderKind, RunStatus, http};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "dnceng-public/public/aspnetcore";
const RUN: &str = "build:1639558";
const TIMELINE: &str = "/dnceng-public/public/_apis/build/builds/1639558/timeline";
const ATTEMPT_1: &str = "6884a131-87da-5381-61f3-d7acc3b91d76";

fn anonymous(server: &MockServer) -> AzureDevOpsProvider {
    let connection = Connection {
        id: "attempts".to_owned(),
        name: "dnceng-public".to_owned(),
        kind: ProviderKind::AzureDevOps,
        base_url: server.uri(),
        organizations: vec!["dnceng-public".to_owned()],
        token: None,
    };
    AzureDevOpsProvider::new(connection, http::build_client())
}

fn fixture(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "application/json")
}

#[tokio::test]
async fn earlier_attempts_load_only_when_opened() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(TIMELINE))
        .and(query_param("api-version", "7.1"))
        .respond_with(fixture(include_str!("fixtures/timeline_1639558.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{TIMELINE}/{ATTEMPT_1}")))
        .and(query_param("api-version", "7.1"))
        .respond_with(fixture(include_str!("fixtures/timeline_1639558_6884a131.json")))
        .expect(1)
        .mount(&server)
        .await;
    let provider = anonymous(&server);

    let stages = provider.pipeline_jobs(REPO, 69916, RUN).await.expect("the run's stages");
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 1, "the run's stages cost one request, attempts none");
    let build = stages.iter().find(|stage| stage.name == "Build").expect("stage Build");
    let job = build
        .children
        .iter()
        .find(|job| job.name == "Source-Build (Managed)")
        .expect("the retried job");
    let Some(AttemptSource::Lazy(target)) =
        job.previous_attempts.first().map(|attempt| &attempt.source)
    else {
        panic!("the earlier attempt is listed by id: {:?}", job.previous_attempts);
    };

    let attempt = provider.pipeline_attempt(REPO, 69916, RUN, target).await.expect("attempt 1");
    assert_eq!(
        (attempt.name.as_str(), attempt.status, attempt.attempt_number()),
        ("Source-Build (Managed)", RunStatus::Failed, 1)
    );
    assert_eq!(attempt.log_id.as_deref(), Some("1639558:95"), "its log is a log of the same build");
    assert!(attempt.children.iter().any(
        |task| task.status == RunStatus::Failed && task.log_id.as_deref() == Some("1639558:87")
    ));
    assert_eq!(
        attempt.web_url.as_deref(),
        Some(format!("{}/dnceng-public/public/_build/results?buildId=1639558&view=logs&j=2f0d093c-1064-5c86-fc5b-b7b1eca8e66a", server.uri()).as_str())
    );
}

#[tokio::test]
async fn a_malformed_attempt_target_sends_nothing() {
    let server = MockServer::start().await;
    let result = anonymous(&server).pipeline_attempt(REPO, 1, RUN, "1639558").await;
    assert!(result.is_err());
    assert!(server.received_requests().await.unwrap_or_default().is_empty());
}
