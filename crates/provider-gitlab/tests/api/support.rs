//! The provider and the project answer every test module builds: a GitLab connection to the mock
//! host on the app's own HTTP client, so the tests send the user agent, compression and timeouts
//! the app sends.

use provider_gitlab::GitLabProvider;
use review_core::{Connection, ProviderKind, http};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub(crate) const TOKEN: &str = "test-token";
pub(crate) const REPO: &str = "group/sub/project";
/// `GET /projects/:id` of [`REPO`].
pub(crate) const PROJECT: &str = "/api/v4/projects/group%2Fsub%2Fproject";

/// The connection `mock` to `server`, without groups.
pub(crate) fn connection(server: &MockServer, token: Option<&str>) -> Connection {
    Connection {
        id: "mock".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::GitLab,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: token.map(str::to_owned),
    }
}

/// The provider on `connection`. The rate-limit pauses of `review_core::http` last for the whole
/// test process and wiremock hands a finished test's port out again, so the host's pauses end
/// first.
pub(crate) fn provider_on(connection: Connection) -> GitLabProvider {
    http::resume(connection.base_url());
    GitLabProvider::new(connection, http::build_client())
}

pub(crate) fn provider(server: &MockServer, token: Option<&str>) -> GitLabProvider {
    provider_on(connection(server, token))
}

/// The project [`REPO`] with the merge settings of `settings` on top.
pub(crate) fn project(settings: Value) -> Value {
    let mut body = json!({
        "id": 42, "path_with_namespace": REPO, "name": "project", "web_url": "https://gitlab.example.com/group/sub/project",
        "default_branch": "main", "http_url_to_repo": "https://gitlab.example.com/group/sub/project.git",
    });
    let (Value::Object(fields), Value::Object(settings)) = (&mut body, settings) else {
        unreachable!("the body and the settings are JSON objects");
    };
    fields.extend(settings);
    body
}

/// [`PROJECT`] answers `body`; merges read it first.
pub(crate) async fn mount_project(server: &MockServer, body: Value) {
    Mock::given(method("GET"))
        .and(path(PROJECT))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}
