//! Change detection for an open pull request against a mocked Azure DevOps REST API (7.1): the
//! probe is the pull request plus its threads, with `api-version` as the only query parameter
//! (learn.microsoft.com/rest/api/azure/devops/git/pull-request-threads/list). `.expect(n)` is
//! verified when the server drops.

use provider_azure::AzureDevOpsProvider;
use review_core::{Connection, Provider, ProviderKind, http};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "coll/My Project/web";
const PULL: &str = "/coll/My%20Project/_apis/git/repositories/web/pullrequests/7";

fn provider(server: &MockServer) -> AzureDevOpsProvider {
    let connection = Connection {
        id: "live".to_owned(),
        name: "live".to_owned(),
        kind: ProviderKind::AzureDevOps,
        base_url: server.uri(),
        organizations: vec!["coll".to_owned()],
        token: Some("dummy-pat".to_owned()),
    };
    AzureDevOpsProvider::new(connection, http::build_client())
}

fn pull_request(source: &str, vote: i32) -> Value {
    json!({
        "pullRequestId": 7, "status": "active", "title": "Change",
        "repository": { "name": "web", "project": { "name": "My Project" } },
        "createdBy": { "id": "author", "displayName": "Grace" }, "creationDate": "2026-01-01T10:00:00Z",
        "sourceRefName": "refs/heads/feature", "targetRefName": "refs/heads/main",
        "lastMergeSourceCommit": { "commitId": source }, "lastMergeTargetCommit": { "commitId": "target1" },
        "reviewers": [{ "id": "r1", "displayName": "Linus", "vote": vote }],
    })
}

fn threads(last_updated: &str) -> Value {
    json!({ "count": 1, "value": [{
        "id": 11, "status": "active", "publishedDate": "2026-01-01T10:00:00Z", "lastUpdatedDate": last_updated,
        "comments": [{ "id": 1, "content": "Why?", "publishedDate": "2026-01-01T10:00:00Z", "author": { "id": "a" } }]
    }] })
}

async fn mount_sequence(server: &MockServer, url_path: &str, answers: &[Value]) {
    for (index, answer) in answers.iter().enumerate() {
        Mock::given(method("GET"))
            .and(path(url_path))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .up_to_n_times(1)
            .with_priority(u8::try_from(index + 1).expect("fewer than 255 answers"))
            .expect(1)
            .mount(server)
            .await;
    }
}

#[tokio::test]
async fn replies_votes_and_pushes_change_the_revision() {
    let server = MockServer::start().await;
    let pulls = [
        pull_request("head1", 0),
        pull_request("head1", 0),
        pull_request("head1", 10),
        pull_request("head2", 10),
    ];
    mount_sequence(&server, PULL, &pulls).await;
    let replies = [
        threads("2026-01-01T10:00:00Z"),
        threads("2026-01-02T09:00:00Z"),
        threads("2026-01-02T09:00:00Z"),
        threads("2026-01-02T09:00:00Z"),
    ];
    mount_sequence(&server, &format!("{PULL}/threads"), &replies).await;
    let provider = provider(&server);

    let opened = provider.revision(REPO, 7).await.expect("probe");
    assert_eq!(opened.head_sha, "head1");
    let replied = provider.revision(REPO, 7).await.expect("probe after a reply");
    assert_ne!(replied.marker, opened.marker, "the thread's lastUpdatedDate moved");
    let voted = provider.revision(REPO, 7).await.expect("probe after a vote");
    assert_ne!(voted.marker, replied.marker, "the reviewer voted");
    let pushed = provider.revision(REPO, 7).await.expect("probe after a push");
    assert_eq!((pushed.head_sha.as_str(), pushed.marker), ("head2", voted.marker));
}
