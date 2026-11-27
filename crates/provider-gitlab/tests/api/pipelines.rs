//! Pipeline reads and retries against a mock GitLab: REST v4 jobs, GraphQL `needs` and the
//! documented retry endpoints (docs.gitlab.com/api/jobs, /api/pipelines, /api/graphql).

use provider_gitlab::GitLabProvider;
use review_core::{
    Connection, PipelineJob, Provider, ProviderError, ProviderKind, RerunAction, RerunKind,
};
use serde_json::{Value, json};
use wiremock::matchers::{
    body_partial_json, body_string_contains, header, method, path, query_param,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "test-token";
const RUN: &str = "pipeline:7:99";

fn provider(server: &MockServer, token: Option<&str>) -> GitLabProvider {
    let connection = Connection {
        id: "mock".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::GitLab,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: token.map(str::to_owned),
    };
    GitLabProvider::new(connection, reqwest::Client::new())
}

async fn mount_rest_jobs(server: &MockServer) {
    let jobs = json!([
        { "id": 1, "name": "compile", "stage": "build", "status": "success" },
        { "id": 2, "name": "lint", "stage": "test", "status": "success" },
        { "id": 3, "name": "docs", "stage": "docs", "status": "failed" }
    ]);
    // Retried jobs come in the same listing: no extra request for earlier attempts.
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/7/pipelines/99/jobs"))
        .and(query_param("include_retried", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(jobs))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/7/pipelines/99/bridges"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;
}

fn graphql_jobs(nodes: &Value) -> Value {
    json!({ "data": { "projects": { "nodes": [ { "pipeline": { "jobs": {
        "pageInfo": { "hasNextPage": false, "endCursor": null },
        "nodes": nodes
    } } } ] } } })
}

/// `compile` is stage-scheduled, `lint` needs `compile`, `docs` needs nothing.
fn needs_nodes() -> Value {
    json!([
        { "name": "compile", "schedulingType": "stage", "needs": { "nodes": [] } },
        { "name": "lint", "schedulingType": "dag", "needs": { "nodes": [ { "name": "compile" } ] } },
        { "name": "docs", "schedulingType": "dag", "needs": { "nodes": [] } }
    ])
}

fn stage_dependencies(stages: &[PipelineJob]) -> Vec<Vec<&str>> {
    stages.iter().map(|stage| stage.depends_on.iter().map(String::as_str).collect()).collect()
}

#[tokio::test]
async fn jobs_carry_their_needs_from_graphql_and_stages_keep_their_order() {
    let server = MockServer::start().await;
    mount_rest_jobs(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(body_partial_json(json!({ "variables": {
            "project": "gid://gitlab/Project/7", "pipeline": "gid://gitlab/Ci::Pipeline/99", "after": null
        } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(graphql_jobs(&needs_nodes())))
        .expect(1)
        .mount(&server)
        .await;
    let stages =
        provider(&server, Some(TOKEN)).pipeline_jobs("group/project", 1, RUN).await.expect("jobs");
    let dependencies: Vec<(&str, Vec<&str>)> = stages
        .iter()
        .map(|stage| (stage.name.as_str(), stage.depends_on.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(
        dependencies,
        vec![("build", vec![]), ("test", vec!["stage:99:build"]), ("docs", vec!["stage:99:test"])]
    );
    assert_eq!(stages[1].children[0].depends_on, vec!["job:1".to_owned()]);
    assert!(
        stages[0].children[0].depends_on.is_empty() && stages[2].children[0].depends_on.is_empty()
    );
}

/// `Project.pipeline(id:)` came with GitLab 17.2. An older host rejects the argument (graphql-ruby's
/// `argumentNotAccepted`); the needs then come by the pipeline's iid from the REST pipeline, and
/// later loads ask by iid at once.
#[tokio::test]
async fn a_host_without_pipeline_by_id_gets_the_needs_by_iid() {
    let server = MockServer::start().await;
    mount_rest_jobs(&server).await;
    let rejected = json!({ "errors": [ {
        "message": "Field 'pipeline' doesn't accept argument 'id'",
        "locations": [ { "line": 2, "column": 35 } ],
        "path": [ "query", "projects", "nodes", "pipeline", "id" ],
        "extensions": { "code": "argumentNotAccepted", "name": "pipeline", "typeName": "Field", "argumentName": "id" }
    } ] });
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains("pipeline(id:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(rejected))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/7/pipelines/99"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "id": 99, "iid": 12, "status": "running" })),
        )
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains("pipeline(iid:"))
        .and(body_partial_json(json!({ "variables": { "pipeline": "12" } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(graphql_jobs(&needs_nodes())))
        .expect(2)
        .mount(&server)
        .await;
    let provider = provider(&server, Some(TOKEN));

    for load in ["first", "second"] {
        let stages = provider.pipeline_jobs("group/project", 1, RUN).await.expect(load);
        assert_eq!(stages[1].children[0].depends_on, vec!["job:1".to_owned()], "{load} load");
    }
}

#[tokio::test]
async fn stages_run_in_order_when_graphql_fails() {
    let server = MockServer::start().await;
    mount_rest_jobs(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "errors": [ { "message": "Field 'schedulingType' doesn't exist" } ] }),
        ))
        .mount(&server)
        .await;
    let stages =
        provider(&server, None).pipeline_jobs("group/project", 1, RUN).await.expect("jobs");
    assert_eq!(
        stage_dependencies(&stages),
        vec![vec![], vec!["stage:99:build"], vec!["stage:99:test"]]
    );
}

/// gitlab.com limits anonymous GraphQL per IP far below REST and answers with a `Retry-After` up to
/// the next full hour (headers as observed on gitlab.com/api/graphql). The jobs load at once
/// without their needs, REST requests to the same host go on, and the paused GraphQL is not asked
/// again until the pause ends.
#[tokio::test]
async fn a_rate_limited_graphql_neither_holds_back_the_jobs_nor_pauses_rest() {
    let server = MockServer::start().await;
    mount_rest_jobs(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "2391")
                .insert_header("RateLimit-Name", "unauthenticated_traffic_per_ip")
                .insert_header("RateLimit-Remaining", "0")
                .set_body_string("Rate limit exceeded; see https://docs.gitlab.com/ee/user/gitlab_com/#gitlabcom-specific-rate-limits for more details\n"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, None);
    let patience = std::time::Duration::from_secs(10);

    let stages = tokio::time::timeout(patience, provider.pipeline_jobs("group/project", 1, RUN))
        .await
        .expect("the jobs do not wait for the GraphQL limit")
        .expect("jobs");
    assert_eq!(
        stage_dependencies(&stages),
        vec![vec![], vec!["stage:99:build"], vec!["stage:99:test"]],
        "stages in order, without needs"
    );

    let notices = review_core::http::rate_limits();
    let notice =
        notices.iter().find(|notice| notice.origin == server.uri()).expect("GraphQL is paused");
    assert_eq!(notice.api, review_core::http::Api::GraphQl);
    assert!(
        notices
            .iter()
            .filter(|notice| notice.origin == server.uri())
            .all(|notice| notice.api == review_core::http::Api::GraphQl),
        "REST is not paused"
    );

    let again = tokio::time::timeout(patience, provider.pipeline_jobs("group/project", 1, RUN))
        .await
        .expect("a reload does not wait either");
    assert_eq!(again.expect("jobs").len(), 3);
    review_core::http::resume(&server.uri());
}

#[tokio::test]
async fn retries_a_job_and_a_pipeline() {
    let server = MockServer::start().await;
    for route in ["/api/v4/projects/7/jobs/3/retry", "/api/v4/projects/7/pipelines/99/retry"] {
        Mock::given(method("POST"))
            .and(path(route))
            .and(header("PRIVATE-TOKEN", TOKEN))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({ "id": 100, "status": "pending" })),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let provider = provider(&server, Some(TOKEN));
    provider
        .rerun("group/project", 1, RUN, &RerunAction::new(RerunKind::Job, "7:3"))
        .await
        .expect("retry job");
    provider
        .rerun("group/project", 1, RUN, &RerunAction::new(RerunKind::RunFailed, "7:99"))
        .await
        .expect("retry pipeline");
    let stage = RerunAction::new(RerunKind::StageAll, "7:99");
    assert!(matches!(
        provider.rerun("group/project", 1, RUN, &stage).await,
        Err(ProviderError::Unsupported(_))
    ));
}

#[tokio::test]
async fn retries_need_a_token() {
    let server = MockServer::start().await;
    let result = provider(&server, None)
        .rerun("group/project", 1, RUN, &RerunAction::new(RerunKind::Job, "7:3"))
        .await;
    assert!(matches!(result, Err(ProviderError::Unsupported(_))), "{result:?}");
    assert!(server.received_requests().await.expect("recorded").is_empty());
}
