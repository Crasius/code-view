//! Attachment images against a mock GitHub Enterprise host and a mock storage host: with a token,
//! an attachment loads through the address `POST /markdown` (gfm, in the repository's context)
//! signs, and that address goes out without the token; an address GitHub does not sign is
//! private; without a token the address as written loads anonymously.

use provider_github::GitHubProvider;
use review_core::http::image::{Body, Validators};
use review_core::{Connection, Provider, ProviderKind};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ASSET: &str = "/user-attachments/assets/e7d69315-2274-4873-a343-7c087c9f8543";
const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn provider(server: &MockServer, token: Option<&str>) -> GitHubProvider {
    let connection = Connection {
        id: "images".to_owned(),
        name: "Mock GitHub Enterprise".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: token.map(str::to_owned),
    };
    GitHubProvider::new(connection, reqwest::Client::new())
}

fn image() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("Content-Type", "image/png")
        .insert_header("Cache-Control", "max-age=2592000")
        .set_body_bytes(PNG.to_vec())
}

/// What `POST /markdown` answers for one image: GitHub's markup around `src`.
fn rendered(src: &str) -> ResponseTemplate {
    let html = format!(
        r#"<p dir="auto"><a target="_blank" rel="noopener noreferrer" href="{src}"><img src="{src}" style="max-width: 100%;"></a></p>"#
    );
    ResponseTemplate::new(200).set_body_raw(html, "text/html;charset=utf-8")
}

fn render_request() -> wiremock::MockBuilder {
    Mock::given(method("POST"))
        .and(path("/api/v3/markdown"))
        .and(header("authorization", "Bearer test-token"))
        .and(body_string_contains(r#""mode":"gfm""#))
        .and(body_string_contains(r#""context":"o/r""#))
        .and(body_string_contains(ASSET))
}

#[tokio::test]
async fn attachments_load_through_the_signed_address_without_the_token() {
    let github = MockServer::start().await;
    let storage = MockServer::start().await;
    let signed = format!(
        "{}/64602043/634273213-e7d69315-2274-4873-a343-7c087c9f8543.png?jwt=header.payload.signature&amp;v=1",
        storage.uri()
    );
    render_request().respond_with(rendered(&signed)).expect(1).mount(&github).await;
    Mock::given(method("GET"))
        .and(path("/64602043/634273213-e7d69315-2274-4873-a343-7c087c9f8543.png"))
        .respond_with(image())
        .mount(&storage)
        .await;

    let body = provider(&github, Some("test-token"))
        .fetch_image("o/r", &format!("{}{ASSET}", github.uri()), 1024, &Validators::NONE)
        .await
        .unwrap();
    let Body::Image(image) = body else { panic!("{body:?}") };
    assert_eq!(&image.bytes[..], &PNG);
    let storage_requests: Vec<Request> = storage.received_requests().await.unwrap();
    assert_eq!(storage_requests.len(), 1);
    assert!(
        storage_requests.iter().all(|request| !request.headers.contains_key("authorization")),
        "the token left GitHub"
    );
    let github_requests: Vec<Request> = github.received_requests().await.unwrap();
    assert!(
        github_requests.iter().all(|request| request.url.path() != ASSET),
        "the address as written is not asked for"
    );
}

#[tokio::test]
async fn an_attachment_github_does_not_sign_is_private() {
    let github = MockServer::start().await;
    let canonical = format!("{}{ASSET}", github.uri());
    render_request().respond_with(rendered(&canonical)).expect(1).mount(&github).await;
    Mock::given(method("GET"))
        .and(path(ASSET))
        .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
        .mount(&github)
        .await;

    let body = provider(&github, Some("test-token"))
        .fetch_image("o/r", &canonical, 1024, &Validators::NONE)
        .await
        .unwrap();
    assert!(matches!(body, Body::Private), "{body:?}");
}

#[tokio::test]
async fn without_a_token_attachments_load_as_written() {
    let github = MockServer::start().await;
    let storage = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ASSET))
        .respond_with(
            ResponseTemplate::new(302).insert_header(
                "Location",
                format!("{}/bucket/a.png?X-Amz-Signature=x", storage.uri()),
            ),
        )
        .mount(&github)
        .await;
    Mock::given(method("GET"))
        .and(path("/bucket/a.png"))
        .respond_with(image())
        .mount(&storage)
        .await;

    let body = provider(&github, None)
        .fetch_image("o/r", &format!("{}{ASSET}", github.uri()), 1024, &Validators::NONE)
        .await
        .unwrap();
    assert!(matches!(body, Body::Image(_)), "{body:?}");
    let github_requests: Vec<Request> = github.received_requests().await.unwrap();
    assert!(
        github_requests.iter().all(|request| request.url.path() != "/api/v3/markdown"),
        "no render without a token"
    );
}
