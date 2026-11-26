//! "Copy image" in the real app: an image in a pull request description, served by a mocked
//! storage host, goes to the clipboard as its pixels from its context menu and from the lightbox
//! with Ctrl+C, and a toast confirms each copy. Test builds copy to the fake clipboard
//! ([`cv_media::image_clipboard::FakeClipboard`]), never to the system's. Needs `git` on
//! PATH, no network.

use egui::Role;
use egui_kittest::kittest::Queryable;
use tokio::runtime::Runtime;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::action_e2e::{app, open_pull_request, press, wait_until};
use super::clicks::Clicks;
use super::github_mock::{GitHubHost, TOKEN};
use cv_media::image_clipboard::COPIED;
use cv_testkit::ScratchDir;

const IMAGE_PATH: &str = "/assets/screenshot.png";

/// A 40 × 20 PNG whose pixels count up, so a copy shows it kept every one in place.
fn png() -> (Vec<u8>, Vec<u8>) {
    let image = image::RgbaImage::from_fn(40, 20, |x, y| image::Rgba([x as u8, y as u8, 7, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).expect("encode png");
    (bytes.into_inner(), image.into_raw())
}

#[test]
fn image_copy_puts_the_pixels_on_the_clipboard_from_the_menu_and_the_lightbox() {
    let scratch = ScratchDir::new("image-copy");
    let host = GitHubHost::with_pull_request(scratch.path());
    let connection = host.connection("gh", "Acme");
    let runtime = Runtime::new().expect("runtime");
    let storage = runtime.block_on(MockServer::start());
    let (body, pixels) = png();
    runtime.block_on(
        Mock::given(method("GET"))
            .and(path(IMAGE_PATH))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/png")
                    .set_body_bytes(body),
            )
            .mount(&storage),
    );
    let url = format!("{}{IMAGE_PATH}", storage.uri());
    host.describe(&format!("The new screen:\n\n<img alt=\"Screenshot\" src=\"{url}\">"));
    host.render_images_as(TOKEN, &url);

    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the image", |harness| {
        harness.query_by_role_and_label(Role::Image, "Image Screenshot").is_some()
    });

    harness
        .secondary_click_at(harness.get_by_role_and_label(Role::Image, "Image Screenshot").rect());
    press(&mut harness, "Copy image");
    wait_until(&mut harness, "the confirmation", |harness| {
        harness.query_by_label_contains(COPIED).is_some()
    });
    let copied = harness.state().services.image_clipboard.fake.copied();
    assert_eq!(copied.len(), 1, "one copy");
    assert_eq!((copied[0].width, copied[0].height), (40, 20));
    assert_eq!(copied[0].rgba, pixels, "every pixel, in place");

    harness
        .secondary_click_at(harness.get_by_role_and_label(Role::Image, "Image Screenshot").rect());
    press(&mut harness, "View image");
    wait_until(&mut harness, "the lightbox", |harness| {
        harness.query_by_label("Close (Esc)").is_some()
    });
    harness.event(egui::Event::Copy);
    wait_until(&mut harness, "the second copy", |harness| {
        harness.state().services.image_clipboard.fake.copied().len() == 2
    });
    assert_eq!(harness.state().services.image_clipboard.fake.copied()[1].rgba, pixels);
}
