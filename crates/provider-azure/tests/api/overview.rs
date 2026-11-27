//! The cross-repository pull request overview against a mocked Azure DevOps REST API (7.1): one
//! `git/pullrequests` listing per organization with the documented `searchCriteria.*` parameters
//! (learn.microsoft.com/rest/api/azure/devops/git/pull-requests/get-pull-requests-by-project),
//! never one per project or repository. `.expect(n)` is verified when the server drops.

use review_core::{Provider, PullRequestQuery, PullRequestScope, StateFilter};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

use crate::support::{AUTHORIZATION, TOKEN, provider};

const ME: &str = "me-id";

fn query(scope: PullRequestScope) -> PullRequestQuery {
    PullRequestQuery { scope, state: StateFilter::Open, repository_key: None, top: 100 }
}

fn listing(organization: &str) -> MockBuilder {
    Mock::given(method("GET"))
        .and(path(format!("/{organization}/_apis/git/pullrequests")))
        .and(query_param("api-version", "7.1"))
        .and(query_param("searchCriteria.status", "active"))
        .and(header("authorization", AUTHORIZATION))
}

fn pull_request(project: &str, repository: &str, id: u64, created: &str) -> Value {
    json!({
        "pullRequestId": id, "status": "active", "title": format!("Change {id}"),
        "repository": { "name": repository, "project": { "name": project } },
        "createdBy": { "id": "author", "displayName": "Grace" }, "creationDate": created,
        "sourceRefName": format!("refs/heads/feature/{id}"), "targetRefName": "refs/heads/main",
        "reviewers": [{ "id": "r1", "displayName": "Linus", "vote": 10 }],
    })
}

fn page(rows: &[Value]) -> ResponseTemplate {
    let count = rows.len();
    ResponseTemplate::new(200).set_body_json(json!({ "value": rows, "count": count }))
}

#[tokio::test]
async fn everything_is_one_listing_per_organization() {
    let server = MockServer::start().await;
    listing("north")
        .and(query_param_is_missing("searchCriteria.creatorId"))
        .and(query_param_is_missing("searchCriteria.reviewerId"))
        .respond_with(page(&[pull_request("Web", "site", 11, "2026-10-02T10:00:00Z")]))
        .expect(1)
        .mount(&server)
        .await;
    listing("south")
        .and(query_param_is_missing("searchCriteria.creatorId"))
        .respond_with(page(&[
            pull_request("Api", "service", 12, "2026-10-03T10:00:00Z"),
            pull_request("Api", "worker", 4, "2026-09-30T10:00:00Z"),
        ]))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["north", "south"], Some(TOKEN));

    let rows =
        provider.list_pull_requests(&query(PullRequestScope::Visible)).await.expect("overview");
    let keys: Vec<(&str, u64)> =
        rows.iter().map(|row| (row.repository_key.as_str(), row.number)).collect();
    assert_eq!(
        keys,
        [("south/Api/service", 12), ("north/Web/site", 11), ("south/Api/worker", 4)],
        "newest first across organizations"
    );
    assert_eq!(rows[0].reviewers.len(), 1, "votes come with the listing");
    let requests = server.received_requests().await.unwrap_or_default();
    assert!(
        requests.iter().all(|request| !request.url.path().ends_with("connectionData")),
        "no user lookup for everything"
    );
}

#[tokio::test]
async fn created_by_me_and_assigned_to_me_filter_by_the_cached_user() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/north/_apis/connectionData"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "authenticatedUser": { "id": ME, "providerDisplayName": "Me" } }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    listing("north")
        .and(query_param("searchCriteria.creatorId", ME))
        .respond_with(page(&[pull_request("Web", "site", 5, "2026-10-02T10:00:00Z")]))
        .expect(1)
        .mount(&server)
        .await;
    listing("north")
        .and(query_param("searchCriteria.reviewerId", ME))
        .respond_with(page(&[pull_request("Web", "site", 6, "2026-10-02T10:00:00Z")]))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["north"], Some(TOKEN));

    let mine = provider
        .list_pull_requests(&query(PullRequestScope::CreatedByMe))
        .await
        .expect("created by me");
    assert_eq!(mine.iter().map(|row| row.number).collect::<Vec<_>>(), [5]);
    let assigned = provider
        .list_pull_requests(&query(PullRequestScope::ReviewRequested))
        .await
        .expect("assigned to me");
    assert_eq!(assigned.iter().map(|row| row.number).collect::<Vec<_>>(), [6]);
}

/// The user's part of one repository (the Favorites card under a role filter): the repository's
/// own listing with the same `searchCriteria`, not the organization's.
#[tokio::test]
async fn the_users_part_of_one_repository_is_that_repositorys_listing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/north/_apis/connectionData"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "authenticatedUser": { "id": ME, "providerDisplayName": "Me" } }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let repository = || {
        Mock::given(method("GET"))
            .and(path("/north/Web/_apis/git/repositories/site/pullrequests"))
            .and(query_param("searchCriteria.status", "active"))
            .and(header("authorization", AUTHORIZATION))
    };
    repository()
        .and(query_param("searchCriteria.creatorId", ME))
        .respond_with(page(&[pull_request("Web", "site", 5, "2026-10-02T10:00:00Z")]))
        .expect(1)
        .mount(&server)
        .await;
    repository()
        .and(query_param("searchCriteria.reviewerId", ME))
        .respond_with(page(&[pull_request("Web", "site", 6, "2026-10-02T10:00:00Z")]))
        .expect(1)
        .mount(&server)
        .await;
    listing("north").respond_with(page(&[])).expect(0).mount(&server).await;
    let provider = provider(&server, &["north"], Some(TOKEN));

    for (role, number) in
        [(PullRequestScope::CreatedByMe, 5), (PullRequestScope::ReviewRequested, 6)]
    {
        let query = PullRequestQuery::in_repository(
            "north/Web/site".to_owned(),
            role,
            StateFilter::Open,
            100,
        );
        let rows =
            provider.list_pull_requests(&query).await.expect("the user's part of the repository");
        assert_eq!(rows.iter().map(|row| row.number).collect::<Vec<_>>(), [number], "{role:?}");
    }
}

/// A row's changed files: the iterations (without their commits), then the latest iteration's
/// changes against the common commit, folders left out. Two requests, no line counts.
#[tokio::test]
async fn diff_stats_count_the_latest_iterations_changed_files() {
    let server = MockServer::start().await;
    let pull_request = "/north/Web/_apis/git/repositories/site/pullrequests/11";
    Mock::given(method("GET"))
        .and(path(format!("{pull_request}/iterations")))
        .and(query_param("includeCommits", "false"))
        .and(header("authorization", AUTHORIZATION))
        .respond_with(page(&[json!({ "id": 1 }), json!({ "id": 3 }), json!({ "id": 2 })]))
        .expect(1)
        .mount(&server)
        .await;
    let entries: Vec<Value> = (1..=4)
        .map(|index| json!({ "changeTrackingId": index, "changeId": index, "item": { "path": format!("/src/f{index}.rs") }, "changeType": "edit" }))
        .chain(std::iter::once(json!({ "changeTrackingId": 9, "changeId": 9, "item": { "path": "/src", "isFolder": true }, "changeType": "add" })))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{pull_request}/iterations/3/changes")))
        .and(query_param("$top", "2000"))
        .and(query_param("$skip", "0"))
        .and(query_param_is_missing("$compareTo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "changeEntries": entries })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["north"], Some(TOKEN));

    let stats = provider.diff_stats("north/Web/site", &[11]).await.expect("diff stats");
    assert_eq!(stats, [(11, review_core::DiffStat { lines: None, files: Some(4) })]);
}

/// The policy evaluations name a pull request by its project's GUID: one repository lookup per
/// project finds it and is kept for later row requests.
#[tokio::test]
async fn row_statuses_look_up_the_project_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/north/Web/_apis/git/repositories/site"))
        .and(header("authorization", AUTHORIZATION))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "id": "repo-guid", "name": "site", "project": { "id": "web-guid", "name": "Web" } }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    for number in [11, 12, 13] {
        Mock::given(method("GET"))
            .and(path("/north/Web/_apis/policy/evaluations"))
            .and(query_param(
                "artifactId",
                format!("vstfs:///CodeReview/CodeReviewId/web-guid/{number}"),
            ))
            .respond_with(page(&[]))
            .expect(1)
            .mount(&server)
            .await;
    }
    let provider = provider(&server, &["north"], Some(TOKEN));

    let first = provider.row_statuses("north/Web/site", &[11]).await;
    let second = provider.row_statuses("north/Web/site", &[12]).await;
    let later = provider.row_statuses("north/Web/site", &[13]).await;

    for (statuses, number) in [(first, 11), (second, 12), (later, 13)] {
        let numbers: Vec<u64> =
            statuses.expect("row statuses").iter().map(|(number, _)| *number).collect();
        assert_eq!(numbers, [number]);
    }
}
