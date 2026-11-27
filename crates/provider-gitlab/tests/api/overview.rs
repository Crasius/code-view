//! The cross-project merge request overview against a mock GitLab REST API v4: one listing per
//! group (`GET /groups/:id/merge_requests`) or per role (`GET /merge_requests?scope=…`), with the
//! parameters documented on <https://docs.gitlab.com/api/merge_requests/>. `.expect(n)` is
//! verified when the server drops.

use provider_gitlab::GitLabProvider;
use review_core::{
    Connection, DiffStat, Provider, ProviderKind, PullRequestQuery, PullRequestScope, StateFilter,
};
use serde_json::{Value, json};
use wiremock::matchers::{
    body_partial_json, body_string_contains, header, method, path, query_param,
    query_param_is_missing,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn provider(server: &MockServer, organizations: &[&str], token: Option<&str>) -> GitLabProvider {
    let connection = Connection {
        id: "overview".to_owned(),
        name: "Mock GitLab".to_owned(),
        kind: ProviderKind::GitLab,
        base_url: server.uri(),
        organizations: organizations.iter().map(|group| (*group).to_owned()).collect(),
        token: token.map(str::to_owned),
    };
    GitLabProvider::new(connection, reqwest::Client::new())
}

fn query(scope: PullRequestScope) -> PullRequestQuery {
    PullRequestQuery { scope, state: StateFilter::Open, repository_key: None, top: 100 }
}

fn merge_request(project: &str, project_id: u64, iid: u64, created_day: u32) -> Value {
    json!({
        "id": project_id * 1000 + iid, "iid": iid, "project_id": project_id, "title": format!("Change {iid}"),
        "description": "", "state": "opened", "author": { "id": 2, "username": "grace", "name": "Grace" },
        "source_branch": format!("feature-{iid}"), "target_branch": "main",
        "created_at": format!("2026-10-{created_day:02}T10:00:00Z"), "updated_at": "2026-10-17T10:00:00Z",
        "web_url": format!("https://gitlab.example/{project}/-/merge_requests/{iid}"),
        "references": { "full": format!("{project}!{iid}") }, "labels": [], "reviewers": [],
    })
}

fn listing(rows: Vec<Value>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(Value::Array(rows))
}

async fn mount_user(server: &MockServer, expected: u64) {
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "id": 1, "username": "ada", "name": "Ada" })),
        )
        .expect(expected)
        .mount(server)
        .await;
}

#[tokio::test]
async fn everything_in_a_group_is_one_listing_even_anonymously() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/groups/acme/merge_requests"))
        .and(query_param("state", "opened"))
        .and(query_param("scope", "all"))
        .and(query_param("with_labels_details", "true"))
        .respond_with(listing(vec![
            merge_request("acme/cli", 7, 3, 2),
            merge_request("acme/app", 9, 8, 5),
        ]))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["acme"], None);

    let rows =
        provider.list_pull_requests(&query(PullRequestScope::Visible)).await.expect("overview");
    let keys: Vec<(&str, u64)> =
        rows.iter().map(|row| (row.repository_key.as_str(), row.number)).collect();
    assert_eq!(keys, [("acme/app", 8), ("acme/cli", 3)], "newest first");
    let requests = server.received_requests().await.expect("recorded");
    assert!(
        requests.iter().all(|request| request.url.path() != "/api/v4/user"),
        "a group listing needs no user"
    );
}

#[tokio::test]
async fn without_groups_everything_is_what_involves_the_user() {
    let server = MockServer::start().await;
    mount_user(&server, 1).await;
    for (scope, rows) in [
        ("created_by_me", vec![merge_request("a/one", 1, 1, 1), merge_request("b/two", 2, 2, 3)]),
        ("assigned_to_me", vec![merge_request("b/two", 2, 2, 3)]),
    ] {
        Mock::given(method("GET"))
            .and(path("/api/v4/merge_requests"))
            .and(query_param("scope", scope))
            .and(query_param("state", "opened"))
            .and(query_param_is_missing("reviewer_id"))
            .respond_with(listing(rows))
            .expect(2)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v4/merge_requests"))
        .and(query_param("scope", "all"))
        .and(query_param("reviewer_id", "1"))
        .respond_with(listing(vec![merge_request("c/three", 3, 9, 2)]))
        .expect(2)
        .mount(&server)
        .await;
    let provider = provider(&server, &[], Some("test-token"));

    let rows =
        provider.list_pull_requests(&query(PullRequestScope::Visible)).await.expect("overview");
    let keys: Vec<(&str, u64)> =
        rows.iter().map(|row| (row.repository_key.as_str(), row.number)).collect();
    assert_eq!(
        keys,
        [("b/two", 2), ("c/three", 9), ("a/one", 1)],
        "merged, each once, newest first"
    );
    // A second load reuses the user read on the first (`/user` expects one request).
    provider.list_pull_requests(&query(PullRequestScope::Visible)).await.expect("second load");
}

#[tokio::test]
async fn assigned_to_me_filters_by_reviewer_id_and_reads_the_user_once() {
    let server = MockServer::start().await;
    mount_user(&server, 1).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/groups/team/merge_requests"))
        .and(query_param("scope", "all"))
        .and(query_param("reviewer_id", "1"))
        .and(query_param("state", "opened"))
        .respond_with(listing(vec![merge_request("team/app", 4, 5, 4)]))
        .expect(3)
        .mount(&server)
        .await;
    let provider = provider(&server, &["team"], Some("test-token"));
    for _ in 0..3 {
        let rows = provider
            .list_pull_requests(&query(PullRequestScope::ReviewRequested))
            .await
            .expect("assigned to me");
        assert_eq!(rows.iter().map(|row| row.number).collect::<Vec<_>>(), [5]);
    }
}

#[tokio::test]
async fn created_by_me_lists_per_group_with_its_scope() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/groups/team/merge_requests"))
        .and(query_param("scope", "created_by_me"))
        .respond_with(listing(vec![merge_request("team/app", 4, 6, 4)]))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["team"], Some("test-token"));
    let rows = provider
        .list_pull_requests(&query(PullRequestScope::CreatedByMe))
        .await
        .expect("created by me");
    assert_eq!(rows.iter().map(|row| row.number).collect::<Vec<_>>(), [6]);
}

/// The user's part of one project (the Favorites card under a role filter): the project's own
/// listing with the role's parameters, not the group's.
#[tokio::test]
async fn the_users_part_of_one_project_is_that_projects_listing() {
    let server = MockServer::start().await;
    mount_user(&server, 1).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/team%2Fapp/merge_requests"))
        .and(query_param("scope", "created_by_me"))
        .and(query_param("state", "opened"))
        .respond_with(listing(vec![merge_request("team/app", 4, 6, 4)]))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/team%2Fapp/merge_requests"))
        .and(query_param("scope", "all"))
        .and(query_param("reviewer_id", "1"))
        .respond_with(listing(vec![merge_request("team/app", 4, 5, 3)]))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["team"], Some("test-token"));
    for (role, number) in
        [(PullRequestScope::CreatedByMe, 6), (PullRequestScope::ReviewRequested, 5)]
    {
        let query =
            PullRequestQuery::in_repository("team/app".to_owned(), role, StateFilter::Open, 100);
        let rows = provider.list_pull_requests(&query).await.expect("the user's part of team/app");
        assert_eq!(rows.iter().map(|row| row.number).collect::<Vec<_>>(), [number], "{role:?}");
    }
}

/// The listing has no line counts; one GraphQL request asks `diffStatsSummary` of every listed
/// merge request of a project, and nothing else.
#[tokio::test]
async fn diff_stats_of_a_project_are_one_graphql_request() {
    let server = MockServer::start().await;
    let nodes = json!([
        { "iid": "5", "diffStatsSummary": { "additions": 1240, "deletions": 312, "fileCount": 18 } },
        { "iid": "6", "diffStatsSummary": null },
        null,
        { "iid": "9", "diffStatsSummary": { "additions": 0, "deletions": 4, "fileCount": 1 } },
    ]);
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(header("authorization", "Bearer test-token"))
        .and(body_partial_json(
            json!({ "variables": { "project": "team/app", "iids": ["5", "6", "9"] } }),
        ))
        .and(body_string_contains("diffStatsSummary { additions deletions fileCount }"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "data": { "project": { "mergeRequests": { "nodes": nodes } } } }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["team"], Some("test-token"));

    let stats = provider.diff_stats("team/app", &[5, 6, 9]).await.expect("diff stats");
    assert_eq!(
        stats,
        [(5, DiffStat::new(1240, 312, 18)), (9, DiffStat::new(0, 4, 1))],
        "a merge request without a diff yet and a null node are left out"
    );
}

/// Listed rows name their head commit (`sha`), which keys the counts kept for them.
#[tokio::test]
async fn listed_rows_name_their_head_commit() {
    let server = MockServer::start().await;
    let mut listed = merge_request("team/app", 4, 5, 4);
    listed["sha"] = json!("abc123");
    Mock::given(method("GET"))
        .and(path("/api/v4/groups/team/merge_requests"))
        .respond_with(listing(vec![listed]))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["team"], None);
    let rows =
        provider.list_pull_requests(&query(PullRequestScope::Visible)).await.expect("overview");
    assert_eq!(rows[0].head_sha.as_deref(), Some("abc123"));
    assert_eq!(rows[0].diff_stat, DiffStat::default(), "REST lists no line counts");
}
