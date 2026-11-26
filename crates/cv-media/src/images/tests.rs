//! The image store: loads only what is wanted, once, a few at a time and never while the window
//! is inactive; failures say why; textures drawn longest ago leave memory first; a stale copy on
//! disk shows at once and is asked about behind it; animations keep every frame within their
//! budget; "Copy image" gets the full-resolution pixels the textures were scaled down from.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use cv_runtime::background::Background;
use cv_runtime::cache::memory::{self, Holder, TextureMemory};
use cv_testkit::ScratchDir;
use egui::{ColorImage, Context, TextureOptions, vec2};
use review_core::http::image::Validators;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::decode::{decode, decode_raster, full_pixels, spawn_frames};
use super::{
    Entry, Fetch, ImageStore, Inner, MAX_TEXTURE_EDGE, Natural, Phase, Target, disk, evict, upload,
};
use crate::image_state::ImageState;
use crate::picture::Picture;

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(width, height)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode png");
    bytes.into_inner()
}

/// A GIF of `frames` frames of `width` × `height`, each shown for 50 ms.
fn gif(width: u32, height: u32, frames: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
    for index in 0..frames {
        let buffer = image::RgbaImage::from_pixel(width, height, image::Rgba([index, 0, 0, 255]));
        let delay = image::Delay::from_numer_denom_ms(50, 1);
        encoder.encode_frame(image::Frame::from_parts(buffer, 0, 0, delay)).expect("encode frame");
    }
    drop(encoder);
    bytes
}

struct Fixture {
    runtime: tokio::runtime::Runtime,
    server: MockServer,
    ctx: Context,
    store: ImageStore,
    _background: Background,
}

impl Fixture {
    fn new(delay: Duration) -> Self {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let server = runtime.block_on(MockServer::start());
        runtime.block_on(
            Mock::given(method("GET"))
                .and(path("/missing.png"))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server),
        );
        runtime.block_on(
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "image/png")
                        .set_body_bytes(png(40, 20))
                        .set_delay(delay),
                )
                .mount(&server),
        );
        let ctx = Context::default();
        let background = Background::new(ctx.clone());
        let store = ImageStore::new(background.spawner());
        Self { runtime, server, ctx, store, _background: background }
    }

    fn target(&self, name: &str) -> Target {
        let url = format!("{}/{name}", self.server.uri());
        Target {
            key: egui::epaint::util::hash(&url),
            fetch: Fetch::Remote { url, owner: None, disk: None },
            place: "127.0.0.1".into(),
            video: None,
        }
    }

    /// A remote image kept in the file `disk`.
    fn kept_target(&self, name: &str, disk: &Path) -> Target {
        let target = self.target(name);
        let Fetch::Remote { url, owner, .. } = target.fetch else { unreachable!("remote") };
        Target { fetch: Fetch::Remote { url, owner, disk: Some(disk.to_owned()) }, ..target }
    }

    fn requests_for(&self, name: &str) -> usize {
        let requests = self.runtime.block_on(self.server.received_requests()).unwrap_or_default();
        requests.iter().filter(|request| request.url.path() == format!("/{name}")).count()
    }

    fn requests(&self) -> usize {
        self.runtime.block_on(self.server.received_requests()).map_or(0, |requests| requests.len())
    }

    /// The state once the load settled (ready or failed).
    fn settled(&self, target: &Target) -> ImageState {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let state = self.store.state(&self.ctx, target, false);
            if !matches!(state, ImageState::Waiting { .. }) || Instant::now() > deadline {
                return state;
            }
            cv_testkit::let_threads_run(Duration::from_millis(10));
        }
    }
}

/// Waits until `done` holds, at most 20 seconds.
fn until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() && Instant::now() < deadline {
        cv_testkit::let_threads_run(Duration::from_millis(10));
    }
}

/// One pass of `ctx` with the window focused or not.
fn pass(ctx: &Context, is_focused: bool, run: impl FnMut(&Context)) {
    let mut input = egui::RawInput::default();
    input.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo { focused: Some(is_focused), ..Default::default() },
    );
    // No renderer takes the textures here.
    ctx.run_pass(input, run).textures_delta.clear();
}

#[test]
fn only_wanted_images_load_once_and_none_while_the_window_is_inactive() {
    let fixture = Fixture::new(Duration::ZERO);
    let target = fixture.target("shot.png");
    for _ in 0..3 {
        assert!(matches!(
            fixture.store.state(&fixture.ctx, &target, false),
            ImageState::Waiting { size: None }
        ));
    }
    pass(&fixture.ctx, false, |ctx| {
        fixture.store.state(ctx, &target, true);
    });
    // A load turns the entry from Idle to Loading in the same call, and never back to Idle.
    let never_started = matches!(fixture.store.lock().entries[&target.key].phase, Phase::Idle);
    assert!(never_started, "images off screen or behind an inactive window do not load");

    pass(&fixture.ctx, true, |ctx| {
        fixture.store.state(ctx, &target, true);
        fixture.store.state(ctx, &target, true);
    });
    let ImageState::Ready { size, .. } = fixture.settled(&target) else {
        panic!("the image loads once wanted")
    };
    assert_eq!(size, vec2(40.0, 20.0), "one image pixel per point at zoom 1");
    pass(&fixture.ctx, true, |ctx| {
        fixture.store.state(ctx, &target, true);
    });
    assert_eq!(
        fixture.requests(),
        1,
        "asked twice while loading and again when loaded: one request"
    );
}

#[test]
fn at_most_four_images_load_at_once() {
    let fixture = Fixture::new(Duration::from_millis(400));
    let targets: Vec<Target> =
        (0..7).map(|index| fixture.target(&format!("{index}.png"))).collect();
    pass(&fixture.ctx, true, |ctx| {
        for target in &targets {
            fixture.store.state(ctx, target, true);
        }
    });
    assert_eq!(fixture.store.in_flight(), super::MAX_IN_FLIGHT);
    let first: Vec<ImageState> =
        targets.iter().map(|target| fixture.settled(target)).take(4).collect();
    assert!(first.iter().all(|state| matches!(state, ImageState::Ready { .. })));
    assert!(
        matches!(fixture.store.state(&fixture.ctx, &targets[6], false), ImageState::Waiting { .. }),
        "the rest waits to be wanted again"
    );
}

#[test]
fn failures_say_why_and_retry_loads_again() {
    let fixture = Fixture::new(Duration::ZERO);
    let missing = fixture.target("missing.png");
    pass(&fixture.ctx, true, |ctx| {
        fixture.store.state(ctx, &missing, true);
    });
    let ImageState::Failed(failure) = fixture.settled(&missing) else { panic!("a 404 fails") };
    assert_eq!((failure.reason.as_str(), failure.can_retry), ("Not found (404)", false));
    assert_eq!(failure.place, "127.0.0.1");

    let blocked = Target {
        key: 7,
        fetch: Fetch::Blocked("Local files are not loaded".into()),
        place: String::new(),
        video: None,
    };
    let ImageState::Failed(failure) = fixture.store.state(&fixture.ctx, &blocked, true) else {
        panic!("blocked at once")
    };
    assert_eq!(failure.reason, "Local files are not loaded");

    fixture.store.retry(missing.key);
    assert!(
        matches!(fixture.store.state(&fixture.ctx, &missing, false), ImageState::Waiting { .. }),
        "a retry forgets the failure"
    );
    let before = fixture.requests();
    pass(&fixture.ctx, true, |ctx| {
        fixture.store.state(ctx, &missing, true);
    });
    fixture.settled(&missing);
    assert_eq!(fixture.requests(), before + 1);
}

#[test]
fn textures_drawn_longest_ago_leave_memory_first_and_keep_their_size() {
    let ctx = Context::default();
    let texture = |name: &str| {
        ctx.load_texture(
            name,
            ColorImage::filled([8, 8], egui::Color32::WHITE),
            TextureOptions::LINEAR,
        )
    };
    let one = memory::texture_bytes(&texture("probe"));
    let mut inner = Inner {
        entries: HashMap::default(),
        in_flight: 0,
        textures: TextureMemory::new(Holder::Images, 2 * one),
        denied: HashMap::default(),
        ahead: HashMap::default(),
        reading_ahead: HashSet::default(),
    };
    for (key, used) in [(1, 5), (2, 3), (3, 9)] {
        let texture = texture(&key.to_string());
        inner.textures.add(&texture);
        let entry = Entry {
            natural: Some(Natural::Pixels(vec2(8.0, 8.0))),
            ..Entry::new(Phase::Ready(Picture::still(texture)), used, 0.0)
        };
        inner.entries.insert(key, entry);
    }
    evict(&mut inner, 9);
    let is_ready = |key: u64| matches!(inner.entries[&key].phase, Phase::Ready(_));
    assert_eq!(
        (is_ready(1), is_ready(2), is_ready(3)),
        (true, false, true),
        "the one drawn longest ago goes"
    );
    assert_eq!(
        inner.entries[&2].natural,
        Some(Natural::Pixels(vec2(8.0, 8.0))),
        "it comes back at the same size"
    );
    assert_eq!(inner.textures.held(), 2 * one);
}

/// A page left behind takes its images' memory along: a texture nobody drew for half a minute
/// goes, keeping its size, while one drawn meanwhile stays.
#[test]
fn textures_nobody_drew_lately_leave_memory_and_keep_their_size() {
    let fixture = Fixture::new(Duration::ZERO);
    let (left, shown) = (fixture.target("left.png"), fixture.target("shown.png"));
    let at = |seconds: f64| {
        let mut input = egui::RawInput { time: Some(seconds), ..Default::default() };
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo { focused: Some(true), ..Default::default() },
        );
        input
    };
    fixture
        .ctx
        .run_pass(at(1.0), |ctx| {
            fixture.store.state(ctx, &left, true);
            fixture.store.state(ctx, &shown, true);
        })
        .textures_delta
        .clear();
    assert!(matches!(fixture.settled(&left), ImageState::Ready { .. }));
    assert!(matches!(fixture.settled(&shown), ImageState::Ready { .. }));
    let held = fixture.store.lock().textures.held();
    let pass = |seconds: f64, draw: &Target| {
        fixture
            .ctx
            .run_pass(at(seconds), |ctx| {
                fixture.store.state(ctx, draw, false);
            })
            .textures_delta
            .clear();
    };
    pass(20.0, &shown);
    fixture.store.release_unseen(20.0);
    assert_eq!(fixture.store.lock().textures.held(), held, "both were drawn within 30 s");
    pass(1.0 + memory::UNSEEN_TEXTURE_SECONDS, &shown);
    fixture.store.release_unseen(1.0 + memory::UNSEEN_TEXTURE_SECONDS);
    assert_eq!(fixture.store.lock().textures.held(), held / 2, "the unseen one went");
    let inner = fixture.store.lock();
    assert!(matches!(inner.entries[&left.key].phase, Phase::Idle));
    assert_eq!(inner.entries[&left.key].natural, Some(Natural::Pixels(vec2(40.0, 20.0))));
    assert!(matches!(inner.entries[&shown.key].phase, Phase::Ready(_)));
}

#[test]
fn decodes_raster_and_svg_and_refuses_web_pages() {
    let decoded = decode(&png(3000, 1000), Some("image/png"), 2.0).expect("png");
    let (image, natural) = (&decoded.frames[0].0, decoded.natural);
    assert_eq!(image.size, [2048, 683], "scaled down to the texture limit");
    assert_eq!(
        natural.points(2.0),
        vec2(1500.0, 500.0),
        "laid out at its full size, one pixel per screen pixel"
    );

    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="90" height="20">
        <image href="file:///C:/Windows/win.ini" width="10" height="10"/><image xlink:href="https://x.test/a.png" width="10" height="10"/>
        <rect width="90" height="20" fill="green"/></svg>"#;
    let decoded = decode(svg, None, 2.0).expect("svg");
    assert_eq!(
        (decoded.frames[0].0.size, decoded.natural.points(1.0)),
        ([180, 40], vec2(90.0, 20.0)),
        "drawn at the screen's density, laid out at its own size"
    );

    let page = decode(b"<!DOCTYPE html><html><body>Sign in</body></html>", Some("text/html"), 1.0);
    assert_eq!(page.err().as_deref(), Some("The host sent a web page, not an image"));
    assert_eq!(
        decode(b"not an image", None, 1.0).err().as_deref(),
        Some("Unsupported image format")
    );
}

/// An image seen once stays on disk past its host's lifetime: the stale copy shows at once, a
/// `304` only renews it, and a changed body replaces file and texture.
#[test]
fn stale_copies_show_at_once_and_are_asked_about_behind_them() {
    // The changed body arrives late, so the stale copy is seen first however fast the host is.
    let fixture = Fixture::new(Duration::from_millis(500));
    let folder = ScratchDir::new("image-store-stale");
    let validators = Validators { etag: Some("\"v1\"".into()), last_modified: None };
    fixture.runtime.block_on(
        Mock::given(method("GET"))
            .and(path("/kept.png"))
            .and(header("If-None-Match", "\"v1\""))
            .respond_with(ResponseTemplate::new(304).insert_header("Cache-Control", "max-age=600"))
            .with_priority(1)
            .mount(&fixture.server),
    );
    let size_of = |target: &Target| match fixture.store.state(&fixture.ctx, target, false) {
        ImageState::Ready { size, .. } => Some(size),
        _ => None,
    };
    for (name, after) in [("kept.png", vec2(10.0, 10.0)), ("changed.png", vec2(40.0, 20.0))] {
        let file = folder.path().join(name);
        disk::write(&file, &png(10, 10), Some("image/png"), &validators, Some(Duration::ZERO));
        let target = fixture.kept_target(name, &file);
        pass(&fixture.ctx, true, |ctx| {
            fixture.store.state(ctx, &target, true);
        });
        let ImageState::Ready { size, .. } = fixture.settled(&target) else {
            panic!("{name}: the stale copy shows")
        };
        assert_eq!(size, vec2(10.0, 10.0), "{name}: the kept copy, not a download");
        until(|| disk::read(&file).is_some_and(|kept| kept.is_fresh));
        until(|| size_of(&target) == Some(after));
        assert_eq!(size_of(&target), Some(after), "{name}");
        assert_eq!(fixture.requests_for(name), 1, "{name}: one question to the host");
        let kept = disk::read(&file).expect("still kept");
        assert!(kept.is_fresh, "{name}: the answer renews the copy");
        assert_eq!(kept.bytes == png(10, 10), name == "kept.png", "{name}: the body on disk");
    }
}

#[test]
fn animated_gifs_keep_every_frame_and_its_delay() {
    let decoded = decode(&gif(30, 20, 3), Some("image/gif"), 1.0).expect("gif");
    let sizes: Vec<_> = decoded.frames.iter().map(|(image, _)| image.size).collect();
    assert_eq!(sizes, vec![[30, 20]; 3]);
    let delays: Vec<f64> = decoded.frames.iter().map(|(_, delay)| *delay).collect();
    assert_eq!(delays, vec![0.05; 3]);
    assert_eq!(decoded.natural, Natural::Pixels(vec2(30.0, 20.0)));
    let first_red = |index: usize| decoded.frames[index].0.pixels[0].r();
    assert_eq!((first_red(0), first_red(2)), (0, 2), "the frames in their order");

    let still = decode(&gif(30, 20, 1), Some("image/gif"), 1.0).expect("one frame");
    assert_eq!(still.frames.len(), 1, "a single-frame GIF is a still image");
}

/// Regression: an animation over its budget was scaled down until its frames fit, so a long
/// recording showed as a blur. Now it keeps its first frame at full size and streams the rest.
#[test]
fn animations_over_their_budget_stream_at_their_full_size() {
    // Ten frames of 100 × 100 take 400,000 bytes; the budget holds a quarter of that.
    let decoded = decode_raster(&gif(100, 100, 10).into(), 100_000).expect("gif");
    assert_eq!(decoded.frames.len(), 1, "only the first frame is kept");
    assert_eq!(decoded.frames[0].0.size, [100, 100], "at its full size");
    assert_eq!(decoded.natural, Natural::Pixels(vec2(100.0, 100.0)));
    let frames = spawn_frames(decoded.stream.expect("a stream plays it"));
    let shades: Vec<u8> = (0..12)
        .map(|_| {
            let (image, delay) = frames.recv_timeout(Duration::from_secs(5)).expect("a frame");
            assert_eq!((image.size, delay), ([100, 100], 0.05), "full size, own delay");
            image.pixels[0].r()
        })
        .collect();
    assert_eq!(shades, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 1], "in order, looping");
}

/// An animation plays from its first frame once it is asked for after a load, asks for a repaint
/// when the next frame is due, and holds still under reduced motion.
#[test]
fn animations_play_from_their_first_frame_and_hold_still_under_reduced_motion() {
    let fixture = Fixture::new(Duration::ZERO);
    fixture.runtime.block_on(
        Mock::given(method("GET"))
            .and(path("/clip.gif"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/gif")
                    .set_body_bytes(gif(8, 8, 2)),
            )
            .with_priority(1)
            .mount(&fixture.server),
    );
    let target = fixture.target("clip.gif");
    let at = |seconds: f64| {
        let mut input = egui::RawInput { time: Some(seconds), ..Default::default() };
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo { focused: Some(true), ..Default::default() },
        );
        input
    };
    let state_at = |seconds: f64, want: bool| {
        let mut state = None;
        fixture
            .ctx
            .run_pass(at(seconds), |ctx| state = Some(fixture.store.state(ctx, &target, want)))
            .textures_delta
            .clear();
        state.expect("one pass")
    };
    state_at(1.0, true);
    fixture.settled(&target);
    let frame = |state: ImageState| match state {
        ImageState::Ready { texture, next_frame, .. } => (texture, next_frame),
        _ => panic!("loaded"),
    };
    let (first, next) = frame(state_at(1.01, false));
    assert_eq!(
        next,
        Some(Duration::from_millis(40)),
        "plays from its first frame since it was first asked for"
    );
    let (second, _) = frame(state_at(1.06, false));
    assert_ne!(first, second, "the next frame after its delay");
    assert_eq!(fixture.store.picture(target.key).map(|picture| picture.textures().len()), Some(2));

    cv_base::theme::motion::set_reduced(&fixture.ctx, true);
    assert_eq!(
        frame(state_at(10.06, false)),
        (first, None),
        "reduced motion holds the first frame"
    );
}

/// A video downloads once into its own file, shows its first frame at its own size, and carries
/// the clip its player plays; the next load reads the file.
#[test]
#[ignore = "needs ffmpeg and ffprobe installed"]
fn a_video_downloads_into_its_file_and_shows_its_first_frame() {
    let fixture = Fixture::new(Duration::ZERO);
    let folder = ScratchDir::new("image-store-video");
    let tools = crate::video::Tools::locate().expect("ffmpeg installed");
    let encoded = folder.path().join("source.webm");
    let status = fixture.runtime.block_on(
        tokio::process::Command::new(tools.ffmpeg())
            .args(["-v", "error", "-f", "lavfi", "-i", "testsrc=size=64x48:rate=10", "-t", "1"])
            .args(["-c:v", "libvpx-vp9", "-y"])
            .arg(&encoded)
            .status(),
    );
    assert!(status.expect("ffmpeg runs").success());
    fixture.runtime.block_on(
        Mock::given(method("GET"))
            .and(path("/demo.webm"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "video/webm")
                    .set_body_bytes(std::fs::read(&encoded).expect("encoded")),
            )
            .with_priority(1)
            .mount(&fixture.server),
    );
    let file = folder.path().join("demo.video");
    let target = Target { video: Some(file.clone()), ..fixture.target("demo.webm") };
    pass(&fixture.ctx, true, |ctx| {
        fixture.store.state(ctx, &target, true);
    });
    let ImageState::Ready { size, clip, .. } = fixture.settled(&target) else {
        panic!("the video loads")
    };
    assert_eq!(size, vec2(64.0, 48.0));
    assert!(clip.is_some_and(|clip| clip.file == file), "it plays from its own file");
    assert!(file.is_file(), "the body is kept for ffmpeg");
    assert_eq!(fixture.requests_for("demo.webm"), 1);
}

/// A body larger than a texture may be is scaled down for the screen, but its picture keeps the
/// body, and the copy gets every pixel of it: full size, unpremultiplied RGBA.
#[test]
fn copied_pixels_are_the_full_resolution_image_not_the_texture() {
    let width = MAX_TEXTURE_EDGE + 452;
    let mut image = image::RgbaImage::from_pixel(width, 10, image::Rgba([10, 20, 30, 128]));
    image.put_pixel(width - 1, 9, image::Rgba([200, 100, 50, 255]));
    let mut body = std::io::Cursor::new(Vec::new());
    image.write_to(&mut body, image::ImageFormat::Png).expect("encode png");
    let decoded = decode(&body.into_inner(), Some("image/png"), 1.0).expect("png");
    let (picture, _) = upload(&Context::default(), "wide", decoded);
    assert_eq!(picture.size_vec2().x, MAX_TEXTURE_EDGE as f32, "the texture is scaled down");

    let pixels = full_pixels(picture.original().expect("the body is kept"), 1.0).expect("pixels");
    assert_eq!((pixels.width, pixels.height), (width as usize, 10));
    assert_eq!(pixels.rgba.len(), width as usize * 10 * 4, "four bytes per pixel");
    assert_eq!(pixels.rgba[..4], [10, 20, 30, 128], "unpremultiplied");
    assert_eq!(pixels.rgba[pixels.rgba.len() - 4..], [200, 100, 50, 255], "the last pixel too");
}

/// An animation copies its first frame; an SVG its drawing at the screen's pixel density.
#[test]
fn copied_animations_give_their_first_frame_and_svgs_their_drawing() {
    let decoded = decode(&gif(30, 20, 3), Some("image/gif"), 1.0).expect("gif");
    let pixels = full_pixels(decoded.original.as_ref().expect("kept"), 1.0).expect("pixels");
    assert_eq!((pixels.width, pixels.height, pixels.rgba.len()), (30, 20, 30 * 20 * 4));
    assert_eq!(pixels.rgba[..4], [0, 0, 0, 255], "the first frame's shade");

    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="10">
        <rect width="40" height="10" fill="#ff0000" fill-opacity="0.5"/></svg>"##;
    let decoded = decode(svg, None, 2.0).expect("svg");
    let pixels = full_pixels(decoded.original.as_ref().expect("kept"), 2.0).expect("pixels");
    assert_eq!((pixels.width, pixels.height), (80, 20), "drawn at two pixels per point");
    let [red, green, blue, alpha] = pixels.rgba[..4] else { unreachable!("four bytes") };
    assert_eq!((red, green, blue), (255, 0, 0), "unpremultiplied red");
    assert!(alpha.abs_diff(128) <= 1, "half transparent: {alpha}");
}

/// A body read ahead waits in memory and never reaches the disk; the load that shows it takes it
/// without another request and keeps it on disk from then on; one nobody took leaves after
/// [`super::AHEAD_SECONDS`].
#[test]
fn images_read_ahead_wait_in_memory_and_show_without_a_request() {
    let fixture = Fixture::new(Duration::ZERO);
    let scratch = ScratchDir::new("images-ahead");
    let shown = fixture.kept_target("shown.png", &scratch.path().join("shown"));
    let left = fixture.kept_target("left.png", &scratch.path().join("left"));
    fixture.store.fetch_ahead(0.0, vec![shown.clone(), left]);
    until(|| super::ahead::waiting(&fixture.store) == 2);
    assert_eq!((super::ahead::waiting(&fixture.store), fixture.requests()), (2, 2));
    assert!(!scratch.path().join("shown").exists(), "nothing read ahead reaches the disk");
    fixture.store.fetch_ahead(1.0, vec![shown.clone()]);
    assert_eq!(fixture.requests(), 2, "a body read ahead already is not read again");

    pass(&fixture.ctx, true, |ctx| {
        fixture.store.state(ctx, &shown, true);
    });
    assert!(matches!(fixture.settled(&shown), ImageState::Ready { .. }));
    assert_eq!(fixture.requests(), 2, "the shown image came from memory");
    assert!(scratch.path().join("shown").exists(), "and is kept on disk now");
    assert_eq!(super::ahead::waiting(&fixture.store), 1, "the one nobody showed still waits");

    fixture.store.release_unseen(super::AHEAD_SECONDS - 1.0);
    assert_eq!(super::ahead::waiting(&fixture.store), 1);
    fixture.store.release_unseen(super::AHEAD_SECONDS + 1.0);
    assert_eq!(super::ahead::waiting(&fixture.store), 0, "and leaves after its time");
}
