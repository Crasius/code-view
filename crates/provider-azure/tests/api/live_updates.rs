//! Change detection for an open pull request against a mocked Azure DevOps REST API (7.1): the
//! probe is the pull request plus its threads, with `api-version` as the only query parameter
//! (learn.microsoft.com/rest/api/azure/devops/git/pull-request-threads/list). `.expect(n)` is
//! verified when the server drops.

use review_core::Provider;
use serde_json::{Value, json};
use wiremock::MockServer;

use crate::support::{COLLECTION, REPO, TOKEN, api, ok, pr_path, provider};

fn pull_request(source: &str, vote: i32) -> Value {
    crate::support::pull_request(&json!({
        "lastMergeSourceCommit": { "commitId": source },
        "reviewers": [{ "id": "r1", "displayName": "Linus", "vote": vote }],
    }))
}

fn threads(last_updated: &str) -> Value {
    json!({ "count": 1, "value": [{
        "id": 11, "status": "active", "publishedDate": "2026-01-01T10:00:00Z", "lastUpdatedDate": last_updated,
        "comments": [{ "id": 1, "content": "Why?", "publishedDate": "2026-01-01T10:00:00Z", "author": { "id": "a" } }]
    }] })
}

async fn mount_sequence(server: &MockServer, url_path: &str, answers: &[Value]) {
    for (index, answer) in answers.iter().enumerate() {
        api("GET", url_path)
            .respond_with(ok(answer.clone()))
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
    mount_sequence(&server, &pr_path(""), &pulls).await;
    let replies = [
        threads("2026-01-01T10:00:00Z"),
        threads("2026-01-02T09:00:00Z"),
        threads("2026-01-02T09:00:00Z"),
        threads("2026-01-02T09:00:00Z"),
    ];
    mount_sequence(&server, &pr_path("/threads"), &replies).await;
    let provider = provider(&server, &[COLLECTION], Some(TOKEN));

    let opened = provider.revision(REPO, 7).await.expect("probe");
    assert_eq!(opened.head_sha, "head1");
    let replied = provider.revision(REPO, 7).await.expect("probe after a reply");
    assert_ne!(replied.marker, opened.marker, "the thread's lastUpdatedDate moved");
    let voted = provider.revision(REPO, 7).await.expect("probe after a vote");
    assert_ne!(voted.marker, replied.marker, "the reviewer voted");
    let pushed = provider.revision(REPO, 7).await.expect("probe after a push");
    assert_eq!((pushed.head_sha.as_str(), pushed.marker), ("head2", voted.marker));
}
