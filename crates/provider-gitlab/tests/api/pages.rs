//! `Link`-header paging against a mock GitLab, on the label catalog
//! (`GET /projects/:id/labels`): the next page is asked on the configured host whatever host
//! the link names, and a link whose path leaves the API root stops the list with an error.

use review_core::{Provider, ProviderError};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{self, TOKEN};

const REPO: &str = "group/app";
const LABELS: &str = "/api/v4/projects/group%2Fapp/labels";

fn page(name: &str, next: Option<&str>) -> ResponseTemplate {
    let answer = ResponseTemplate::new(200).set_body_json(json!([{ "name": name }]));
    match next {
        Some(link) => answer.insert_header("Link", format!("<{link}>; rel=\"next\"").as_str()),
        None => answer,
    }
}

async fn mount_page(server: &MockServer, number: Option<&str>, answer: ResponseTemplate) {
    let mock = Mock::given(method("GET")).and(path(LABELS)).and(header("PRIVATE-TOKEN", TOKEN));
    let mock = match number {
        Some(number) => mock.and(query_param("page", number)),
        None => mock.and(query_param_is_missing("page")),
    };
    mock.respond_with(answer).expect(1).mount(server).await;
}

/// The bug: a `Link` header naming another host took `PRIVATE-TOKEN` there.
#[tokio::test]
async fn page_links_never_take_the_token_to_another_host() {
    let server = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    let second = format!("{}{LABELS}?page=2&per_page=100", elsewhere.uri());
    let third = format!("{}{LABELS}?page=3&per_page=100", server.uri());
    mount_page(&server, None, page("bug", Some(&second))).await;
    mount_page(&server, Some("2"), page("docs", Some(&third))).await;
    mount_page(&server, Some("3"), page("ui", None)).await;

    let labels =
        support::provider(&server, Some(TOKEN)).available_labels(REPO).await.expect("labels");

    let names: Vec<&str> = labels.iter().map(|label| label.name.as_str()).collect();
    assert_eq!(names, ["bug", "docs", "ui"]);
    assert!(
        elsewhere.received_requests().await.expect("recorded").is_empty(),
        "the host the link named got no request"
    );
}

#[tokio::test]
async fn a_page_link_outside_the_api_root_stops_the_list() {
    for link in ["/api/v4evil/x?page=2", "not a url"] {
        let server = MockServer::start().await;
        let next =
            if link.starts_with('/') { format!("{}{link}", server.uri()) } else { link.to_owned() };
        mount_page(&server, None, page("bug", Some(&next))).await;

        let error = support::provider(&server, Some(TOKEN))
            .available_labels(REPO)
            .await
            .expect_err("the link leaves the API root");

        assert!(
            matches!(&error, ProviderError::Decode(message) if message.contains("pagination link")),
            "{link}: {error:?}"
        );
        assert_eq!(server.received_requests().await.expect("recorded").len(), 1, "{link}");
    }
}
