//! The cross-repository pull request overview against a mock GitHub Enterprise host: GraphQL
//! issue searches (`search(type: ISSUE)`, documented search qualifiers) over the connection's
//! owners, never a walk over repositories. `.expect(n)` is verified when the server drops.

use provider_github::GitHubProvider;
use review_core::{
    CheckState, Connection, DiffStat, Provider, ProviderKind, PullRequestQuery, PullRequestScope,
    RunStatus, StateFilter,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

fn provider(server: &MockServer, organizations: &[&str], token: Option<&str>) -> GitHubProvider {
    let connection = Connection {
        id: "overview".to_owned(),
        name: "Mock GitHub Enterprise".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: server.uri(),
        organizations: organizations.iter().map(|owner| (*owner).to_owned()).collect(),
        token: token.map(str::to_owned),
    };
    GitHubProvider::new(connection, reqwest::Client::new())
}

fn query(scope: PullRequestScope) -> PullRequestQuery {
    PullRequestQuery { scope, state: StateFilter::Open, repository_key: None, top: 100 }
}

/// What `gh auth token` hands out: an OAuth app token.
const CLI_TOKEN: &str = "gho_cli-token";

fn graphql(search: &str) -> MockBuilder {
    graphql_as("test-token", search)
}

fn graphql_as(token: &str, search: &str) -> MockBuilder {
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(header("authorization", format!("Bearer {token}").as_str()))
        .and(body_string_contains("search(type: ISSUE"))
        .and(body_string_contains(search))
}

fn node(repository: &str, number: u64, created: &str, rollup: Option<&str>) -> Value {
    let name = repository.rsplit('/').next().unwrap_or(repository);
    json!({
        "number": number, "title": format!("Change {number}"), "url": format!("https://ghe.example/{repository}/pull/{number}"),
        "state": "OPEN", "isDraft": false, "createdAt": created, "updatedAt": created,
        "headRefName": format!("feature/{number}"), "baseRefName": "main",
        "author": { "login": "mona", "databaseId": 1 },
        "repository": { "nameWithOwner": repository, "name": name },
        "labels": { "nodes": [] }, "reviewRequests": { "nodes": [] }, "latestOpinionatedReviews": { "nodes": [] },
        "commits": { "nodes": [{ "commit": { "statusCheckRollup": rollup.map(|state| json!({ "state": state })) } }] },
    })
}

fn draft(mut node: Value, author: &str) -> Value {
    node["isDraft"] = json!(true);
    node["author"] = json!({ "login": author, "databaseId": 2 });
    node
}

fn results(nodes: &[Value]) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "data": { "search": { "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": nodes } }
    }))
}

/// The signed-in user `mona` and her organizations, each asked for once.
async fn mount_reach(server: &MockServer, organizations: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/api/v3/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 1, "login": "mona" })))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/user/orgs"))
        .and(query_param("per_page", "100"))
        .respond_with(organizations)
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_organization(server: &MockServer, owner: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/users/{owner}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "login": owner, "type": "Organization" })),
        )
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn everything_in_the_organizations_is_one_search() {
    let server = MockServer::start().await;
    mount_organization(&server, "contoso").await;
    graphql(r#""q":"is:pr is:open org:contoso sort:created-desc""#)
        .respond_with(results(&[
            node("contoso/app", 12, "2026-10-02T10:00:00Z", Some("SUCCESS")),
            node("contoso/tools", 4, "2026-10-01T10:00:00Z", None),
        ]))
        .expect(2)
        .mount(&server)
        .await;
    let provider = provider(&server, &["contoso"], Some("test-token"));

    let rows =
        provider.list_pull_requests(&query(PullRequestScope::Visible)).await.expect("overview");
    assert_eq!(
        rows.iter().map(|row| (row.repository_key.as_str(), row.number)).collect::<Vec<_>>(),
        [("contoso/app", 12), ("contoso/tools", 4)]
    );
    assert_eq!(rows[0].checks, Some(RunStatus::Succeeded));
    assert_eq!(rows[1].checks, None, "no checks on the head commit");

    // A second load searches again; the owner kind (`expect(1)` above) was looked up once.
    provider.list_pull_requests(&query(PullRequestScope::Visible)).await.expect("second load");
}

/// Line and file counts are fields of the search's pull requests: the rows have them without a
/// request of their own.
#[tokio::test]
async fn the_search_brings_line_and_file_counts() {
    let server = MockServer::start().await;
    mount_organization(&server, "contoso").await;
    let mut counted = node("contoso/app", 12, "2026-10-02T10:00:00Z", None);
    counted["additions"] = json!(1240);
    counted["deletions"] = json!(312);
    counted["changedFiles"] = json!(18);
    graphql(r#""q":"is:pr is:open org:contoso sort:created-desc""#)
        .and(body_string_contains("additions deletions changedFiles"))
        .respond_with(results(&[counted]))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["contoso"], Some("test-token"));

    let rows =
        provider.list_pull_requests(&query(PullRequestScope::Visible)).await.expect("overview");
    assert_eq!(rows[0].diff_stat, DiffStat::new(1240, 312, 18));
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.iter().filter(|request| request.url.path() == "/api/graphql").count(),
        1,
        "one search, nothing per row"
    );
}

/// A "Use my GitHub CLI sign-in" connection that shows everything the token sees: the owners are
/// the user and both organizations, looked up once, and colleagues' drafts are part of the list.
#[tokio::test]
async fn everything_lists_colleagues_drafts_in_the_users_organizations() {
    let server = MockServer::start().await;
    mount_reach(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!([{ "login": "acme" }, { "login": "tools" }])),
    )
    .await;
    graphql_as(CLI_TOKEN, r#""q":"is:pr is:open user:mona org:acme org:tools sort:created-desc""#)
        .respond_with(results(&[
            draft(node("acme/api", 31, "2026-10-04T10:00:00Z", None), "colleague"),
            draft(node("tools/ci", 7, "2026-10-03T10:00:00Z", None), "another-colleague"),
            node("acme/web", 12, "2026-10-02T10:00:00Z", Some("SUCCESS")),
            draft(node("mona/dotfiles", 2, "2026-09-30T10:00:00Z", None), "mona"),
        ]))
        .expect(2)
        .mount(&server)
        .await;
    let outside = "-user:mona -org:acme -org:tools sort:created-desc";
    graphql_as(CLI_TOKEN, &format!(r#""q":"is:pr involves:@me is:open {outside}""#))
        .respond_with(results(&[node("contoso/app", 99, "2026-10-01T10:00:00Z", Some("PENDING"))]))
        .expect(2)
        .mount(&server)
        .await;
    graphql_as(CLI_TOKEN, &format!(r#""q":"is:pr review-requested:@me is:open {outside}""#))
        .respond_with(results(&[node("contoso/app", 99, "2026-10-01T10:00:00Z", Some("PENDING"))]))
        .expect(2)
        .mount(&server)
        .await;
    let provider = provider(&server, &[], Some(CLI_TOKEN));

    let rows =
        provider.list_pull_requests(&query(PullRequestScope::Visible)).await.expect("overview");
    let keys: Vec<(&str, u64, bool)> =
        rows.iter().map(|row| (row.repository_key.as_str(), row.number, row.is_draft)).collect();
    assert_eq!(
        keys,
        [
            ("acme/api", 31, true),
            ("tools/ci", 7, true),
            ("acme/web", 12, false),
            ("contoso/app", 99, false),
            ("mona/dotfiles", 2, true)
        ],
        "everyone's pull requests in the owners' repositories plus the user's elsewhere, each once, newest first"
    );
    assert_eq!(rows[0].author.login.as_deref(), Some("colleague"));
    assert_eq!(rows[3].checks, Some(RunStatus::Running));
    assert!(provider.partial_access().is_none(), "nothing was left out");

    // The user and organization lookup (`expect(1)` in `mount_reach`) is cached for the next load.
    provider.list_pull_requests(&query(PullRequestScope::Visible)).await.expect("second load");
}

/// An organization behind SAML single sign-on the CLI token is not authorized for: GitHub leaves
/// it out of `/user/orgs` and the searches and says so in `X-GitHub-SSO`. The listing still works
/// and reports the organization with the page that authorizes it.
#[tokio::test]
async fn organizations_behind_sso_surface_an_authorize_action() {
    let server = MockServer::start().await;
    let partial = "partial-results; organizations=4242";
    mount_reach(
        &server,
        ResponseTemplate::new(200)
            .insert_header("x-github-sso", partial)
            .set_body_json(json!([{ "login": "acme" }])),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/organizations/4242"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "login": "secure-corp", "id": 4242 })),
        )
        .expect(1)
        .mount(&server)
        .await;
    graphql_as(CLI_TOKEN, r#""q":"is:pr is:open user:mona org:acme sort:created-desc""#)
        .respond_with(ResponseTemplate::new(200).insert_header("x-github-sso", partial).set_body_json(json!({
            "data": { "search": { "pageInfo": { "hasNextPage": false, "endCursor": null },
                "nodes": [draft(node("acme/api", 31, "2026-10-04T10:00:00Z", None), "colleague"), null] } },
            "errors": [{ "type": "FORBIDDEN", "path": ["search", "nodes", 1],
                "message": "Resource protected by organization SAML enforcement. You must grant your OAuth token access to this organization." }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    graphql_as(CLI_TOKEN, "@me").respond_with(results(&[])).expect(2).mount(&server).await;
    let provider = provider(&server, &[], Some(CLI_TOKEN));

    let rows = provider
        .list_pull_requests(&query(PullRequestScope::Visible))
        .await
        .expect("the visible part still lists");
    assert_eq!(rows.iter().map(|row| row.number).collect::<Vec<_>>(), [31]);
    let problem = provider.partial_access().expect("the left-out organization is reported");
    assert!(problem.title.contains("secure-corp"), "{}", problem.title);
    assert!(problem.fix.contains("gh auth refresh"), "{}", problem.fix);
    assert_eq!(problem.actions.len(), 1, "named once, from the header of both answers");
    assert_eq!(problem.actions[0].label, "Authorize for secure-corp");
    assert_eq!(problem.actions[0].url, format!("{}/orgs/secure-corp/sso", server.uri()));
}

#[tokio::test]
async fn created_by_me_and_assigned_to_me_use_their_qualifiers() {
    let server = MockServer::start().await;
    mount_organization(&server, "contoso").await;
    graphql(r#""q":"is:pr author:@me is:open org:contoso sort:created-desc""#)
        .respond_with(results(&[node("contoso/app", 5, "2026-10-01T10:00:00Z", None)]))
        .expect(1)
        .mount(&server)
        .await;
    graphql(r#""q":"is:pr review-requested:@me is:open org:contoso sort:created-desc""#)
        .respond_with(results(&[node("contoso/app", 6, "2026-10-01T10:00:00Z", None)]))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["contoso"], Some("test-token"));

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

/// The user's part of one repository (the Favorites card under a role filter): one search named
/// by that repository, no owner lookup.
#[tokio::test]
async fn the_users_part_of_one_repository_is_one_search_for_it() {
    let server = MockServer::start().await;
    graphql(r#""q":"is:pr author:@me is:open repo:contoso/app sort:updated-desc""#)
        .respond_with(results(&[node("contoso/app", 5, "2026-10-01T10:00:00Z", None)]))
        .expect(1)
        .mount(&server)
        .await;
    graphql(r#""q":"is:pr review-requested:@me is:open repo:contoso/app sort:updated-desc""#)
        .respond_with(results(&[node("contoso/app", 6, "2026-10-01T10:00:00Z", None)]))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["contoso"], Some("test-token"));

    for (role, number) in
        [(PullRequestScope::CreatedByMe, 5), (PullRequestScope::ReviewRequested, 6)]
    {
        let query =
            PullRequestQuery::in_repository("contoso/app".to_owned(), role, StateFilter::Open, 100);
        let rows =
            provider.list_pull_requests(&query).await.expect("the user's part of contoso/app");
        assert_eq!(rows.iter().map(|row| row.number).collect::<Vec<_>>(), [number], "{role:?}");
    }
    let requests = server.received_requests().await.unwrap_or_default();
    assert!(requests.iter().all(|request| request.url.path() == "/api/graphql"), "no owner lookup");
}

/// A search page that runs out of time behind GitHub's gateway (502 with nginx's HTML page, seen
/// with a fine-grained token) is asked again at half its size; one that still fails at the
/// smallest size shows the page's title, not its markup.
#[tokio::test]
async fn a_timed_out_search_page_is_asked_again_at_half_its_size() {
    let server = MockServer::start().await;
    let bad_gateway = || {
        ResponseTemplate::new(502).set_body_string(
            "<html>\r\n<head><title>502 Bad Gateway</title></head>\r\n<body>nginx</body>\r\n</html>",
        )
    };
    let search = r#""q":"is:pr author:@me is:open repo:contoso/app sort:updated-desc""#;
    graphql(search)
        .and(body_string_contains(r#""first":100"#))
        .respond_with(bad_gateway())
        .expect(1)
        .mount(&server)
        .await;
    graphql(search)
        .and(body_string_contains(r#""first":50"#))
        .respond_with(results(&[node("contoso/app", 5, "2026-10-01T10:00:00Z", None)]))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["contoso"], Some("test-token"));
    let query = PullRequestQuery::in_repository(
        "contoso/app".to_owned(),
        PullRequestScope::CreatedByMe,
        StateFilter::Open,
        100,
    );

    let rows = provider.list_pull_requests(&query).await.expect("the smaller page passed");
    assert_eq!(rows.iter().map(|row| row.number).collect::<Vec<_>>(), [5]);

    server.reset().await;
    graphql(search).respond_with(bad_gateway()).expect(4).mount(&server).await;
    let error = provider.list_pull_requests(&query).await.expect_err("every size timed out");
    assert_eq!(error.status(), Some(502));
    assert_eq!(error.to_string(), "502 Bad Gateway (502 from /api/graphql)");
}

/// A row a search listed and the repository's REST listing lists again: its checks come from a
/// fresh request, not from what the earlier search saw.
#[tokio::test]
async fn a_rest_listing_asks_for_checks_an_earlier_search_listed() {
    let server = MockServer::start().await;
    mount_organization(&server, "contoso").await;
    graphql(r#""q":"is:pr is:open org:contoso sort:created-desc""#)
        .respond_with(results(&[node(
            "contoso/app",
            1234,
            "2024-05-01T10:00:00Z",
            Some("PENDING"),
        )]))
        .expect(1)
        .mount(&server)
        .await;
    let rest_row: Value =
        serde_json::from_str(include_str!("fixtures/pull_request.json")).expect("fixture");
    Mock::given(method("GET"))
        .and(path("/api/v3/repos/contoso/app/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([rest_row])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains("pr1234: pullRequest(number: 1234)"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "repository": {
            "pr1234": { "number": 1234, "reviewDecision": null,
                "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "SUCCESS" } } }] } }
        } } })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["contoso"], Some("test-token"));

    provider.list_pull_requests(&query(PullRequestScope::Visible)).await.expect("search");
    let repository = PullRequestQuery::in_repository(
        "contoso/app".to_owned(),
        PullRequestScope::Repository,
        StateFilter::Open,
        100,
    );
    let rows = provider.list_pull_requests(&repository).await.expect("REST listing");
    assert_eq!(rows.iter().map(|row| row.number).collect::<Vec<_>>(), [1234]);
    let statuses = provider.row_statuses("contoso/app", &[1234]).await.expect("checks");
    assert_eq!(
        statuses.iter().map(|(number, status)| (*number, status.reported)).collect::<Vec<_>>(),
        [(1234, Some(CheckState::Passed))],
        "the checks as they are now, not as the search saw them"
    );
}

#[tokio::test]
async fn anonymous_overview_asks_for_a_token_without_a_request() {
    let server = MockServer::start().await;
    let provider = provider(&server, &["contoso"], None);
    let error = provider
        .list_pull_requests(&query(PullRequestScope::Visible))
        .await
        .expect_err("GraphQL needs a token");
    assert!(error.to_string().contains("token"), "{error}");
    assert!(server.received_requests().await.unwrap_or_default().is_empty());
}

/// Several repositories share one search, so one busy repository can fill the page: each keeps at
/// most `top` rows, and the repositories that came back short while the search had more are
/// searched again on their own.
#[tokio::test]
async fn a_busy_repository_leaves_the_others_their_share() {
    let server = MockServer::start().await;
    let busy: Vec<Value> = (1..=4)
        .map(|number| {
            node("acme/busy", number, &format!("2026-10-0{}T10:00:00Z", 5 - number), None)
        })
        .collect();
    graphql(r#""q":"is:pr is:open repo:acme/busy repo:acme/quiet sort:updated-desc""#)
        .and(body_string_contains(r#""first":4"#))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "search": {
            "pageInfo": { "hasNextPage": true, "endCursor": "c4" }, "nodes": busy
        } } })))
        .expect(1)
        .mount(&server)
        .await;
    graphql(r#""q":"is:pr is:open repo:acme/quiet sort:updated-desc""#)
        .and(body_string_contains(r#""first":2"#))
        .respond_with(results(&[node("acme/quiet", 9, "2026-09-01T10:00:00Z", None)]))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["acme"], Some("test-token"));
    let repositories = ["acme/busy".to_owned(), "acme/quiet".to_owned()];

    let rows = provider
        .list_repositories_pull_requests(
            &repositories,
            PullRequestScope::Visible,
            StateFilter::Open,
            2,
        )
        .await
        .expect("both repositories");
    let listed: Vec<(&str, u64)> =
        rows.iter().map(|row| (row.repository_key.as_str(), row.number)).collect();
    assert_eq!(listed, [("acme/busy", 1), ("acme/busy", 2), ("acme/quiet", 9)]);
}

/// A pull request the batched status query names but GitHub does not know comes back as `null`
/// with a `NOT_FOUND` error next to the data: it is left out, the others still report. Any other
/// error still fails the batch.
#[tokio::test]
async fn a_missing_pull_request_in_the_status_batch_is_left_out() {
    let server = MockServer::start().await;
    let status_query = || {
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(body_string_contains("pr7: pullRequest(number: 7)"))
            .and(body_string_contains("pr8: pullRequest(number: 8)"))
    };
    let data = json!({ "repository": {
        "pr7": { "number": 7, "reviewDecision": null,
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "SUCCESS" } } }] } },
        "pr8": null
    } });
    status_query()
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": data, "errors": [{
            "type": "NOT_FOUND", "path": ["repository", "pr8"],
            "message": "Could not resolve to a PullRequest with the number of 8."
        }] })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &["contoso"], Some("test-token"));

    let statuses = provider.row_statuses("contoso/app", &[7, 8]).await.expect("the found one");
    assert_eq!(
        statuses.iter().map(|(number, status)| (*number, status.reported)).collect::<Vec<_>>(),
        [(7, Some(CheckState::Passed))]
    );

    server.reset().await;
    status_query()
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": data, "errors": [{
            "type": "FORBIDDEN", "message": "Resource not accessible by integration"
        }] })))
        .expect(1)
        .mount(&server)
        .await;
    let error = provider.row_statuses("contoso/app", &[7, 8]).await.expect_err("not a missing row");
    assert_eq!(error.status(), Some(403));
}
