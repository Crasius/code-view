//! Linked item states against a mock GitHub Enterprise (`{base}/api/v3`, `{base}/api/graphql`):
//! one GraphQL request for every reference with a token, `GET /repos/{o}/{r}/issues/{n}` without.

use provider_github::GitHubProvider;
use review_core::{Connection, ItemKind, ItemRef, LinkedState, Provider, ProviderKind, RunStatus};
use serde_json::json;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn provider(server: &MockServer, token: Option<&str>) -> GitHubProvider {
    let connection = Connection {
        id: "github-linked".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: token.map(str::to_owned),
    };
    GitHubProvider::new(connection, reqwest::Client::new())
}

fn reference(kind: ItemKind, repository: &str, number: u64) -> ItemRef {
    ItemRef { kind, repository_key: repository.to_owned(), number }
}

#[tokio::test]
async fn one_graphql_request_resolves_every_reference() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(header("authorization", "Bearer test-token"))
        .and(body_string_contains("issueOrPullRequest(number: 59)"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "r0": {
                    "i0": { "__typename": "PullRequest", "number": 59, "title": "Speed up login", "url": "https://ghe/acme/app/pull/59",
                            "state": "MERGED", "isDraft": false, "repository": { "nameWithOwner": "acme/app" },
                            "statusCheckRollup": { "state": "SUCCESS" } },
                    "i1": { "__typename": "Issue", "number": 60, "title": "Login is slow", "url": "https://ghe/acme/app/issues/60",
                            "state": "CLOSED", "stateReason": "COMPLETED", "repository": { "nameWithOwner": "acme/app" } }
                },
                "r1": null
            },
            "errors": [{ "type": "NOT_FOUND", "message": "Could not resolve to a Repository with the name 'gone/repo'." }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let asked = [
        reference(ItemKind::IssueOrPullRequest, "acme/app", 59),
        reference(ItemKind::IssueOrPullRequest, "acme/app", 60),
        reference(ItemKind::PullRequest, "gone/repo", 1),
    ];
    let items =
        provider(&server, Some("test-token")).linked_items(&asked).await.expect("linked items");
    assert_eq!(items.len(), 2, "the missing repository is left out: {items:?}");
    assert_eq!(items[0].reference, asked[0]);
    assert_eq!(
        (items[0].kind, items[0].state, items[0].checks),
        (ItemKind::PullRequest, LinkedState::Merged, Some(RunStatus::Succeeded))
    );
    assert_eq!(
        (items[1].kind, items[1].state, items[1].title.as_str()),
        (ItemKind::Issue, LinkedState::Done, "Login is slow")
    );
}

#[tokio::test]
async fn anonymous_reads_issues_one_by_one_and_skips_unknown_numbers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/repos/acme/app/issues/3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 3, "title": "Draft work", "state": "open", "draft": true,
            "html_url": "https://ghe/acme/app/pull/3", "pull_request": { "merged_at": null }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/repos/acme/app/issues/4"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })))
        .expect(1)
        .mount(&server)
        .await;

    let asked = [
        reference(ItemKind::IssueOrPullRequest, "acme/app", 3),
        reference(ItemKind::Issue, "acme/app", 4),
    ];
    let items = provider(&server, None).linked_items(&asked).await.expect("linked items");
    assert_eq!(items.len(), 1);
    assert_eq!(
        (items[0].kind, items[0].state, items[0].state_label.as_str()),
        (ItemKind::PullRequest, LinkedState::Draft, "Draft")
    );
    let requests = server.received_requests().await.expect("recorded");
    assert!(
        requests.iter().all(|request| !request.headers.contains_key("authorization")),
        "anonymous requests carry no token"
    );
}

#[tokio::test]
async fn work_items_and_empty_lists_send_nothing() {
    let server = MockServer::start().await;
    let provider = provider(&server, Some("test-token"));
    let none = provider.linked_items(&[]).await.expect("empty");
    assert!(none.is_empty(), "{none:?}");
    let work_item = reference(ItemKind::WorkItem, "org/project", 5);
    let ignored = provider.linked_items(&[work_item]).await.expect("ignored");
    assert!(ignored.is_empty(), "{ignored:?}");
    let requests = server.received_requests().await.expect("recorded");
    assert!(requests.is_empty(), "{requests:?}");
}
