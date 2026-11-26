//! "Copy image" off the UI thread: a request reaches the fake clipboard as full pixels, and a
//! failure becomes an error toast that says why.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cv_runtime::background::Background;
use cv_runtime::toasts::Toasts;
use egui::Context;

use super::{ImageClipboard, Original, request, requested};

fn png(width: u32, height: u32) -> Arc<[u8]> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(width, height, image::Rgba([1, 2, 3, 255]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode png");
    bytes.into_inner().into()
}

/// Runs `update` until `done`, as the frames after a copy would.
fn frames_until(
    clipboard: &ImageClipboard,
    ctx: &Context,
    toasts: &mut Toasts,
    done: impl Fn(&ImageClipboard, &Toasts) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(clipboard, toasts) {
        assert!(Instant::now() < deadline, "the copy did not end");
        clipboard.update(ctx, toasts);
        cv_testkit::let_threads_run(Duration::from_millis(5));
    }
}

#[test]
fn a_requested_image_lands_on_the_clipboard_as_its_pixels() {
    let ctx = Context::default();
    let background = Background::new(ctx.clone());
    let clipboard = ImageClipboard::new(background.spawner());
    let mut toasts = Toasts::default();
    let body = png(6, 4);
    request(&ctx, Original::new(body.clone(), Some("image/png")), None);
    assert_eq!(requested(&ctx), [body], "asked for, not copied on the UI thread");

    frames_until(&clipboard, &ctx, &mut toasts, |clipboard, _| !clipboard.fake.copied().is_empty());
    assert!(requested(&ctx).is_empty(), "started once");
    let copied = clipboard.fake.copied();
    assert_eq!(copied.len(), 1);
    assert_eq!((copied[0].width, copied[0].height), (6, 4));
    assert_eq!(copied[0].rgba, [1, 2, 3, 255].repeat(24));
    assert_eq!(toasts.error_texts(), [] as [&str; 0], "no failure");
}

#[test]
fn a_failed_copy_says_why() {
    let ctx = Context::default();
    let background = Background::new(ctx.clone());
    let clipboard = ImageClipboard::new(background.spawner());
    let mut toasts = Toasts::default();
    clipboard.fake.fail_with("Another app is using the clipboard");
    request(&ctx, Original::new(png(2, 2), None), Some("https://x.test/a.png".to_owned()));
    frames_until(&clipboard, &ctx, &mut toasts, |_, toasts| !toasts.error_texts().is_empty());
    assert_eq!(
        toasts.error_texts(),
        ["Could not copy the image. Another app is using the clipboard."]
    );

    let damaged = Original::new(Arc::from(&b"not an image"[..]), None);
    request(&ctx, damaged, None);
    frames_until(&clipboard, &ctx, &mut toasts, |_, toasts| toasts.error_texts().len() == 2);
    assert_eq!(toasts.error_texts()[1], "Could not copy the image. Unsupported image format.");
}
