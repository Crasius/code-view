//! Images in rendered Markdown, with a scripted [`ImageSource`]: the room they reserve, loading
//! only near the view, the view keeping its place when an image above it arrives, the failure
//! card's actions, the lightbox, "Copy image", animations repainting only while on screen, and
//! videos.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cv_base::theme;
use cv_media::image_clipboard::{self, Original};
use cv_media::image_state::{ImageFailure, ImageState, Remedy};
use cv_media::picture::Picture;
use cv_media::video;
use cv_testkit::Clicks;
use egui::accesskit::Role;
use egui::{Context, TextureHandle, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::test_document::{self, lines};
use super::{ImageRequest, ImageSource, RenderOptions};

#[derive(Default)]
struct Fake {
    states: Mutex<HashMap<String, ImageState>>,
    wanted: Mutex<Vec<String>>,
    retried: Mutex<Vec<String>>,
    picture: Mutex<Option<Picture>>,
    fixes: Mutex<usize>,
}

impl Fake {
    fn set(&self, src: &str, state: ImageState) {
        self.states.lock().unwrap().insert(src.to_owned(), state);
    }

    fn ready(&self, ctx: &Context, src: &str, size: egui::Vec2) {
        let texture = texture(ctx, "fake");
        self.set(
            src,
            ImageState::Ready { texture: texture.id(), size, next_frame: None, clip: None },
        );
        let original = Original::new(Arc::from(BODY), Some("image/png"));
        *self.picture.lock().unwrap() = Some(Picture::still(texture).with_original(Some(original)));
    }

    fn wanted(&self) -> Vec<String> {
        self.wanted.lock().unwrap().clone()
    }
}

impl ImageSource for Fake {
    fn state(&self, _ctx: &Context, image: &ImageRequest<'_>, want: bool) -> ImageState {
        if want {
            let mut wanted = self.wanted.lock().unwrap();
            if !wanted.iter().any(|src| src == image.src) {
                wanted.push(image.src.to_owned());
            }
        }
        self.states
            .lock()
            .unwrap()
            .get(image.src)
            .cloned()
            .unwrap_or(ImageState::Waiting { size: None })
    }

    fn retry(&self, image: &ImageRequest<'_>) {
        self.retried.lock().unwrap().push(image.src.to_owned());
    }

    fn web_url(&self, image: &ImageRequest<'_>) -> Option<String> {
        Some(format!("https://x.test/{}", image.src))
    }

    fn picture(&self, _image: &ImageRequest<'_>) -> Option<Picture> {
        self.picture.lock().unwrap().clone()
    }

    fn fix_access(&self, _ctx: &Context) {
        *self.fixes.lock().unwrap() += 1;
    }
}

/// A 4x4 red texture named `name`, standing in for a decoded image or frame.
fn texture(ctx: &Context, name: &str) -> TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::filled([4, 4], egui::Color32::RED),
        egui::TextureOptions::LINEAR,
    )
}

/// A failure that a retry may fix, needing neither a token nor ffmpeg.
fn failure(reason: &str, place: &str) -> ImageFailure {
    ImageFailure::new(reason.into(), place.into(), true)
}

/// The body the fake's ready images were decoded from.
const BODY: &[u8] = b"the encoded image";

struct Scene {
    source: Arc<Fake>,
    text: String,
    /// Scrolls to this offset in the first frame.
    offset: Option<f32>,
}

fn harness(text: String, offset: Option<f32>, size: [f32; 2]) -> Harness<'static, Scene> {
    let scene = Scene { source: Arc::new(Fake::default()), text, offset };
    Harness::builder().with_size(size).with_context_setup(test_document::light).build_ui_state(
        |ui, scene: &mut Scene| {
            super::publish_images(ui.ctx(), Arc::<Fake>::downgrade(&scene.source));
            let options = RenderOptions::default();
            test_document::scrolled(ui, &mut scene.offset, &scene.text, options);
        },
        scene,
    )
}

#[test]
fn attributes_and_placeholders_reserve_the_room_so_nothing_moves_on_arrival() {
    let text =
        format!("Intro\n\n<img src=\"sized.png\" width=\"200\" height=\"100\">\n\n{}", lines(3));
    let mut harness = harness(text, None, [600.0, 600.0]);
    harness.run_steps(3);
    let below =
        |harness: &Harness<'_, Scene>| harness.get_by_label("Line 0 of the document.").rect();
    let before = below(&harness);
    let placeholder = harness.get_by_role_and_label(Role::Image, "Loading image sized.png").rect();
    assert_eq!(placeholder.size(), vec2(200.0, 100.0), "the attributes' size from the start");
    assert_eq!(harness.state().source.wanted(), ["sized.png"], "an image in view is wanted");

    let ctx = harness.ctx.clone();
    harness.state().source.ready(&ctx, "sized.png", vec2(800.0, 300.0));
    harness.run_steps(3);
    assert_eq!(below(&harness), before, "the text below stays where it was");
    let shown = harness.get_by_role_and_label(Role::Image, "Image sized.png").rect();
    assert_eq!(shown.size(), vec2(200.0, 100.0));
}

#[test]
fn only_images_near_the_view_load() {
    let text = format!("![top](top.png)\n\n{}\n\n![bottom](bottom.png)", lines(80));
    let mut harness = harness(text, None, [600.0, 400.0]);
    harness.run_steps(4);
    assert_eq!(harness.state().source.wanted(), ["top.png"], "the image far below the view waits");
}

#[test]
fn an_image_arriving_above_the_view_keeps_the_text_in_place() {
    // The placeholder (160 high) ends just above the view: laid out, not visible.
    let text = format!("![shot](shot.png)\n\n{}", lines(40));
    let mut harness = harness(text, Some(260.0), [600.0, 300.0]);
    harness.run_steps(4);
    let line = |harness: &Harness<'_, Scene>| {
        harness.get_by_label("Line 12 of the document.").rect().top()
    };
    let before = line(&harness);
    let ctx = harness.ctx.clone();
    harness.state().source.ready(&ctx, "shot.png", vec2(300.0, 40.0));
    harness.run_steps(4);
    assert!((line(&harness) - before).abs() < 0.5, "moved from {before} to {}", line(&harness));
}

#[test]
fn a_failed_image_says_why_and_offers_retry_and_the_browser() {
    let mut harness = harness("![Build chart](chart.png)".to_owned(), None, [600.0, 300.0]);
    let failure = failure("Not found (404)", "x.test");
    harness.state().source.set("chart.png", ImageState::Failed(Arc::new(failure)));
    harness.run_steps(3);
    harness.get_by_label("Build chart");
    harness.get_by_label("Not found (404) · x.test");
    harness.get_by_role_and_label(Role::Button, "Open in browser");
    assert!(harness.query_by_label("Fix token").is_none(), "a missing image is no token problem");
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Retry").rect());
    harness.run_steps(2);
    assert_eq!(*harness.state().source.retried.lock().unwrap(), ["chart.png"]);
}

#[test]
fn a_private_image_offers_the_token_fix_on_a_card_of_the_usual_size() {
    let mut harness = harness("![Screenshot](shot.png)\n\nBelow".to_owned(), None, [600.0, 300.0]);
    let failure = ImageFailure {
        remedy: Remedy::SignIn,
        ..failure("Sign-in needed: this image is private", "github.com")
    };
    harness.state().source.set("shot.png", ImageState::Failed(Arc::new(failure)));
    harness.run_steps(3);
    harness.get_by_label("Sign-in needed: this image is private · github.com");
    harness.get_by_role_and_label(Role::Button, "Open in browser");
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Fix token").rect());
    harness.run_steps(2);
    assert_eq!(*harness.state().source.fixes.lock().unwrap(), 1);
    let card = harness
        .get_by_role_and_label(
            Role::Image,
            "Image Screenshot not shown: Sign-in needed: this image is private",
        )
        .rect();
    assert_eq!(
        card.height(),
        super::images::CARD_HEIGHT,
        "the extra action does not grow the card"
    );
}

#[test]
fn without_a_source_images_show_a_card_not_a_hole() {
    let mut harness = Harness::builder()
        .with_size([600.0, 300.0])
        .with_context_setup(theme::fonts::install)
        .build_ui(|ui| {
            super::render(ui, "doc", "![diagram](https://x.test/d.png)", RenderOptions::default());
        });
    harness.run_steps(2);
    harness.get_by_label("Images load on pull request pages · x.test");
}

#[test]
fn clicking_an_image_opens_the_lightbox_and_escape_closes_it() {
    let mut harness = harness("![shot](shot.png)".to_owned(), None, [800.0, 600.0]);
    let ctx = harness.ctx.clone();
    harness.state().source.ready(&ctx, "shot.png", vec2(300.0, 200.0));
    harness.run_steps(3);
    harness.click_at(harness.get_by_role_and_label(Role::Image, "Image shot").rect());
    harness.run_steps(3);
    harness.get_by_role_and_label(Role::Button, "Copy link");
    harness.get_by_role_and_label(Role::Button, "Close (Esc)");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(harness.query_by_label("Close (Esc)").is_none(), "Esc closes the lightbox");
}

/// An animation repaints when its next frame is due while it is on screen, and costs nothing
/// once scrolled out of view.
#[test]
fn animations_repaint_only_while_on_screen() {
    let text = format!("![clip](clip.gif)\n\n{}", lines(80));
    let mut shown = harness(text.clone(), None, [600.0, 300.0]);
    let ctx = shown.ctx.clone();
    let frame = texture(&ctx, "frame");
    let next_frame = Some(std::time::Duration::from_millis(80));
    let size = vec2(40.0, 40.0);
    let state = ImageState::Ready { texture: frame.id(), size, next_frame, clip: None };
    shown.state().source.set("clip.gif", state.clone());
    shown.run_steps(3);
    let delay = |harness: &Harness<'_, Scene>| {
        harness.output().viewport_output[&egui::ViewportId::ROOT].repaint_delay
    };
    assert!(
        delay(&shown) <= std::time::Duration::from_millis(80),
        "the next frame is drawn on time"
    );

    let mut away = harness(text, Some(2_000.0), [600.0, 300.0]);
    away.state().source.set("clip.gif", state);
    away.run_steps(4);
    assert_eq!(delay(&away), std::time::Duration::MAX, "nothing on screen asks to repaint");
}

/// A video shows its first frame with a play mark and a bar to play it and move along it; a click
/// plays it instead of opening the lightbox.
#[test]
fn a_video_shows_its_controls_and_plays_in_place() {
    let mut harness = harness("![demo](demo.webm)".to_owned(), None, [800.0, 600.0]);
    let ctx = harness.ctx.clone();
    let poster = texture(&ctx, "poster");
    let clip = Some(video::Clip::without_ffmpeg([320, 180], 12.0));
    let state = ImageState::Ready {
        texture: poster.id(),
        size: vec2(320.0, 180.0),
        next_frame: None,
        clip,
    };
    harness.state().source.set("demo.webm", state);
    harness.run_steps(3);
    let video = harness.get_by_role_and_label(Role::Image, "Video demo").rect();
    assert_eq!(video.size(), vec2(320.0, 180.0));
    harness.get_by_role_and_label(Role::Button, "Play");
    harness.get_by_label("Position, 0:00");
    harness.click_at(harness.get_by_role_and_label(Role::Image, "Video demo").rect());
    harness.run_steps(3);
    assert!(harness.query_by_label("Close (Esc)").is_none(), "a video opens no lightbox");
}

/// Without ffmpeg a video says so and hands over the command that installs it.
#[test]
fn a_video_without_ffmpeg_offers_the_install_command() {
    let mut harness = harness("![demo](demo.webm)".to_owned(), None, [600.0, 300.0]);
    let failure =
        ImageFailure { remedy: Remedy::InstallFfmpeg, ..failure(video::MISSING, "github.com") };
    harness.state().source.set("demo.webm", ImageState::Failed(Arc::new(failure)));
    harness.run_steps(3);
    harness.get_by_role_and_label(Role::Button, "Retry");
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Copy install command").rect());
    harness.run_steps(1);
    let copied = harness.output().platform_output.commands.iter().any(|command| {
        matches!(command, egui::OutputCommand::CopyText(text)
            if text == video::install_command())
    });
    assert!(copied, "the install command is on the clipboard");
}

/// "Copy image" sits right after "View image" in a ready image's menu and asks for the image's
/// body; an image still loading offers no copy.
#[test]
fn a_ready_image_copies_from_its_context_menu_and_a_loading_one_does_not() {
    let mut harness =
        harness("![shot](shot.png)\n\n![later](later.png)".to_owned(), None, [800.0, 600.0]);
    let ctx = harness.ctx.clone();
    harness.state().source.ready(&ctx, "shot.png", vec2(300.0, 200.0));
    harness.run_steps(3);

    harness.secondary_click_at(
        harness.get_by_role_and_label(Role::Image, "Loading image later").rect(),
    );
    harness.run_steps(2);
    assert!(harness.query_by_label("Copy image").is_none(), "nothing to copy while loading");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);

    harness.secondary_click_at(harness.get_by_role_and_label(Role::Image, "Image shot").rect());
    harness.run_steps(2);
    let top = |label: &str| harness.get_by_label(label).rect().top();
    assert!(top("View image") < top("Copy image"), "after View image");
    assert!(top("Copy image") < top("Copy image link"), "before the link entries");
    harness.click_at(harness.get_by_label("Copy image").rect());
    harness.run_steps(2);
    assert_eq!(image_clipboard::requested(&ctx), [Arc::from(BODY)], "the image's own body");
    assert!(harness.query_by_label("Copy image").is_none(), "the menu closed");
}

/// In the lightbox, Ctrl+C (a copy event) and its button copy the image shown.
#[test]
fn the_lightbox_copies_its_image_on_ctrl_c() {
    let mut harness = harness("![shot](shot.png)".to_owned(), None, [800.0, 600.0]);
    let ctx = harness.ctx.clone();
    harness.state().source.ready(&ctx, "shot.png", vec2(300.0, 200.0));
    harness.run_steps(3);
    harness.click_at(harness.get_by_role_and_label(Role::Image, "Image shot").rect());
    harness.run_steps(3);
    harness.event(egui::Event::Copy);
    harness.run_steps(1);
    assert_eq!(image_clipboard::requested(&ctx), [Arc::from(BODY)]);
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Copy image").rect());
    harness.run_steps(1);
    assert_eq!(image_clipboard::requested(&ctx).len(), 2, "the button copies too");
}
