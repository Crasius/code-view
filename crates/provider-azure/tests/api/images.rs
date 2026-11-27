//! Attachment images against a mock Azure DevOps Server: the PAT goes to the server's attachment
//! API and never along a redirect to storage; the sign-in page the server answers without access
//! (`203 Non-Authoritative Information` with HTML) means the image is private.

use review_core::Provider;
use review_core::http::image::{Body, Validators};
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::support::{COLLECTION, TOKEN, provider};

const ATTACHMENT: &str =
    "/coll/project/_apis/git/repositories/repo/pullRequests/42/attachments/shot.png";
const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

#[tokio::test]
async fn attachments_take_the_pat_and_storage_does_not() {
    let server = MockServer::start().await;
    let storage = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ATTACHMENT))
        .and(header_exists("authorization"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/blob/shot.png?sig=x", storage.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/blob/shot.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "image/png")
                .set_body_bytes(PNG.to_vec()),
        )
        .mount(&storage)
        .await;

    let body = provider(&server, &[COLLECTION], Some(TOKEN))
        .fetch_image(
            "coll/project/repo",
            &format!("{}{ATTACHMENT}", server.uri()),
            1024,
            &Validators::NONE,
        )
        .await
        .unwrap();
    let Body::Image(image) = body else { panic!("{body:?}") };
    assert_eq!(&image.bytes[..], &PNG);
    let storage_requests: Vec<Request> = storage.received_requests().await.unwrap();
    assert!(
        storage_requests.iter().all(|request| !request.headers.contains_key("authorization")),
        "the PAT left the server"
    );
}

#[tokio::test]
async fn the_sign_in_page_means_the_image_is_private() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ATTACHMENT))
        .respond_with(
            ResponseTemplate::new(203)
                .set_body_raw("<!DOCTYPE html><title>Sign in</title>", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let body = provider(&server, &[COLLECTION], Some(TOKEN))
        .fetch_image(
            "coll/project/repo",
            &format!("{}{ATTACHMENT}", server.uri()),
            1024,
            &Validators::NONE,
        )
        .await
        .unwrap();
    assert!(matches!(body, Body::Private), "{body:?}");
}
