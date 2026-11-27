//! Linked item states against a mock GitLab: one `iids[]` list request per project and kind.

use review_core::{ItemKind, ItemRef, LinkedState, Provider};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{self, TOKEN};

fn reference(kind: ItemKind, repository: &str, number: u64) -> ItemRef {
    ItemRef { kind, repository_key: repository.to_owned(), number }
}

#[tokio::test]
async fn batches_merge_requests_and_issues_per_project() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/group%2Fapp/merge_requests"))
        .and(query_param("iids[]", "12"))
        .and(query_param("iids[]", "13"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "iid": 12, "title": "Add cache", "state": "merged", "draft": false, "web_url": "https://gl/group/app/-/merge_requests/12" },
            { "iid": 13, "title": "Try idea", "state": "opened", "draft": true, "web_url": "https://gl/group/app/-/merge_requests/13" }
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/group%2Fapp/issues"))
        .and(query_param("iids[]", "4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "iid": 4, "title": "Cache misses", "state": "closed", "web_url": "https://gl/group/app/-/issues/4" }
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/group%2Fgone/merge_requests"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({ "message": "404 Project Not Found" })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let asked = [
        reference(ItemKind::PullRequest, "group/app", 12),
        reference(ItemKind::PullRequest, "group/app", 13),
        reference(ItemKind::Issue, "group/app", 4),
        reference(ItemKind::PullRequest, "group/gone", 1),
    ];
    let mut items =
        support::provider(&server, Some(TOKEN)).linked_items(&asked).await.expect("linked items");
    items.sort_by_key(|item| item.reference.number);
    let states: Vec<(u64, ItemKind, LinkedState)> =
        items.iter().map(|item| (item.reference.number, item.kind, item.state)).collect();
    assert_eq!(
        states,
        [
            (4, ItemKind::Issue, LinkedState::Done),
            (12, ItemKind::PullRequest, LinkedState::Merged),
            (13, ItemKind::PullRequest, LinkedState::Draft)
        ]
    );
}
