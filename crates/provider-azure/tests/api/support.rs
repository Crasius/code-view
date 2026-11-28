//! What the API tests share: the provider on a mock server, the request matcher with the PAT
//! header, and the pull request `coll/My Project/web!7` the write tests answer with.

use provider_azure::AzureDevOpsProvider;
use review_core::{Connection, ProviderKind, http};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

pub(crate) const COLLECTION: &str = "coll";
pub(crate) const REPO: &str = "coll/My Project/web";
pub(crate) const PR: u64 = 7;
pub(crate) const TOKEN: &str = "dummy-pat";
/// `base64(":dummy-pat")`.
pub(crate) const AUTHORIZATION: &str = "Basic OmR1bW15LXBhdA==";

/// The provider on `server`, the base URL of an Azure DevOps Server with `organizations` as its
/// collections. The tests share one process, and wiremock hands a finished test's port out again:
/// a rate-limit pause an earlier test left on this origin is ended first.
pub(crate) fn provider(
    server: &MockServer,
    organizations: &[&str],
    token: Option<&str>,
) -> AzureDevOpsProvider {
    http::resume(&server.uri());
    let connection = Connection {
        id: "test".to_owned(),
        name: "test".to_owned(),
        kind: ProviderKind::AzureDevOps,
        base_url: server.uri(),
        organizations: organizations
            .iter()
            .map(|organization| (*organization).to_owned())
            .collect(),
        token: token.map(str::to_owned),
    };
    AzureDevOpsProvider::new(connection, http::build_client())
}

/// Method + exact path + `api-version=7.1` + PAT header.
pub(crate) fn api(verb: &str, url_path: &str) -> MockBuilder {
    Mock::given(method(verb))
        .and(path(url_path))
        .and(query_param("api-version", "7.1"))
        .and(header("authorization", AUTHORIZATION))
}

/// The path of the pull request [`PR`] of [`REPO`], followed by `suffix`.
pub(crate) fn pr_path(suffix: &str) -> String {
    format!("/{COLLECTION}/My%20Project/_apis/git/repositories/web/pullrequests/{PR}{suffix}")
}

pub(crate) fn ok(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

/// The pull request [`PR`] of [`REPO`] from `feature` into `main`, with the fields of `extra` on top.
pub(crate) fn pull_request(extra: &Value) -> Value {
    let mut value = json!({
        "pullRequestId": PR, "status": "active",
        "repository": { "name": "web", "project": { "name": "My Project" } },
        "createdBy": { "id": "author", "displayName": "Author" },
        "creationDate": "2026-10-01T08:00:00Z", "title": "T",
        "sourceRefName": "refs/heads/feature", "targetRefName": "refs/heads/main",
        "lastMergeSourceCommit": { "commitId": "head-sha" }, "lastMergeTargetCommit": { "commitId": "target-sha" }
    });
    value.as_object_mut().expect("an object").extend(extra.as_object().expect("an object").clone());
    value
}

/// `Refs - List` of the source branch (`refs/heads/feature`) answers `tip`; with `times`, exactly
/// that many times.
pub(crate) async fn mount_source_tip(server: &MockServer, tip: &str, times: Option<u64>) {
    let mock = api("GET", &format!("/{COLLECTION}/My%20Project/_apis/git/repositories/web/refs"))
        .and(query_param("filter", "heads/feature"))
        .respond_with(ok(json!({ "count": 2, "value": [
            { "name": "refs/heads/feature-2", "objectId": "other-sha" },
            { "name": "refs/heads/feature", "objectId": tip }
        ] })));
    match times {
        Some(times) => mock.up_to_n_times(times).expect(times),
        None => mock,
    }
    .mount(server)
    .await;
}
