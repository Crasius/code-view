//! Upload images against a mock GitLab: with a token they load through the uploads API
//! (`GET /projects/:id/uploads/:secret/:filename`, GitLab 17.4) with `PRIVATE-TOKEN`, which never
//! follows a redirect to storage; where the API does not serve them and the web route answers
//! with the sign-in page, the image is private. Raw repository files load through the files API.

use provider_gitlab::GitLabProvider;
use review_core::http::image::{Body, Validators};
use review_core::{Connection, Provider, ProviderKind};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const SECRET: &str = "0123456789abcdef0123456789abcdef";
const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn provider(server: &MockServer) -> GitLabProvider {
    let connection = Connection {
        id: "gitlab-images".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::GitLab,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: Some("test-token".to_owned()),
    };
    GitLabProvider::new(connection, reqwest::Client::new())
}

#[tokio::test]
async fn uploads_load_through_the_api_and_the_token_stays_on_gitlab() {
    let gitlab = MockServer::start().await;
    let storage = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/projects/group%2Fapp/uploads/{SECRET}/shot.png")))
        .and(header("PRIVATE-TOKEN", "test-token"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            format!("{}/objects/shot.png?X-Amz-Signature=x", storage.uri()),
        ))
        .mount(&gitlab)
        .await;
    Mock::given(method("GET"))
        .and(path("/objects/shot.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "image/png")
                .set_body_bytes(PNG.to_vec()),
        )
        .mount(&storage)
        .await;

    let url = format!("{}/group/app/uploads/{SECRET}/shot.png", gitlab.uri());
    let body =
        provider(&gitlab).fetch_image("group/app", &url, 1024, &Validators::NONE).await.unwrap();
    let Body::Image(image) = body else { panic!("{body:?}") };
    assert_eq!(&image.bytes[..], &PNG);
    let storage_requests: Vec<Request> = storage.received_requests().await.unwrap();
    assert!(
        storage_requests.iter().all(|request| !request.headers.contains_key("private-token")),
        "the token left GitLab"
    );
}

/// `/-/raw/feature/docs/a.png` names ref `feature/docs` and file `a.png`; the ref `feature` the
/// first split tries does not exist.
#[tokio::test]
async fn a_raw_file_on_a_branch_with_slashes_loads_through_the_api() {
    let gitlab = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/group%2Fapp/repository/files/a.png/raw"))
        .and(query_param("ref", "feature/docs"))
        .and(header("PRIVATE-TOKEN", "test-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "image/png")
                .set_body_bytes(PNG.to_vec()),
        )
        .mount(&gitlab)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/group%2Fapp/repository/files/docs%2Fa.png/raw"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&gitlab)
        .await;

    let url = format!("{}/group/app/-/raw/feature/docs/a.png", gitlab.uri());
    let body =
        provider(&gitlab).fetch_image("group/app", &url, 1024, &Validators::NONE).await.unwrap();
    let Body::Image(image) = body else { panic!("{body:?}") };
    assert_eq!(&image.bytes[..], &PNG);
}

#[tokio::test]
async fn a_sign_in_page_means_the_image_is_private() {
    let gitlab = MockServer::start().await;
    let upload = format!("/group/app/uploads/{SECRET}/shot.png");
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/projects/group%2Fapp/uploads/{SECRET}/shot.png")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&gitlab)
        .await;
    Mock::given(method("GET"))
        .and(path(upload.as_str()))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/users/sign_in"))
        .mount(&gitlab)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/sign_in"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw("<!DOCTYPE html><title>Sign in</title>", "text/html; charset=utf-8"),
        )
        .mount(&gitlab)
        .await;

    let body = provider(&gitlab)
        .fetch_image("group/app", &format!("{}{upload}", gitlab.uri()), 1024, &Validators::NONE)
        .await
        .unwrap();
    assert!(matches!(body, Body::Private), "{body:?}");
}
