//! The list rows' checks and votes from one GraphQL query against a mock GitLab
//! (<https://docs.gitlab.com/api/graphql/reference/#mergerequest>). The error answer is the one
//! graphql-ruby's static validation writes for an unknown field, as a Community Edition host
//! answers for the Premium approval counts. `.expect(n)` is verified when the server drops.

use provider_gitlab::GitLabProvider;
use review_core::{CheckState, Connection, Provider, ProviderKind, ReviewVote, RowStatus};
use serde_json::Value;
use wiremock::matchers::{body_partial_json, body_string_contains, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ROWS: &str = include_str!("../fixtures/graphql_row_statuses.json");
const UNKNOWN_FIELD: &str = include_str!("../fixtures/graphql_unknown_field.json");

fn provider(server: &MockServer) -> GitLabProvider {
    let connection = Connection {
        id: "rows".to_owned(),
        name: "Mock GitLab".to_owned(),
        kind: ProviderKind::GitLab,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: Some("test-token".to_owned()),
    };
    GitLabProvider::new(connection, reqwest::Client::new())
}

fn answer(json: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(serde_json::from_str::<Value>(json).expect("fixture is JSON"))
}

/// The fixture as a host without the approval counts answers it.
fn without_approval_counts() -> ResponseTemplate {
    let mut rows: Value = serde_json::from_str(ROWS).expect("fixture is JSON");
    for node in rows["data"]["project"]["mergeRequests"]["nodes"].as_array_mut().expect("nodes") {
        let node = node.as_object_mut().expect("node");
        node.remove("approvalsRequired");
        node.remove("approvalsLeft");
    }
    ResponseTemplate::new(200).set_body_json(rows)
}

fn checks(status: &RowStatus) -> Vec<(&str, CheckState)> {
    status.checks.iter().map(|check| (check.name.as_str(), check.state)).collect()
}

fn asks_approval_counts(request: &Request) -> bool {
    String::from_utf8_lossy(&request.body).contains("approvalsLeft")
}

#[tokio::test]
async fn rows_get_stages_approvals_and_votes_in_one_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_partial_json(
            serde_json::json!({ "variables": { "project": "group/app", "iids": ["4", "5"] } }),
        ))
        .and(body_string_contains("approvalsLeft"))
        .respond_with(answer(ROWS))
        .expect(1)
        .mount(&server)
        .await;

    let statuses = provider(&server).row_statuses("group/app", &[4, 5]).await.expect("statuses");

    let (iid, status) = statuses.first().expect("first row");
    assert_eq!(*iid, 4);
    assert_eq!(checks(status), [("build", CheckState::Passed), ("test", CheckState::Failed)]);
    assert_eq!(
        status.required_approvals.as_ref().map(|rule| rule.state),
        Some(CheckState::Pending),
        "the approval rule is no check"
    );
    let votes: Vec<(&str, ReviewVote)> = status
        .reviewers
        .iter()
        .flatten()
        .map(|reviewer| (reviewer.identity.display_name.as_str(), reviewer.vote))
        .collect();
    assert_eq!(votes, [("Ann", ReviewVote::ChangesRequested), ("Bob", ReviewVote::Approved)]);
    let (iid, status) = statuses.get(1).expect("second row");
    assert_eq!((*iid, status.verdict()), (5, None), "no pipeline, no approval rule");
}

/// Owner report: on GitLab the rows' checks failed with an unknown field. The Community Edition
/// has no `approvalsRequired`/`approvalsLeft`, and GraphQL failed the whole query for them. The
/// host's answer is learned once: the rows get their stages and votes without the approval
/// check, and the next load asks without the counts at once.
#[tokio::test]
async fn a_host_without_the_approval_counts_still_gets_its_checks_and_votes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(asks_approval_counts)
        .respond_with(answer(UNKNOWN_FIELD))
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains("headPipeline"))
        .respond_with(without_approval_counts())
        .expect(2)
        .mount(&server)
        .await;
    let provider = provider(&server);

    for load in ["first", "second"] {
        let statuses = provider.row_statuses("group/app", &[4, 5]).await.expect(load);
        let (_, status) = statuses.first().expect("first row");
        assert_eq!(
            checks(status),
            [("build", CheckState::Passed), ("test", CheckState::Failed)],
            "{load} load"
        );
        assert_eq!(status.reviewers.as_ref().map(Vec::len), Some(2), "{load} load");
    }
}

/// An unknown field the query cannot do without stays an error with GitLab's own message.
#[tokio::test]
async fn an_unknown_field_the_query_needs_is_reported() {
    let server = MockServer::start().await;
    let error = serde_json::json!({ "errors": [ {
        "message": "Field 'reviewers' doesn't exist on type 'MergeRequest'",
        "extensions": { "code": "undefinedField", "typeName": "MergeRequest", "fieldName": "reviewers" }
    } ] });
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(error))
        .expect(1)
        .mount(&server)
        .await;

    let result = provider(&server).row_statuses("group/app", &[4]).await;

    let message = result.expect_err("no data").to_string();
    assert!(
        message.contains("Field 'reviewers' doesn't exist on type 'MergeRequest'"),
        "{message}"
    );
}
