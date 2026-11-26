//! A private attachment in a pull request description: the real app and GitHub provider against
//! the mocked GitHub Enterprise host ([`super::github_mock`]) and a mocked storage host. The token
//! does not reach the image, so its card says so and offers Fix token; once the connection is
//! saved with a token that does, the open page loads the image without being reopened. Needs `git`
//! on PATH, no network, no real token.

use egui::Role;
use egui_kittest::kittest::Queryable;
use review_core::Connection;
use tokio::runtime::Runtime;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::action_e2e::{app, open_pull_request, press, wait_until};
use super::github_mock::{GitHubHost, NUMBER, REPOSITORY, TOKEN};
use cv_runtime::route::{PullRequestKey, Route};
use cv_testkit::ScratchDir;

const CONNECTION_ID: &str = "gh";
/// The token the connection is fixed with; it only ever reaches the mock host.
const FIXED_TOKEN: &str = "test-token-with-access";
const SIGNED_PATH: &str = "/64602043/634273213-e7d69315-2274-4873-a343-7c087c9f8543.png";
const PRIVATE: &str = "Sign-in needed: this image is private · 127.0.0.1";

fn png() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(40, 20)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode png");
    bytes.into_inner()
}

#[test]
fn private_image_loads_on_the_open_page_once_the_token_is_fixed() {
    let scratch = ScratchDir::new("private-image");
    let host = GitHubHost::with_pull_request(scratch.path());
    let connection = host.connection(CONNECTION_ID, "Acme");
    let asset = format!(
        "{}/user-attachments/assets/e7d69315-2274-4873-a343-7c087c9f8543",
        connection.base_url
    );
    host.describe(&format!("Before and after:\n\n<img alt=\"Screenshot\" src=\"{asset}\">"));
    // The first token does not reach the attachment: GitHub leaves its address as written, where it answers 404.
    host.render_images_as(TOKEN, &asset);
    let runtime = Runtime::new().expect("runtime");
    let storage = runtime.block_on(MockServer::start());
    cv_testkit::fresh_origin(&storage.uri());
    runtime.block_on(
        Mock::given(method("GET"))
            .and(path(SIGNED_PATH))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/png")
                    .set_body_bytes(png()),
            )
            .mount(&storage),
    );
    host.render_images_as(
        FIXED_TOKEN,
        &format!("{}{SIGNED_PATH}?jwt=header.payload.signature", storage.uri()),
    );

    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the private image's card", |harness| {
        harness.query_by_label(PRIVATE).is_some()
    });
    assert!(
        harness.query_by_role_and_label(Role::Button, "Open in browser").is_some(),
        "the browser is a way on"
    );

    press(&mut harness, "Fix token");
    wait_until(&mut harness, "the connection setup", |harness| {
        harness.state().route == Route::Settings
    });
    // What the setup's save does: store the connection with its new token and announce it.
    let services = &mut harness.state_mut().services;
    services
        .connections
        .upsert(Connection { token: Some(FIXED_TOKEN.to_owned()), ..connection.clone() })
        .expect("save the fixed connection");
    services.connection_saved(CONNECTION_ID);

    let key = PullRequestKey {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number: NUMBER,
    };
    harness.state_mut().navigator.go(Route::PullRequest(key));
    wait_until(&mut harness, "the image", |harness| {
        harness.query_by_role_and_label(Role::Image, "Image Screenshot").is_some()
    });
    let fetched = runtime.block_on(storage.received_requests()).expect("recorded requests");
    assert_eq!(fetched.len(), 1, "the image loaded once, by its signed address");
    assert!(
        fetched.iter().all(|request| !request.headers.contains_key("authorization")),
        "the token stayed on GitHub"
    );
}
