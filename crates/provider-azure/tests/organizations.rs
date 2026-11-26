//! A connection that names several organizations lists their pull requests at the same time:
//! one organization-wide listing each, against a mock REST API. The user's own listings also read
//! the current user of every organization at the same time, and once per organization.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use provider_azure::AzureDevOpsProvider;
use review_core::{
    Connection, Provider, ProviderKind, PullRequestQuery, PullRequestScope, StateFilter, http,
};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Each listing answers this long after it arrived; listings one after another could not arrive
/// closer together than that.
const ANSWER_DELAY: Duration = Duration::from_secs(2);

type Arrivals = Arc<Mutex<Vec<(String, Instant)>>>;

/// Notes that a request for the organization in `request`'s first path segment arrived now.
fn note_arrival(arrived: &Arrivals, request: &Request) -> String {
    let organization =
        request.url.path().trim_start_matches('/').split('/').next().unwrap_or_default();
    arrived
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((organization.to_owned(), Instant::now()));
    organization.to_owned()
}

/// Answers an organization's listing with one pull request after [`ANSWER_DELAY`], and notes when
/// each listing arrived.
struct Listings {
    arrived: Arrivals,
}

impl Respond for Listings {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let organization = note_arrival(&self.arrived, request);
        let id = if organization == "north" { 1 } else { 2 };
        let body = json!({ "count": 1, "value": [pull_request(id)] });
        ResponseTemplate::new(200).set_body_json(body).set_delay(ANSWER_DELAY)
    }
}

/// Answers an organization's `connectionData` with its user after [`ANSWER_DELAY`], and notes
/// when each request arrived.
struct ConnectionDatas {
    arrived: Arrivals,
}

impl Respond for ConnectionDatas {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let organization = note_arrival(&self.arrived, request);
        let body = json!({ "authenticatedUser": {
            "id": format!("me-{organization}"), "providerDisplayName": "Grace",
        }});
        ResponseTemplate::new(200).set_body_json(body).set_delay(ANSWER_DELAY)
    }
}

fn pull_request(id: u64) -> Value {
    json!({
        "pullRequestId": id, "status": "active", "title": format!("Change {id}"),
        "repository": { "name": "web", "project": { "name": "Shop" } },
        "createdBy": { "id": "author", "displayName": "Grace" },
        "creationDate": format!("2026-01-0{id}T10:00:00Z"),
        "sourceRefName": "refs/heads/feature", "targetRefName": "refs/heads/main", "reviewers": [],
    })
}

fn provider(server: &MockServer) -> AzureDevOpsProvider {
    let connection = Connection {
        id: "azure-organizations".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::AzureDevOps,
        base_url: server.uri(),
        organizations: vec!["north".to_owned(), "south".to_owned()],
        token: Some("dummy-pat".to_owned()),
    };
    AzureDevOpsProvider::new(connection, http::build_client())
}

fn query(scope: PullRequestScope) -> PullRequestQuery {
    PullRequestQuery { scope, state: StateFilter::Open, repository_key: None, top: 50 }
}

fn arrivals(arrived: &Arrivals) -> Vec<(String, Instant)> {
    arrived.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

#[tokio::test]
async fn organizations_are_listed_at_the_same_time() {
    let server = MockServer::start().await;
    let arrived = Arrivals::default();
    Mock::given(wiremock::matchers::path_regex(r"^/(north|south)/_apis/git/pullrequests$"))
        .respond_with(Listings { arrived: Arc::clone(&arrived) })
        .mount(&server)
        .await;
    let provider = provider(&server);

    let listed = provider
        .list_pull_requests(&query(PullRequestScope::Visible))
        .await
        .expect("both organizations answer");

    let numbers: Vec<u64> = listed.iter().map(|pull_request| pull_request.number).collect();
    assert_eq!(numbers, [2, 1], "both organizations' rows, newest first");
    let arrived = arrivals(&arrived);
    let [(_, first), (_, second)] = arrived.as_slice() else {
        panic!("one listing per organization: {arrived:?}");
    };
    assert!(
        second.duration_since(*first) < ANSWER_DELAY,
        "the second listing went out before the first was answered"
    );
}

#[tokio::test]
async fn current_users_of_organizations_are_read_at_the_same_time() {
    let server = MockServer::start().await;
    let users = Arrivals::default();
    Mock::given(wiremock::matchers::path_regex(r"^/(north|south)/_apis/connectionData$"))
        .respond_with(ConnectionDatas { arrived: Arc::clone(&users) })
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::path_regex(r"^/(north|south)/_apis/git/pullrequests$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "count": 0, "value": [] })))
        .mount(&server)
        .await;
    let provider = provider(&server);

    provider
        .list_pull_requests(&query(PullRequestScope::CreatedByMe))
        .await
        .expect("both organizations answer");

    let users = arrivals(&users);
    let [(_, first), (_, second)] = users.as_slice() else {
        panic!("one user lookup per organization: {users:?}");
    };
    assert!(
        second.duration_since(*first) < ANSWER_DELAY,
        "the second organization's user lookup went out before the first was answered"
    );
}

#[tokio::test]
async fn callers_at_the_same_time_share_one_user_lookup() {
    let server = MockServer::start().await;
    let users = Arrivals::default();
    Mock::given(wiremock::matchers::path("/north/_apis/connectionData"))
        .respond_with(ConnectionDatas { arrived: Arc::clone(&users) })
        .mount(&server)
        .await;
    let provider = provider(&server);

    let (first, second) = tokio::join!(provider.current_user(), provider.current_user());

    assert_eq!(first.expect("the user answers").id, "me-north");
    assert_eq!(second.expect("the shared answer").id, "me-north");
    assert_eq!(arrivals(&users).len(), 1, "both callers shared one connectionData request");
}
