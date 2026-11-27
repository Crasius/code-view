//! Images in rendered Markdown: `![alt](src)` and HTML `<img>`. A screen publishes an
//! [`ImageSource`] each frame that knows where a source points (a provider attachment, a public URL,
//! a repository file at a commit) and loads it; without one, images show as a card saying so.
//!
//! An image takes its final size from the start where it can (`width`/`height` attributes, a size
//! known from an earlier load) and a placeholder box otherwise. It loads only once it is about to
//! scroll into view, fades in when it arrives, and when an image above the view changes its height
//! the view moves with it, so what the reader looks at stays put. Images never grow past one image
//! pixel per screen pixel, and shrink to the text width keeping their aspect ratio. A click opens
//! the image in a lightbox (any click or Esc closes it); a linked image opens its link instead.
//! "Copy image" in its context menu, and Ctrl+C (Cmd+C) or the button in the lightbox, put the
//! image on the clipboard at its full resolution ([`image_clipboard`]).
//! Animations play while they are on screen ([`Picture`]); videos play in place ([`video`]). A
//! failed image becomes a compact card with the reason and Retry / Open in browser, and Fix token
//! for a private image the connection does not reach.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use cv_base::theme::{self, Palette, icons, motion, style as tokens};
use cv_media::image_clipboard;
use cv_media::image_state::{ImageFailure, ImageState};
use cv_media::picture::Picture;
use cv_widgets::{buttons, glyph};
use egui::{
    Align, Align2, Color32, Context, CursorIcon, FontId, Id, Layout, Rect, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, pos2, vec2,
};

use super::parse::{ImageSpec, Length};
use super::render::links::is_url;
use super::video;
use super::{InApp, ResolvedLink, link_hint, request_in_app};

mod lightbox;

pub(super) use lightbox::open_lightbox;
pub use lightbox::show_lightbox;

/// The box an image of unknown size holds until it arrives (at zoom 1).
const PLACEHOLDER: Vec2 = vec2(480.0, 160.0);
/// The same for a status badge (shields.io and friends), which is much smaller.
const BADGE_PLACEHOLDER: Vec2 = vec2(88.0, 20.0);
pub(super) const CARD_HEIGHT: f32 = 56.0;
const CARD_MAX_WIDTH: f32 = 560.0;
/// Images this share of a screen above or below the view load ahead of being scrolled to.
pub(super) const PRELOAD_SCREENS: f32 = 0.5;

const SOURCE_ID: &str = "review.markdown.images";
const BASES_ID: &str = "review.markdown.image_bases";

/// Where the relative image paths of a repository file point: `path` at `commit`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileBase {
    pub commit: String,
    pub path: String,
}

/// A [`FileBase`] registered with [`file_base`], small enough for [`super::RenderOptions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageBase(u64);

#[derive(Clone, Default)]
struct Bases(HashMap<u64, Arc<FileBase>>);

/// The handle for relative images of `path` at `commit`.
pub fn file_base(ctx: &Context, commit: &str, path: &str) -> ImageBase {
    let key = egui::epaint::util::hash((commit, path));
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Bases>(Id::unique(BASES_ID)).0.entry(key).or_insert_with(
            || Arc::new(FileBase { commit: commit.to_owned(), path: path.to_owned() }),
        );
    });
    ImageBase(key)
}

fn base_of(ctx: &Context, base: ImageBase) -> Option<Arc<FileBase>> {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Bases>(Id::unique(BASES_ID)).0.get(&base.0).cloned()
    })
}

/// One image as written in a document, and the document's base for relative paths.
pub struct ImageRequest<'a> {
    pub src: &'a str,
    pub base: Option<&'a FileBase>,
    /// It is a video ([`ImageSpec::is_video`]).
    pub video: bool,
}

/// Loads and keeps the images of one screen's Markdown.
pub trait ImageSource: Send + Sync {
    /// The image's state. With `want` (it is about to be seen), one that is not loaded starts
    /// loading when the app may send requests (the window is active, the budget allows).
    fn state(&self, ctx: &Context, image: &ImageRequest<'_>, want: bool) -> ImageState;
    /// Forgets a failure; the next `state` with `want` loads again.
    fn retry(&self, image: &ImageRequest<'_>);
    /// The address "Open in browser" and "Copy link" use.
    fn web_url(&self, image: &ImageRequest<'_>) -> Option<String>;
    /// The loaded image, for the lightbox to keep while it shows it.
    fn picture(&self, image: &ImageRequest<'_>) -> Option<Picture>;
    /// Opens the setup of the connection the images load through, for a private image it does not
    /// reach ([`ImageFailure::needs_access`]).
    fn fix_access(&self, ctx: &Context);
}

/// Makes `source` load the images of every Markdown text drawn this frame. The context keeps only
/// a weak reference, the screen owns the source: a source reaches the context through its image
/// store's tasks, and a strong one would keep the context, and everything it holds, alive forever.
pub fn publish_images(ctx: &Context, source: Weak<dyn ImageSource>) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| data.insert_temp(Id::unique(SOURCE_ID), (pass, source)));
}

fn current_source(ctx: &Context) -> Option<Arc<dyn ImageSource>> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|data| data.get_temp::<(u64, Weak<dyn ImageSource>)>(Id::unique(SOURCE_ID)))
        .filter(|(published, _)| *published == pass)
        .and_then(|(_, source)| source.upgrade())
}

/// One image in a paragraph, as the renderer hands it over.
pub(super) struct Shown<'a> {
    pub id: Id,
    pub spec: &'a ImageSpec,
    /// The link around the image: its target as written and how it resolves.
    pub link: Option<(&'a str, &'a ResolvedLink)>,
    pub base: Option<ImageBase>,
    /// A change color around the image (the rendered diff's added or removed blocks).
    pub border: Option<Color32>,
    /// Who keeps the reader's place when the image changes height above the view.
    pub anchor: Anchor,
    /// The text width: images shrink to it.
    pub max_width: f32,
    pub scale: f32,
    pub palette: &'a Palette,
}

/// The size an image takes: its attributes, else its natural size, else a placeholder; never wider
/// than `max_width`, keeping the aspect ratio.
pub(super) fn display_size(
    spec: &ImageSpec,
    natural: Option<Vec2>,
    max_width: f32,
    placeholder: Vec2,
) -> Vec2 {
    let length = |length: Option<Length>, whole: f32| match length? {
        Length::Points(points) => Some(points),
        Length::Percent(share) => Some(whole * share / 100.0),
    };
    let width = length(spec.width, max_width);
    // A percentage height refers to a container height the text does not have: it is ignored.
    let height = match spec.height {
        Some(Length::Points(points)) => Some(points),
        _ => None,
    };
    let aspect = natural.filter(|size| size.x > 0.0 && size.y > 0.0).map(|size| size.y / size.x);
    let size = match (width, height, natural) {
        (Some(width), Some(height), _) => vec2(width, height),
        (Some(width), None, _) => {
            vec2(width, aspect.map_or(placeholder.y, |aspect| width * aspect))
        }
        (None, Some(height), _) => {
            vec2(aspect.map_or(placeholder.x.min(height), |aspect| height / aspect), height)
        }
        (None, None, Some(natural)) => natural,
        (None, None, None) => placeholder,
    };
    let size = size.max(vec2(1.0, 1.0));
    if size.x > max_width && max_width > 0.0 { size * (max_width / size.x) } else { size }
}

/// The height an image of unknown size holds, for height estimates.
pub(super) fn placeholder_height(scale: f32) -> f32 {
    PLACEHOLDER.y * scale
}

/// Status badges are small; their placeholder is too, so a row of them does not jump.
fn is_badge(src: &str) -> bool {
    let src = src.to_ascii_lowercase();
    ["shields.io", "badge", "badgen.net", "codecov.io", "coveralls.io", "/status.svg"]
        .iter()
        .any(|hint| src.contains(hint))
}

/// The state of an image as the current source has it, without loading it.
fn current_state(
    ctx: &Context,
    source: Option<&dyn ImageSource>,
    request: &ImageRequest<'_>,
) -> ImageState {
    match source {
        Some(source) => source.state(ctx, request, false),
        None => ImageState::Failed(Arc::new(ImageFailure {
            reason: "Images load on pull request pages".to_owned(),
            place: host_of(request.src),
            can_retry: false,
            needs_access: false,
            needs_ffmpeg: false,
        })),
    }
}

/// The room an image in `state` takes.
fn layout_size(spec: &ImageSpec, state: &ImageState, max_width: f32, scale: f32) -> Vec2 {
    let placeholder = if is_badge(&spec.src) { BADGE_PLACEHOLDER } else { PLACEHOLDER } * scale;
    match state {
        ImageState::Failed(_) => vec2(max_width.min(CARD_MAX_WIDTH * scale), CARD_HEIGHT * scale),
        ImageState::Waiting { size } => display_size(spec, *size, max_width, placeholder),
        ImageState::Ready { size, .. } => display_size(spec, Some(*size), max_width, placeholder),
    }
}

/// The room an image takes right now, for layouts that measure before they draw (table cells).
pub(super) fn measure(
    ctx: &Context,
    spec: &ImageSpec,
    base: Option<ImageBase>,
    max_width: f32,
    scale: f32,
) -> Vec2 {
    let source = current_source(ctx);
    let base = base.and_then(|base| base_of(ctx, base));
    let request = ImageRequest { src: &spec.src, base: base.as_deref(), video: spec.is_video() };
    layout_size(spec, &current_state(ctx, source.as_deref(), &request), max_width, scale)
}

/// Lays out and draws one image at the cursor.
pub(super) fn show(ui: &mut Ui, shown: &Shown<'_>) {
    let ctx = ui.ctx().clone();
    let source = current_source(&ctx);
    let base = shown.base.and_then(|base| base_of(&ctx, base));
    let video = shown.spec.is_video();
    let request = ImageRequest { src: &shown.spec.src, base: base.as_deref(), video };
    let mut state = current_state(&ctx, source.as_deref(), &request);
    let size = layout_size(shown.spec, &state, shown.max_width, shown.scale);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    keep_anchor(ui, shown.id, rect, shown.anchor);
    let ahead = ui.clip_rect().expand2(vec2(0.0, ui.clip_rect().height() * PRELOAD_SCREENS));
    if let (Some(source), ImageState::Waiting { .. }) = (&source, &state)
        && ui.is_visible()
        && ahead.intersects(rect)
    {
        state = source.state(&ctx, &request, true);
    }
    let is_ready = matches!(state, ImageState::Ready { .. });
    let shown_share = motion::reveal(&ctx, shown.id, is_ready);
    if !ui.is_rect_visible(rect) {
        return;
    }
    response
        .widget_info(|| WidgetInfo::labeled(egui::Role::Image, true, describe(shown.spec, &state)));
    match &state {
        ImageState::Failed(failure) => {
            failure_card(ui, rect, shown, failure, source.as_deref(), &request);
            return;
        }
        ImageState::Waiting { .. } => paint_placeholder(ui, rect, shown, 1.0),
        ImageState::Ready { texture, next_frame, clip, .. } => {
            if shown_share < 1.0 {
                paint_placeholder(ui, rect, shown, 1.0 - shown_share);
            }
            if let Some(clip) = clip {
                let poster = video::Poster { texture: *texture, opacity: shown_share };
                video::show(ui, rect, &response, shown.id, clip, poster, shown.palette);
            } else {
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                ui.painter().image(*texture, rect, uv, Color32::WHITE.gamma_multiply(shown_share));
                if let Some(next_frame) = next_frame {
                    ctx.request_repaint_after(*next_frame);
                }
            }
        }
    }
    if let Some(border) = shown.border {
        ui.painter().rect_stroke(rect, 2, Stroke::new(2.0, border), StrokeKind::Outside);
    }
    let opens_lightbox = is_ready && !video;
    interact(ui, &response, shown, opens_lightbox, source.as_deref(), &request);
}

/// The accessible name: alt text, and what the image is doing.
fn describe(spec: &ImageSpec, state: &ImageState) -> String {
    let name = display_name(spec);
    let kind = if spec.is_video() { "video" } else { "image" };
    match state {
        ImageState::Waiting { .. } => format!("Loading {kind} {name}"),
        ImageState::Ready { .. } if spec.is_video() => format!("Video {name}"),
        ImageState::Ready { .. } => format!("Image {name}"),
        ImageState::Failed(failure) => {
            let kind = if spec.is_video() { "Video" } else { "Image" };
            format!("{kind} {name} not shown: {}", failure.reason)
        }
    }
}

/// Who keeps the reader's place when an image (or a diagram) above the view changes height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Anchor {
    /// The block scrolls the enclosing scroll area by its change.
    ScrollsView,
    /// The view keeps its own anchor (the rendered diff measures its rows again).
    LeftToView,
}

/// With [`Anchor::ScrollsView`], an image (or a diagram) that changed height above the view moves
/// the view by as much, so the content the reader looks at stays where it is. The pass is redrawn
/// at once with the new offset.
pub(super) fn keep_anchor(ui: &Ui, id: Id, rect: Rect, anchor: Anchor) {
    let height_id = id.with("height");
    let last: Option<f32> = ui.data(|data| data.get_temp(height_id));
    if last == Some(rect.height()) {
        return;
    }
    ui.data_mut(|data| data.insert_temp(height_id, rect.height()));
    if let Some(last) = last
        && anchor == Anchor::ScrollsView
        && rect.top() < ui.clip_rect().top()
    {
        ui.scroll_with_delta_animation(
            vec2(0.0, last - rect.height()),
            egui::style::ScrollAnimation::none(),
        );
        ui.ctx().request_discard("an image above the view changed its height");
    }
}

fn paint_placeholder(ui: &Ui, rect: Rect, shown: &Shown<'_>, opacity: f32) {
    let palette = shown.palette;
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        tokens::RADIUS_CONTROL,
        palette.surface_subtle.gamma_multiply(opacity),
    );
    painter.rect_stroke(
        rect,
        tokens::RADIUS_CONTROL,
        Stroke::new(1.0, palette.border.gamma_multiply(opacity)),
        StrokeKind::Inside,
    );
    if rect.height() >= 24.0 && rect.width() >= 24.0 {
        let icon_size = (rect.height() * 0.3).clamp(12.0, 28.0);
        let font = icons::rest_font(palette, icon_size);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            kind_icon(shown.spec),
            font,
            palette.text_disabled.gamma_multiply(opacity),
        );
    }
}

fn kind_icon(spec: &ImageSpec) -> &'static str {
    if spec.is_video() { icons::FILE_VIDEO } else { icons::IMAGE }
}

/// Hover, click and the context menu of a shown or loading image; a video plays and pauses on its
/// own ([`video::show`]), so `is_ready` is false for it.
fn interact(
    ui: &Ui,
    response: &egui::Response,
    shown: &Shown<'_>,
    is_ready: bool,
    source: Option<&dyn ImageSource>,
    request: &ImageRequest<'_>,
) {
    let ctx = ui.ctx();
    if shown.link.is_some() || is_ready {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let open_lightbox = || {
        if let Some(picture) = source.and_then(|source| source.picture(request)) {
            let title = caption(shown.spec).to_owned();
            let web_url = source.and_then(|source| source.web_url(request));
            let size = match source.map(|source| source.state(ctx, request, false)) {
                Some(ImageState::Ready { size, .. }) => size,
                _ => picture.size_vec2() / ctx.pixels_per_point(),
            };
            open_lightbox(ctx, picture, size, title, web_url);
        }
    };
    if response.clicked() {
        match shown.link {
            Some((target, link)) => {
                let wants_browser = ui.input(|input| input.modifiers.command);
                if link.in_app != InApp::No && !wants_browser {
                    request_in_app(ctx, target);
                } else {
                    ctx.open_url(egui::OpenUrl::new_tab(&link.web_url));
                }
            }
            None if is_ready => open_lightbox(),
            None => {}
        }
    }
    response.context_menu(|ui| {
        if is_ready && buttons::menu_item(ui, icons::IMAGE, "View image", None, None).clicked() {
            open_lightbox();
            ui.close();
        }
        let original = source
            .filter(|_| is_ready)
            .and_then(|source| source.picture(request))
            .and_then(|picture| picture.original().cloned());
        if let Some(original) = original
            && buttons::menu_item(ui, icons::COPY, "Copy image", None, None).clicked()
        {
            let web_url = source.and_then(|source| source.web_url(request));
            image_clipboard::request(ui.ctx(), original, web_url);
            ui.close();
        }
        if let Some((_, link)) = shown.link
            && buttons::menu_item(ui, icons::ARROW_SQUARE_OUT, "Open link in browser", None, None)
                .clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(&link.web_url));
            ui.close();
        }
        if let Some(url) = source.and_then(|source| source.web_url(request)) {
            if buttons::menu_item(ui, icons::ARROW_SQUARE_OUT, "Open image in browser", None, None)
                .clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(&url));
                ui.close();
            }
            if buttons::menu_item(ui, icons::LINK, "Copy image link", None, None).clicked() {
                ui.ctx().copy_text(url);
                ui.close();
            }
        }
    });
    if response.hovered()
        && let Some(hint) = hover_hint(shown, is_ready)
    {
        response.clone().on_hover_text_at_pointer(hint);
    }
}

/// What hovering an image says: where its link goes, that a click enlarges it, or that it loads;
/// nothing over a video, whose bar says it all.
fn hover_hint(shown: &Shown<'_>, is_ready: bool) -> Option<String> {
    match shown.link {
        Some((_, link)) => Some(link_hint(link)),
        None if is_ready => {
            let name = caption(shown.spec);
            Some(if name.trim().is_empty() {
                "Click to enlarge".to_owned()
            } else {
                format!("{name}\nClick to enlarge")
            })
        }
        None if shown.spec.is_video() => None,
        None => Some("Loading the image".to_owned()),
    }
}

/// A compact card in the image's place: what it is, where from, why it is missing, and the ways on.
fn failure_card(
    ui: &mut Ui,
    rect: Rect,
    shown: &Shown<'_>,
    failure: &ImageFailure,
    source: Option<&dyn ImageSource>,
    request: &ImageRequest<'_>,
) {
    let palette = shown.palette;
    let painter = ui.painter();
    painter.rect_filled(rect, tokens::RADIUS_CONTROL, palette.surface_subtle);
    let border = shown.border.unwrap_or(palette.border);
    painter.rect_stroke(rect, tokens::RADIUS_CONTROL, Stroke::new(1.0, border), StrokeKind::Inside);
    let web_url = source
        .and_then(|source| source.web_url(request))
        .or_else(|| is_url(&shown.spec.src).then(|| shown.spec.src.clone()));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(tokens::SPACE_M, tokens::SPACE_S)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.spacing_mut().item_spacing.x = tokens::SPACE_S;
    glyph::decorative(
        &mut child,
        egui::RichText::new(kind_icon(shown.spec))
            .font(icons::rest_font(palette, icons::SIZE_SMALL + 4.0))
            .color(palette.warning),
    );
    child.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if let Some(url) = &web_url
            && buttons::icon(ui, icons::ARROW_SQUARE_OUT, "Open in browser").clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        if failure.needs_ffmpeg {
            let command = cv_media::video::install_command();
            if buttons::subtle(ui, icons::COPY, "Copy install command")
                .on_hover_text(format!("Copies `{command}`; run it, then Retry"))
                .clicked()
            {
                ui.ctx().copy_text(command.to_owned());
            }
        }
        if failure.needs_access
            && let Some(source) = source
            && buttons::subtle(ui, icons::KEY, "Fix token")
                .on_hover_text("Open the connection setup to give the token access")
                .clicked()
        {
            source.fix_access(ui.ctx());
        }
        if failure.can_retry
            && let Some(source) = source
            && buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Retry").clicked()
        {
            source.retry(request);
            ui.ctx().request_repaint();
        }
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let title = egui::RichText::new(display_name(shown.spec))
                .font(theme::fonts::semibold_font(tokens::SMALL_SIZE + 1.0))
                .color(palette.text);
            ui.add(egui::Label::new(title).truncate());
            let detail = if failure.place.is_empty() {
                failure.reason.clone()
            } else {
                format!("{} · {}", failure.reason, failure.place)
            };
            let detail = egui::RichText::new(detail)
                .font(FontId::proportional(tokens::SMALL_SIZE))
                .color(palette.text_secondary);
            ui.add(egui::Label::new(detail).truncate());
        });
    });
}

/// What an image is called: its alt text, else its file name.
fn display_name(spec: &ImageSpec) -> String {
    let alt = spec.alt.trim();
    if alt.is_empty() { file_name(&spec.src) } else { alt.to_owned() }
}

/// What the lightbox and the hover hint call an image: its title, else its alt text.
fn caption(spec: &ImageSpec) -> &str {
    if spec.title.is_empty() { &spec.alt } else { &spec.title }
}

/// The host of an absolute address, else the source as written.
fn host_of(src: &str) -> String {
    url::Url::parse(src)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| src.to_owned())
}

/// The last path segment of a source, for images without alt text.
fn file_name(src: &str) -> String {
    let path = src.split(['?', '#']).next().unwrap_or(src).trim_end_matches('/');
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.is_empty() {
        "Image".to_owned()
    } else {
        urlencoding::decode(name).map_or_else(|_| name.to_owned(), std::borrow::Cow::into_owned)
    }
}

#[cfg(test)]
mod tests {
    use egui::vec2;

    use super::{ImageSpec, Length, display_size, file_name, is_badge};

    fn spec(width: Option<Length>, height: Option<Length>) -> ImageSpec {
        ImageSpec { src: "a.png".into(), width, height, ..ImageSpec::default() }
    }

    #[test]
    fn images_shrink_to_the_text_keeping_their_aspect_and_never_grow() {
        let placeholder = vec2(480.0, 160.0);
        assert_eq!(
            display_size(&spec(None, None), Some(vec2(200.0, 100.0)), 600.0, placeholder),
            vec2(200.0, 100.0),
            "natural size, not stretched"
        );
        assert_eq!(
            display_size(&spec(None, None), Some(vec2(1200.0, 600.0)), 600.0, placeholder),
            vec2(600.0, 300.0)
        );
        assert_eq!(
            display_size(&spec(None, None), None, 300.0, placeholder),
            vec2(300.0, 100.0),
            "the placeholder fits too"
        );
    }

    #[test]
    fn attributes_reserve_the_final_size_before_the_image_arrives() {
        let placeholder = vec2(480.0, 160.0);
        let both = spec(Some(Length::Points(120.0)), Some(Length::Points(40.0)));
        assert_eq!(
            display_size(&both, None, 600.0, placeholder),
            display_size(&both, Some(vec2(999.0, 1.0)), 600.0, placeholder)
        );
        let width_only = spec(Some(Length::Points(100.0)), None);
        assert_eq!(
            display_size(&width_only, Some(vec2(400.0, 200.0)), 600.0, placeholder),
            vec2(100.0, 50.0)
        );
        let percent = spec(Some(Length::Percent(50.0)), None);
        assert_eq!(
            display_size(&percent, Some(vec2(100.0, 100.0)), 600.0, placeholder),
            vec2(300.0, 300.0)
        );
        let height_only = spec(None, Some(Length::Points(20.0)));
        assert_eq!(
            display_size(&height_only, Some(vec2(80.0, 40.0)), 600.0, placeholder),
            vec2(40.0, 20.0)
        );
    }

    #[test]
    fn unnamed_images_are_called_by_their_decoded_file_name() {
        assert_eq!(file_name("../docs/My%20shot.png?raw=true"), "My shot.png");
    }

    #[test]
    fn badge_hosts_get_the_small_placeholder() {
        assert!(is_badge("https://img.shields.io/badge/build-passing-green"));
        assert!(is_badge("https://github.com/o/r/actions/workflows/ci.yml/badge.svg"));
        assert!(!is_badge("./docs/screenshot.png"));
    }
}
