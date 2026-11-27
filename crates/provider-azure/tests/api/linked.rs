//! Work items and pull requests by id against a mock Azure DevOps REST API (api-version 7.1):
//! the pull request's `workitems` list, one `workitemsbatch` POST, and `git/pullrequests/{id}`.

use review_core::{ItemKind, ItemRef, LinkedState, Provider};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{TOKEN, provider};

const ORG: &str = "coll";

#[tokio::test]
async fn lists_the_pull_requests_work_items() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/{ORG}/Web/_apis/git/repositories/site/pullrequests/7/workitems")))
        .and(query_param("api-version", "7.1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "count": 2, "value": [ { "id": "31", "url": "x" }, { "id": "32", "url": "y" } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let references = provider(&server, &[ORG], Some(TOKEN))
        .linked_references(&format!("{ORG}/Web/site"), 7)
        .await
        .expect("work items");
    let ids: Vec<u64> = references.iter().map(|reference| reference.number).collect();
    assert_eq!(ids, [31, 32]);
    assert!(references.iter().all(|reference| reference.kind == ItemKind::WorkItem
        && reference.repository_key == format!("{ORG}/Web")));
}

#[tokio::test]
async fn reads_work_items_in_one_batch_and_pull_requests_by_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/{ORG}/_apis/wit/workitemsbatch")))
        .and(query_param("api-version", "7.1"))
        .and(body_partial_json(json!({ "ids": [31, 99], "errorPolicy": "omit" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "count": 2,
            "value": [
                { "id": 31, "rev": 3, "fields": { "System.Id": 31, "System.Title": "Login bug", "System.State": "Resolved",
                                                  "System.WorkItemType": "Bug", "System.TeamProject": "Web" } },
                null
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/{ORG}/_apis/git/pullrequests/40")))
        .and(query_param("api-version", "7.1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "pullRequestId": 40, "status": "active", "isDraft": true, "title": "API change",
            "creationDate": "2026-10-01T09:30:00Z",
            "repository": { "name": "api", "project": { "name": "Web" } },
            "createdBy": { "id": "u1", "displayName": "Ada" },
            "sourceRefName": "refs/heads/api", "targetRefName": "refs/heads/main"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let asked = [
        ItemRef { kind: ItemKind::WorkItem, repository_key: format!("{ORG}/Web"), number: 31 },
        ItemRef { kind: ItemKind::WorkItem, repository_key: format!("{ORG}/Web"), number: 99 },
        ItemRef {
            kind: ItemKind::PullRequest,
            repository_key: format!("{ORG}/Web/site"),
            number: 40,
        },
    ];
    let items =
        provider(&server, &[ORG], Some(TOKEN)).linked_items(&asked).await.expect("linked items");
    assert_eq!(items.len(), 2, "{items:?}");
    let bug = &items[0];
    assert_eq!(
        (bug.state, bug.state_label.as_str(), bug.item_type.as_deref()),
        (LinkedState::Done, "Resolved", Some("Bug"))
    );
    assert!(bug.web_url.ends_with("/coll/Web/_workitems/edit/31"), "{}", bug.web_url);
    let pull = &items[1];
    assert_eq!(pull.reference, asked[2], "answers carry the reference as asked");
    assert_eq!(
        (pull.repository_key.as_str(), pull.state),
        ("coll/Web/api", LinkedState::Draft),
        "the real repository"
    );
}

/// A mention (`#31` in project Web) and a link to the same work item under another project
/// name one item: it is read once and both references get it.
#[tokio::test]
async fn one_work_item_named_twice_answers_both_references() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/{ORG}/_apis/wit/workitemsbatch")))
        .and(body_partial_json(json!({ "ids": [31] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "count": 1,
            "value": [ { "id": 31, "fields": { "System.Title": "Login bug", "System.State": "Active",
                                               "System.TeamProject": "Web" } } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let asked = [
        ItemRef { kind: ItemKind::WorkItem, repository_key: format!("{ORG}/Web"), number: 31 },
        ItemRef { kind: ItemKind::WorkItem, repository_key: format!("{ORG}/Other"), number: 31 },
    ];
    let items =
        provider(&server, &[ORG], Some(TOKEN)).linked_items(&asked).await.expect("linked items");
    let answered: Vec<&ItemRef> = items.iter().map(|item| &item.reference).collect();
    assert_eq!(answered, [&asked[0], &asked[1]]);
    assert!(items.iter().all(|item| item.title == "Login bug"));
}
