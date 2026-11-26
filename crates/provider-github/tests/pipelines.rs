//! Pipeline reads against a mock GitHub Enterprise host.

use provider_github::GitHubProvider;
use review_core::{Connection, Provider, ProviderError, ProviderKind, RerunAction, RerunKind};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "cli/cli";
const NUMBER: u64 = 7;
const HEAD_SHA: &str = "head111";

async fn setup() -> (MockServer, GitHubProvider) {
    let server = MockServer::start().await;
    let connection = Connection {
        id: "mock".to_owned(),
        name: "Mock GitHub Enterprise".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: Some("test-token".to_owned()),
    };
    let provider = GitHubProvider::new(connection, reqwest::Client::new());
    (server, provider)
}

async fn mount_get(server: &MockServer, repo_path: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}{repo_path}")))
        .respond_with(response)
        .mount(server)
        .await;
}

fn ok_json(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

#[tokio::test]
async fn forbidden_check_runs_still_show_runs_and_statuses() {
    let (server, provider) = setup().await;
    let pull: Value = serde_json::from_str(include_str!("fixtures/pull_request.json"))
        .expect("pull request fixture");
    mount_get(&server, &format!("/pulls/{NUMBER}"), ok_json(pull)).await;
    mount_get(&server, "/actions/runs", ok_json(json!({ "workflow_runs": [] }))).await;
    // Fine-grained personal access tokens get 403 from the checks API.
    let forbidden = ResponseTemplate::new(403)
        .set_body_json(json!({ "message": "Resource not accessible by personal access token" }));
    mount_get(&server, &format!("/commits/{HEAD_SHA}/check-runs"), forbidden).await;
    let status = json!({
        "statuses": [{ "id": 1, "context": "ci/build", "state": "success", "target_url": "https://ci.example/1" }]
    });
    mount_get(&server, &format!("/commits/{HEAD_SHA}/status"), ok_json(status)).await;

    let runs =
        provider.pipeline_runs(REPO, NUMBER).await.expect("pipelines load without check runs");

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].name, "ci/build");
}

fn job(id: u64, name: &str, started: &str, completed: &str) -> Value {
    json!({ "id": id, "name": name, "status": "completed", "conclusion": "success",
            "started_at": started, "completed_at": completed, "steps": [] })
}

async fn mount_jobs(server: &MockServer) {
    let jobs = json!({ "total_count": 2, "jobs": [
        job(2, "build", "2024-05-01T10:00:00Z", "2024-05-01T10:02:00Z"),
        job(1, "lint", "2024-05-01T10:00:00Z", "2024-05-01T10:01:00Z")
    ] });
    // Every attempt's jobs in one listing: earlier attempts cost no extra request.
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}/actions/runs/11/jobs")))
        .and(query_param("filter", "all"))
        .respond_with(ok_json(jobs))
        .mount(server)
        .await;
}

/// Runs - Get gives the workflow path and commit; Repository contents - Get the file (base64).
#[tokio::test]
async fn job_graph_follows_the_workflow_files_needs() {
    let (server, provider) = setup().await;
    mount_jobs(&server).await;
    let run = json!({ "id": 11, "html_url": "https://github.com/cli/cli/actions/runs/11",
                      "path": ".github/workflows/ci.yml", "head_sha": HEAD_SHA });
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}/actions/runs/11")))
        .respond_with(ok_json(run))
        .expect(1)
        .mount(&server)
        .await;
    // `jobs:\n  lint: {}\n  build:\n    needs: lint\n`, wrapped like GitHub wraps base64.
    let content = "am9iczoKICBsaW50OiB7fQogIGJ1aWxk\nOgogICAgbmVlZHM6IGxpbnQK\n";
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}/contents/.github/workflows/ci.yml")))
        .and(query_param("ref", HEAD_SHA))
        .respond_with(ok_json(json!({ "type": "file", "encoding": "base64", "content": content })))
        .expect(1)
        .mount(&server)
        .await;

    let stages = provider.pipeline_jobs(REPO, NUMBER, "run:11").await.expect("jobs");
    let needs: Vec<(&str, Vec<&str>)> = stages
        .iter()
        .map(|stage| (stage.name.as_str(), stage.depends_on.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(needs, vec![("lint", vec![]), ("build", vec!["group:lint"])]);
    // The file is read once per run.
    provider.pipeline_jobs(REPO, NUMBER, "run:11").await.expect("jobs again");
}

/// The run listing names each run's workflow file and commit: a listed run's jobs read the file
/// without `GET /actions/runs/{id}`. The head commit's runs are head runs; a workflow triggered
/// again on it supersedes its earlier run.
#[tokio::test]
async fn listed_runs_read_their_workflow_file_without_another_run_request() {
    let (server, provider) = setup().await;
    let pull: Value = serde_json::from_str(include_str!("fixtures/pull_request.json"))
        .expect("pull request fixture");
    mount_get(&server, &format!("/pulls/{NUMBER}"), ok_json(pull)).await;
    let listed = |id: u64| {
        json!({ "id": id, "workflow_id": 5, "name": "CI", "status": "completed", "conclusion": "success", "event": "pull_request",
                "html_url": format!("https://github.com/cli/cli/actions/runs/{id}"), "path": ".github/workflows/ci.yml", "head_sha": HEAD_SHA })
    };
    mount_get(
        &server,
        "/actions/runs",
        ok_json(json!({ "workflow_runs": [listed(11), listed(9)] })),
    )
    .await;
    mount_get(
        &server,
        &format!("/commits/{HEAD_SHA}/check-runs"),
        ok_json(json!({ "check_runs": [] })),
    )
    .await;
    mount_get(&server, &format!("/commits/{HEAD_SHA}/status"), ok_json(json!({ "statuses": [] })))
        .await;
    mount_jobs(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}/actions/runs/11")))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let content = "am9iczoKICBsaW50OiB7fQogIGJ1aWxk\nOgogICAgbmVlZHM6IGxpbnQK\n";
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/repos/{REPO}/contents/.github/workflows/ci.yml")))
        .and(query_param("ref", HEAD_SHA))
        .respond_with(ok_json(json!({ "type": "file", "encoding": "base64", "content": content })))
        .expect(1)
        .mount(&server)
        .await;

    let runs = provider.pipeline_runs(REPO, NUMBER).await.expect("runs");
    let heads: Vec<(&str, bool)> = runs.iter().map(|run| (run.id.as_str(), run.is_head)).collect();
    assert_eq!(heads, [("run:11", true), ("run:9", false)]);
    let stages = provider.pipeline_jobs(REPO, NUMBER, "run:11").await.expect("jobs");
    assert_eq!(stages[1].depends_on, ["group:lint"], "the graph comes from the file");
}

#[tokio::test]
async fn job_graph_falls_back_to_timing_without_the_workflow_file() {
    let (server, provider) = setup().await;
    mount_jobs(&server).await;
    mount_get(&server, "/actions/runs/11", ResponseTemplate::new(403)).await;
    let stages =
        provider.pipeline_jobs(REPO, NUMBER, "run:11").await.expect("jobs without workflow");
    let needs: Vec<(&str, Vec<&str>)> = stages
        .iter()
        .map(|stage| (stage.name.as_str(), stage.depends_on.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(needs, vec![("build", vec![]), ("lint", vec![])]);
}

/// Re-run endpoints: POST, no body required, 201 Created.
#[tokio::test]
async fn reruns_a_job_failed_jobs_and_a_whole_run() {
    let (server, provider) = setup().await;
    for route in
        ["/actions/jobs/501/rerun", "/actions/runs/11/rerun-failed-jobs", "/actions/runs/11/rerun"]
    {
        Mock::given(method("POST"))
            .and(path(format!("/api/v3/repos/{REPO}{route}")))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
    }
    provider
        .rerun(REPO, NUMBER, "run:11", &RerunAction::new(RerunKind::Job, "501"))
        .await
        .expect("job");
    provider
        .rerun(REPO, NUMBER, "run:11", &RerunAction::new(RerunKind::RunFailed, "11"))
        .await
        .expect("failed jobs");
    provider
        .rerun(REPO, NUMBER, "run:11", &RerunAction::new(RerunKind::RunAll, "11"))
        .await
        .expect("all jobs");
    let stage = RerunAction::new(RerunKind::StageAll, "11");
    assert!(matches!(
        provider.rerun(REPO, NUMBER, "run:11", &stage).await,
        Err(ProviderError::Unsupported(_))
    ));
}

#[tokio::test]
async fn reruns_need_a_token() {
    let server = MockServer::start().await;
    let connection = Connection {
        id: "mock".to_owned(),
        name: "anonymous".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: None,
    };
    let provider = GitHubProvider::new(connection, reqwest::Client::new());
    let result =
        provider.rerun(REPO, NUMBER, "run:11", &RerunAction::new(RerunKind::Job, "501")).await;
    assert!(matches!(result, Err(ProviderError::Unsupported(_))), "{result:?}");
    assert!(server.received_requests().await.unwrap_or_default().is_empty());
}
