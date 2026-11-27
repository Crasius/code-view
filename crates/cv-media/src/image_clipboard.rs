//! "Copy image": an image's full-resolution pixels on the system clipboard, as an image rather than
//! a link. Textures are scaled down to [`images::MAX_TEXTURE_EDGE`] and their pixels leave the
//! heap, so a picture keeps the body it was decoded from ([`Original`]), and a copy decodes that
//! again, unscaled ([`images::full_pixels`]): a raster image's own pixels (an animation's first
//! frame), an SVG drawn at the screen's pixel density.
//!
//! Widgets ask with [`request`]. Once per frame [`ImageClipboard::update`] starts each request on
//! the background runtime, so the UI thread never waits for decoding or the clipboard, and turns
//! each result into a toast: "Image copied", or why not with Retry and the image's link. Test
//! builds write to a [`FakeClipboard`], never to the system clipboard.

#[cfg(not(any(test, feature = "test-support")))]
use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};

use cv_base::theme::icons;
use cv_runtime::background::Spawner;
use cv_runtime::toasts::{ToastAction, Toasts, WaysOut};
use egui::{Context, Id};

use crate::images;

/// The toast after a copy that worked.
pub const COPIED: &str = "Image copied";

const REQUESTS_ID: &str = "review.image_clipboard.requests";

/// The encoded body a picture was decoded from, and its content type when the host sent one.
#[derive(Clone)]
pub struct Original {
    pub bytes: Arc<[u8]>,
    pub content_type: Option<Arc<str>>,
}

impl Original {
    pub fn new(bytes: Arc<[u8]>, content_type: Option<&str>) -> Self {
        Self { bytes, content_type: content_type.map(Arc::from) }
    }
}

/// Unpremultiplied RGBA pixels, row by row: what the clipboard takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixels {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Where a copied image goes: the system clipboard, or a fake in tests.
pub(crate) trait ImageSink: Send + Sync {
    /// Puts `pixels` on the clipboard; else why not, for the toast.
    fn set_image(&self, pixels: &Pixels) -> Result<(), String>;
}

/// The system clipboard. One handle for the app's life: on Linux a copied image is served only
/// while the handle that copied it lives.
#[cfg(not(any(test, feature = "test-support")))]
#[derive(Default)]
struct SystemClipboard(Mutex<Option<arboard::Clipboard>>);

#[cfg(not(any(test, feature = "test-support")))]
impl ImageSink for SystemClipboard {
    fn set_image(&self, pixels: &Pixels) -> Result<(), String> {
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let clipboard = match &mut *held {
            Some(clipboard) => clipboard,
            empty => empty.insert(arboard::Clipboard::new().map_err(|error| reason(&error))?),
        };
        let image = arboard::ImageData {
            width: pixels.width,
            height: pixels.height,
            bytes: Cow::Borrowed(&pixels.rgba),
        };
        clipboard.set_image(image).map_err(|error| reason(&error))
    }
}

#[cfg(not(any(test, feature = "test-support")))]
fn reason(error: &arboard::Error) -> String {
    match error {
        arboard::Error::ClipboardOccupied => "Another app is using the clipboard".to_owned(),
        arboard::Error::ClipboardNotSupported => "The system clipboard takes no images".to_owned(),
        arboard::Error::ConversionFailure => {
            "The image could not be converted for the clipboard".to_owned()
        }
        other => other.to_string(),
    }
}

/// The clipboard of test builds: it keeps what was copied, or fails with a set reason.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct FakeClipboard {
    copied: Mutex<Vec<Pixels>>,
    failure: Mutex<Option<String>>,
}

#[cfg(any(test, feature = "test-support"))]
impl FakeClipboard {
    pub fn copied(&self) -> Vec<Pixels> {
        self.copied.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Every copy from now on fails with `reason`.
    pub(crate) fn fail_with(&self, reason: &str) {
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(reason.to_owned());
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ImageSink for FakeClipboard {
    fn set_image(&self, pixels: &Pixels) -> Result<(), String> {
        if let Some(reason) = self.failure.lock().unwrap_or_else(PoisonError::into_inner).clone() {
            return Err(reason);
        }
        self.copied.lock().unwrap_or_else(PoisonError::into_inner).push(pixels.clone());
        Ok(())
    }
}

/// One "Copy image": the body, the screen's pixel density an SVG is drawn at, and the image's
/// address for the ways out of a failure.
#[derive(Clone)]
struct Request {
    original: Original,
    pixels_per_point: f32,
    web_url: Option<String>,
}

/// Asks for `original` on the clipboard; the next [`ImageClipboard::update`] starts the copy.
/// `web_url` is the image's address, offered when the copy fails.
pub fn request(ctx: &Context, original: Original, web_url: Option<String>) {
    let request = Request { original, pixels_per_point: ctx.pixels_per_point(), web_url };
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<Request>>(Id::unique(REQUESTS_ID)).push(request);
    });
    ctx.request_repaint();
}

/// The bodies asked for and not started yet (test builds only).
#[cfg(any(test, feature = "test-support"))]
pub fn requested(ctx: &Context) -> Vec<Arc<[u8]>> {
    ctx.data(|data| data.get_temp::<Vec<Request>>(Id::unique(REQUESTS_ID)))
        .unwrap_or_default()
        .into_iter()
        .map(|request| request.original.bytes)
        .collect()
}

struct Outcome {
    request: Request,
    result: Result<(), String>,
}

/// Runs the copies widgets ask for and reports how they went.
pub struct ImageClipboard {
    spawner: Spawner,
    sink: Arc<dyn ImageSink>,
    outcomes: Arc<Mutex<Vec<Outcome>>>,
    /// What test builds copy to.
    #[cfg(any(test, feature = "test-support"))]
    pub fake: Arc<FakeClipboard>,
}

impl ImageClipboard {
    /// Copies to the system clipboard; test builds to a [`FakeClipboard`].
    pub fn new(spawner: Spawner) -> Self {
        #[cfg(any(test, feature = "test-support"))]
        {
            let fake = Arc::new(FakeClipboard::default());
            Self { spawner, sink: fake.clone(), outcomes: Arc::default(), fake }
        }
        #[cfg(not(any(test, feature = "test-support")))]
        {
            Self { spawner, sink: Arc::new(SystemClipboard::default()), outcomes: Arc::default() }
        }
    }

    /// Starts the copies asked for since the last frame and toasts the ones that ended. Call once
    /// per frame.
    pub fn update(&self, ctx: &Context, toasts: &mut Toasts) {
        let requests = cv_base::mailbox::take::<Vec<Request>>(ctx, Id::unique(REQUESTS_ID))
            .unwrap_or_default();
        for request in requests {
            self.start(request);
        }
        let ended =
            std::mem::take(&mut *self.outcomes.lock().unwrap_or_else(PoisonError::into_inner));
        for Outcome { request, result } in ended {
            match result {
                Ok(()) => toasts.success(COPIED),
                Err(reason) => {
                    toasts.error(format!("Could not copy the image. {reason}."), ways_out(request));
                }
            }
        }
    }

    /// Decodes and copies off the UI thread; the task's end wakes the UI, which toasts it.
    fn start(&self, request: Request) {
        let sink = self.sink.clone();
        let outcomes = self.outcomes.clone();
        self.spawner.spawn(async move {
            let job = request.clone();
            let result = tokio::task::spawn_blocking(move || copy(sink.as_ref(), &job))
                .await
                .unwrap_or_else(|_| Err("The copy stopped".to_owned()));
            outcomes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Outcome { request, result });
        });
    }
}

fn copy(sink: &dyn ImageSink, request: &Request) -> Result<(), String> {
    let pixels = images::full_pixels(&request.original, request.pixels_per_point)?;
    sink.set_image(&pixels)
}

/// Retry, and the image's link to copy or open where the app has its address.
fn ways_out(request: Request) -> WaysOut {
    let web_url = request.web_url.clone();
    let retry = ToastAction::new(icons::ARROW_CLOCKWISE, "Retry", move |ctx| {
        let Request { original, web_url, .. } = request;
        self::request(ctx, original, web_url);
    });
    WaysOut::new(retry)
        .and_some(web_url.clone().map(|url| ToastAction::copy("Copy image link", url)))
        .and_some(web_url.map(ToastAction::open_in_browser))
}

#[cfg(test)]
mod tests;
