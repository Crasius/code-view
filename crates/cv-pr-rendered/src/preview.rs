//! Rendered before/after view for images, SVG and Markdown. Decoding runs in the background
//! and textures are capped so a huge asset cannot blow up GPU memory. Images decode and play as
//! in rendered Markdown (`cv_media::images`): GIF, WebP and APNG animations play. Decoded pixels move to the
//! GPU the frame they arrive and leave the heap: only the texture handles stay (a 1200×800 image
//! kept 3.7 MB per side besides its texture), and the encoded file, which "Copy image" in a side's
//! context menu decodes again at full resolution. Markdown shows as a rendered diff (see
//! [`markdown_diff`](crate::markdown_diff)).

use std::sync::Arc;

use egui::Ui;
use review_core::{DiffSide, ReviewThread};
use review_git::ChangedFile;
use review_store::{AppSettings, ImageCompare, PaneSizes, RenderedWidth};

use crate::markdown_diff::{MarkdownDiff, MarkdownModel, RenderedEvent, RenderedInputs};
use cv_base::theme::{self, icons, style};
use cv_media::images::{self as image_store, Decoded};
use cv_media::picture::Picture;
use cv_pr_diff::diff_view::{ChangePosition, Direction};
use cv_pr_session::session::PullRequestSession;
use cv_pr_session::web_link;
use cv_pr_threads::ThreadsUi;
use cv_runtime::load::Load;
use cv_services::Services;
use cv_widgets::splitter::Splitter;
use cv_widgets::states::{self, Action};
use cv_widgets::tabs::{self, Segment};
use cv_widgets::{skeleton, surfaces};

mod canvas;
mod load;

use canvas::{Textures, image_column, paint_overlay};
use load::LoadJob;
pub use load::load_markdown;

const COLUMN_GAP: f32 = style::SPACE_L;
/// How a changed image compares its two versions.
const COMPARE_MODES: [Segment<'static, ImageCompare>; 3] = [
    Segment::new(ImageCompare::SideBySide, "2-up"),
    Segment::new(ImageCompare::Swipe, "Swipe"),
    Segment::new(ImageCompare::OnionSkin, "Onion skin"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewKind {
    Raster,
    Svg,
    Markdown,
}

impl PreviewKind {
    fn of(path: &str) -> Option<Self> {
        let extension =
            path.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase())?;
        match extension.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" => Some(Self::Raster),
            "svg" => Some(Self::Svg),
            "md" | "markdown" | "mdx" => Some(Self::Markdown),
            _ => None,
        }
    }
}

#[derive(Clone)]
enum Side {
    /// Decoded frames, until [`upload`] moves them into textures.
    Image(Decoded),
    /// An image whose pixels are on the GPU; [`Preview`]'s textures hold it.
    Uploaded,
    Text(String),
}

/// A file's preview, loaded: its versions and, for Markdown, the rendered diff of them.
#[derive(Clone)]
pub struct Loaded {
    old: Option<Side>,
    new: Option<Side>,
    /// Markdown: identifies the images the versions show: the file's paths and the blobs of its
    /// repository images, not the commits they resolve in, so a comparison that moved leaves the
    /// rendered diff as it is.
    images_key: u64,
    /// Markdown: both versions parsed and lined up, with their images, by the load.
    pub model: Option<Arc<MarkdownModel>>,
}

/// The file a preview was loaded for: two files with the same blobs in other folders (a copied
/// README) show their own images.
#[derive(Debug, PartialEq, Eq)]
struct LoadedFor {
    /// Its kind, and the folder its relative images resolve in.
    path: String,
    old_oid: String,
    new_oid: String,
    /// The compared (base, head) commits its images resolve in.
    commits: Option<(String, String)>,
}

impl LoadedFor {
    fn new(file: &ChangedFile, commits: Option<(String, String)>) -> Self {
        let (path, old_oid, new_oid) =
            (file.path.clone(), file.old_oid.clone(), file.new_oid.clone());
        Self { path, old_oid, new_oid, commits }
    }

    /// The preview for `other` shows the same versions of the same file: only the commits its
    /// images resolve in moved (an update of the pull request).
    fn same_versions(&self, other: &Self) -> bool {
        (&self.path, &self.old_oid, &self.new_oid) == (&other.path, &other.old_oid, &other.new_oid)
    }
}

pub struct Preview {
    is_enabled: bool,
    /// A Markdown or SVG file shown as its text diff although previews are on: the user jumped to
    /// a comment in it, or its preview failed. Switching the preview on again ends it.
    source_for: Option<String>,
    compare: ImageCompare,
    mix: f32,
    loaded_for: Option<LoadedFor>,
    content: Load<Arc<Loaded>>,
    /// The shown file's preview loading again for moved commits, while `content` stays on screen.
    reloading: Load<Arc<Loaded>>,
    /// When `content` started loading, on egui's clock: its placeholder waits a moment.
    started: f64,
    textures: Textures,
    markdown: MarkdownDiff,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            is_enabled: true,
            source_for: None,
            compare: ImageCompare::SideBySide,
            mix: 0.5,
            loaded_for: None,
            content: Load::Idle,
            reloading: Load::Idle,
            started: 0.0,
            textures: Textures::default(),
            markdown: MarkdownDiff::default(),
        }
    }
}

/// Identifies the rendered diff of a Markdown file: its two versions and their images
/// ([`Loaded::images_key`]). Both the model the load builds and every lookup of the diff's state
/// use it, so they cannot drift apart.
fn rendered_key(file: &ChangedFile, images_key: u64) -> u64 {
    egui::epaint::util::hash((
        &file.path,
        &file.old_oid,
        &file.new_oid,
        file.has_old_blob(),
        file.has_new_blob(),
        images_key,
    ))
}

pub(crate) fn can_preview(file: &ChangedFile) -> bool {
    PreviewKind::of(&file.path).is_some()
}

/// Previews the user can switch off for the text diff: images always show as images.
pub(crate) fn is_optional(file: &ChangedFile) -> bool {
    matches!(PreviewKind::of(&file.path), Some(PreviewKind::Svg | PreviewKind::Markdown))
}

impl Preview {
    /// The user switched previews on or off.
    pub fn set_enabled(&mut self, is_enabled: bool) {
        self.is_enabled = is_enabled;
        self.source_for = None;
    }

    /// The saved choices, which another pull request may have changed.
    pub fn restore(&mut self, is_enabled: bool, compare: ImageCompare) {
        self.is_enabled = is_enabled;
        self.compare = compare;
    }

    /// Shows `path` as its text diff, where comments sit, without changing the choice for other files.
    pub fn show_source(&mut self, path: &str) {
        if self.source_for.as_deref() != Some(path) {
            self.source_for = Some(path.to_owned());
        }
    }

    /// Markdown and SVG files show as a preview rather than as a text diff.
    pub fn is_enabled(&self) -> bool {
        self.is_enabled
    }

    pub fn compare(&self) -> ImageCompare {
        self.compare
    }

    /// Images always preview (a text diff of a PNG is useless); Markdown follows the toggle.
    pub fn is_active(&self, file: &ChangedFile) -> bool {
        self.previews(&file.path)
    }

    fn previews(&self, path: &str) -> bool {
        match PreviewKind::of(path) {
            Some(PreviewKind::Raster) => true,
            Some(_) => self.is_enabled && self.source_for.as_deref() != Some(path),
            None => false,
        }
    }

    /// The page went out of sight (see the Files tab's `hibernate`): what the preview loaded
    /// goes, unless it shows `open`, the file open in the tab, whose rendered view keeps its
    /// place. The choices the user made stay.
    pub fn hibernate(&mut self, open: Option<&str>) {
        if open.is_some_and(|path| self.previews(path)) {
            return;
        }
        *self = Self {
            is_enabled: self.is_enabled,
            source_for: self.source_for.take(),
            compare: self.compare,
            mix: self.mix,
            ..Self::default()
        };
    }

    /// Where the rendered Markdown diff's change in focus is among its changes while it shows
    /// `file`; `None` for other previews and until it is laid out.
    pub fn change_position(&self, file: &ChangedFile) -> Option<ChangePosition> {
        if !is_markdown(&file.path) || !self.is_active(file) {
            return None;
        }
        let Load::Ready(loaded) = &self.content else {
            return None;
        };
        self.markdown.change_position(rendered_key(file, loaded.images_key))
    }

    /// Steps to the next or previous change of the rendered Markdown diff.
    pub fn go_to_change(&mut self, direction: Direction) {
        self.markdown.go_to_change(direction);
    }

    /// The source line at the top of the rendered Markdown diff, for the text diff to open there.
    pub fn top_line(&self) -> Option<(DiffSide, u32)> {
        self.markdown.top_line()
    }

    /// The rendered Markdown diff opens at this source line (the text diff's place), or at the top.
    pub fn reveal_line(&mut self, place: Option<(DiffSide, u32)>) {
        self.markdown.reveal_line(place);
    }

    /// The rendered Markdown fills the pane (Full width, or a reading column as wide as the pane).
    pub(crate) fn fills_width(&self) -> bool {
        self.markdown.fills_width()
    }

    /// Switches the rendered Markdown between its reading column and full width.
    pub(crate) fn switch_width(&self, width: &mut RenderedWidth) {
        self.markdown.switch_width(width);
    }

    /// The rendered Markdown diff scrolls to this thread's card once it shows.
    pub fn reveal_thread(&mut self, thread_id: &str) {
        self.markdown.reveal_thread(thread_id);
    }

    /// Whether the file at `path` shows as a rendered Markdown diff, where its comments show too.
    pub fn shows_rendered(&self, path: &str) -> bool {
        is_markdown(path) && self.previews(path)
    }

    /// Returns what the user asked of the rendered Markdown diff that the tab carries out:
    /// commenting on a block's lines, showing them in the text diff, or a thread's reply or status.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        session: &PullRequestSession,
        services: &mut Services,
        file: &ChangedFile,
        threads: &[&ReviewThread],
        threads_ui: &mut ThreadsUi,
    ) -> Option<RenderedEvent> {
        let kind = PreviewKind::of(&file.path)?;
        self.ensure_loaded(ui.ctx(), session, services, file, kind);
        // A reload that failed leaves the shown preview, whose versions are the same.
        let is_reloaded = self.reloading.poll() && matches!(self.reloading, Load::Ready(_));
        if is_reloaded {
            self.content = std::mem::take(&mut self.reloading);
        }
        if self.content.poll() || is_reloaded {
            self.textures = match &mut self.content {
                Load::Ready(loaded) => {
                    upload(ui.ctx(), Arc::make_mut(loaded), file, ui.input(|input| input.time))
                }
                _ => Textures::default(),
            };
        }
        let loaded = match &self.content {
            Load::Ready(loaded) => loaded.clone(),
            Load::Failed(message) => {
                let message = message.clone();
                self.show_failure(ui, session, file, kind, &message);
                return None;
            }
            Load::Idle | Load::Loading(_) => {
                show_skeleton(ui, kind, skeleton::reveal(ui.ctx(), self.started));
                return None;
            }
        };
        let settings = &mut services.settings;
        if kind == PreviewKind::Markdown {
            let Some(model) = &loaded.model else {
                show_skeleton(ui, kind, skeleton::reveal(ui.ctx(), self.started));
                return None;
            };
            let inputs = RenderedInputs {
                model,
                platform: session.kind,
                layout: settings.diff_layout,
                context: settings.context_lines,
                // The rendered file follows the code zoom, like the diff of the same file.
                scale: settings.code_font_size / AppSettings::DEFAULT_CODE_FONT_SIZE,
                can_comment: session.has_token,
                threads,
                show_threads: settings.show_comments_in_diff,
                past_end: settings.scroll_past_end,
            };
            let event = self.markdown.show(
                ui,
                &inputs,
                &mut settings.panes.preview_split,
                &mut settings.rendered_width,
                threads_ui,
            );
            let is_setting =
                matches!(event, Some(RenderedEvent::SplitResized | RenderedEvent::WidthResized));
            if is_setting {
                services.save_settings();
                return None;
            }
            return event;
        }
        let mut is_resized = false;
        let output =
            egui::ScrollArea::both().id_salt("preview").auto_shrink(false).show(ui, |ui| {
                egui::Frame::new().inner_margin(egui::Margin::same(style::SPACE_L as i8)).show(
                    ui,
                    |ui| {
                        is_resized = self.show_images(ui, &mut settings.panes.preview_split);
                    },
                );
            });
        surfaces::mark_horizontal_scroll(ui, output.inner_rect);
        if is_resized {
            services.save_settings();
        }
        None
    }

    /// Retry, the file on its platform, and for text formats the plain diff as a way around.
    fn show_failure(
        &mut self,
        ui: &mut Ui,
        session: &PullRequestSession,
        file: &ChangedFile,
        kind: PreviewKind,
        message: &str,
    ) {
        let palette = theme::palette(ui.ctx());
        let link = web_link::for_file(session, file);
        let mut actions = vec![Action::primary(icons::ARROW_CLOCKWISE, "Retry")];
        let can_show_diff = kind != PreviewKind::Raster;
        if can_show_diff {
            actions.push(Action::secondary(icons::FILE_TEXT, "Show text diff"));
        }
        if let Some(link) = &link {
            actions.push(Action::secondary(link.icon, &link.label));
        }
        match states::notice(
            ui,
            icons::WARNING,
            palette.error,
            "Couldn't show the preview",
            message,
            &actions,
        ) {
            Some(0) => self.loaded_for = None,
            Some(1) if can_show_diff => self.show_source(&file.path),
            Some(_) => {
                if let Some(link) = link {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(link.url));
                }
            }
            None => {}
        }
    }

    fn ensure_loaded(
        &mut self,
        ctx: &egui::Context,
        session: &PullRequestSession,
        services: &Services,
        file: &ChangedFile,
        kind: PreviewKind,
    ) {
        let commits = if kind == PreviewKind::Markdown { session.compared_commits() } else { None };
        let key = LoadedFor::new(file, commits);
        if self.loaded_for.as_ref() == Some(&key) {
            return;
        }
        let Some(mirror) = session.mirror.ready() else {
            return;
        };
        let commits = key.commits.clone();
        let job = LoadJob::new(ctx, mirror.git.clone(), commits, session.kind, file, kind);
        // The same versions for moved commits load behind the shown preview: no placeholder.
        let is_reload = self.content.ready().is_some()
            && self.loaded_for.as_ref().is_some_and(|shown| shown.same_versions(&key));
        self.loaded_for = Some(key);
        if is_reload {
            self.reloading.start(&services.background, job.run());
        } else {
            self.reloading = Load::Idle;
            self.started = skeleton::now(ctx);
            self.textures = Textures::default();
            self.content.start(&services.background, job.run());
        }
    }

    /// Returns true when the user finished resizing the 2-up columns. An added or deleted image
    /// has one side only: it shows alone, without compare modes or an empty column.
    fn show_images(&mut self, ui: &mut Ui, split: &mut f32) -> bool {
        let textures = &self.textures;
        let (old, new) = match (&textures.old, &textures.new) {
            (Some(old), Some(new)) => (old, new),
            (old, new) => {
                let title = if new.is_some() { "Added" } else { "Deleted" };
                image_column(ui, title, new.as_ref().or(old.as_ref()), textures.since);
                return false;
            }
        };
        tabs::segments(ui, &mut self.compare, &COMPARE_MODES);
        ui.add_space(style::SPACE_S);
        match self.compare {
            ImageCompare::SideBySide => {
                Splitter::new("preview_images_splitter", "Resize Before and After")
                    .default_value(PaneSizes::DEFAULT.preview_split)
                    .show_columns(
                        ui,
                        split,
                        COLUMN_GAP,
                        |ui| image_column(ui, "Before", Some(old), textures.since),
                        |ui| image_column(ui, "After", Some(new), textures.since),
                    )
                    .committed
            }
            ImageCompare::Swipe | ImageCompare::OnionSkin => {
                ui.add(
                    egui::Slider::new(&mut self.mix, 0.0..=1.0)
                        .show_value(false)
                        .text("Before ↔ After"),
                );
                ui.add_space(style::SPACE_S);
                let is_swipe = self.compare == ImageCompare::Swipe;
                paint_overlay(ui, [old, new], textures.since, self.mix, is_swipe);
                false
            }
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    /// The rendered diff on screen for `file`, by its key; `None` while the placeholder shows.
    pub fn shown(&self, file: &ChangedFile) -> Option<u64> {
        self.content.ready().map(|loaded| rendered_key(file, loaded.images_key))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn is_loading(&self) -> bool {
        self.content.is_loading() || self.reloading.is_loading()
    }
}

fn text_of(side: Option<&Side>) -> Option<&str> {
    match side {
        Some(Side::Text(text)) => Some(text.as_str()),
        _ => None,
    }
}

/// `path` is a Markdown file, which has a rendered diff.
pub fn is_markdown(path: &str) -> bool {
    PreviewKind::of(path) == Some(PreviewKind::Markdown)
}

/// Moves the decoded images of `loaded` into textures; the sides keep only that they were images.
/// Animations play from `since`, on egui's clock.
fn upload(ctx: &egui::Context, loaded: &mut Loaded, file: &ChangedFile, since: f64) -> Textures {
    Textures {
        old: upload_side(ctx, &mut loaded.old, &format!("preview-old:{}", file.old_oid)),
        new: upload_side(ctx, &mut loaded.new, &format!("preview-new:{}", file.new_oid)),
        since,
    }
}

fn upload_side(ctx: &egui::Context, side: &mut Option<Side>, name: &str) -> Option<Picture> {
    let Some(Side::Image(decoded)) = side.take_if(|side| matches!(side, Side::Image(_))) else {
        return None;
    };
    *side = Some(Side::Uploaded);
    Some(image_store::upload(ctx, name, decoded).0)
}

/// `shown` is how much of the placeholder to draw ([`skeleton::reveal`]); at 0 the area stays empty.
fn show_skeleton(ui: &mut Ui, kind: PreviewKind, shown: f32) {
    if shown <= 0.0 {
        return;
    }
    ui.multiply_opacity(shown);
    let response =
        egui::Frame::new().inner_margin(egui::Margin::same(style::SPACE_L as i8)).show(ui, |ui| {
            ui.columns(2, |columns| {
                for column in columns.iter_mut() {
                    skeleton::bar(column, 80.0, 14.0);
                    column.add_space(style::SPACE_S);
                    match kind {
                        PreviewKind::Markdown => skeleton::code_lines(column, 16, 20.0, false),
                        PreviewKind::Raster | PreviewKind::Svg => {
                            let width = column.available_width();
                            skeleton::bar(column, width, width * 0.6);
                        }
                    }
                }
            });
        });
    skeleton::announce(&response.response, "Loading the preview");
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use egui::{Color32, ColorImage};
    use review_git::{ChangeKind, ChangedFile};
    use review_store::ImageCompare;

    use super::load::{changed_images, decode};
    use super::{Loaded, LoadedFor, Preview, PreviewKind, Side, upload};
    use cv_media::images::{Decoded, MAX_TEXTURE_EDGE, Natural};
    use cv_runtime::load::Load;

    fn image(width: usize, height: usize) -> Side {
        let frames = vec![(ColorImage::filled([width, height], Color32::RED), 0.0)];
        let natural = Natural::Pixels(egui::vec2(width as f32, height as f32));
        Side::Image(Decoded { frames, natural, stream: None, original: None })
    }

    fn loaded(old: Option<Side>, new: Option<Side>) -> Loaded {
        Loaded { old, new, images_key: 0, model: None }
    }

    fn png(kind: ChangeKind) -> ChangedFile {
        ChangedFile { is_binary: true, ..ChangedFile::sample("assets/logo.png", kind) }
    }

    #[test]
    fn uploaded_images_leave_no_pixels_on_the_heap() {
        let ctx = egui::Context::default();
        let mut loaded = loaded(Some(image(1200, 800)), Some(image(1200, 800)));
        let textures = upload(&ctx, &mut loaded, &png(ChangeKind::Modified), 0.0);
        for (side, picture) in [(&loaded.old, &textures.old), (&loaded.new, &textures.new)] {
            assert!(matches!(side, Some(Side::Uploaded)), "the pixels left with the upload");
            let size = picture.as_ref().map(cv_media::picture::Picture::size_vec2);
            assert_eq!(size, Some(egui::vec2(1200.0, 800.0)));
        }
    }

    #[test]
    fn a_missing_side_uploads_nothing() {
        let ctx = egui::Context::default();
        let mut loaded = loaded(None, Some(image(4, 4)));
        let textures = upload(&ctx, &mut loaded, &png(ChangeKind::Added), 0.0);
        assert!(textures.old.is_none() && loaded.old.is_none());
        assert!(textures.new.is_some());
    }

    /// Regression: the preview was loaded once per pair of blobs, so a second file with the same
    /// content in another folder (a copied README) kept the first one's preview, whose relative
    /// images resolve in the first file's folder.
    #[test]
    fn files_with_the_same_blobs_in_other_folders_load_their_own_preview() {
        let readme = |path: &str| ChangedFile::sample(path, ChangeKind::Added);
        let commits = || Some(("base".to_owned(), "head".to_owned()));
        assert_ne!(
            LoadedFor::new(&readme("docs/a/README.md"), commits()),
            LoadedFor::new(&readme("docs/b/README.md"), commits())
        );
    }

    /// Regression: a push moved the compared commits, which keyed the rendered diff, so a Markdown
    /// file whose versions stayed the same loaded again and jumped back to its top. The commits
    /// only say where its images resolve; a moved comparison reloads behind the shown preview.
    #[test]
    fn moved_commits_keep_the_versions_of_the_file() {
        let readme = ChangedFile::sample("README.md", ChangeKind::Modified);
        let at = |head: &str| LoadedFor::new(&readme, Some(("base".to_owned(), head.to_owned())));
        assert!(at("head").same_versions(&at("pushed")));
        let edited = ChangedFile { new_oid: "3".repeat(40), ..readme.clone() };
        assert!(!at("head").same_versions(&LoadedFor::new(&edited, None)));
    }

    #[test]
    fn only_images_shown_by_both_versions_with_other_blobs_changed() {
        let map = |entries: &[(&str, &str)]| -> BTreeMap<String, String> {
            entries
                .iter()
                .map(|(source, blob)| ((*source).to_owned(), (*blob).to_owned()))
                .collect()
        };
        let old = map(&[("a.png", "1"), ("b.png", "2"), ("gone.png", "3")]);
        let new = map(&[("a.png", "1"), ("b.png", "9"), ("new.png", "4")]);
        let [old_changed, new_changed] = changed_images([old, new]);
        assert_eq!(old_changed, map(&[("b.png", "2")]));
        assert_eq!(new_changed, map(&[("b.png", "9")]));
    }

    /// Regression: an added image showed an empty "Before" column beside it.
    #[test]
    fn an_added_image_shows_alone_without_compare_modes() {
        use egui_kittest::kittest::Queryable as _;

        let mut harness = egui_kittest::Harness::builder()
            .with_size([800.0, 600.0])
            .with_context_setup(cv_base::theme::fonts::install)
            .build_ui_state(
                |ui, preview: &mut Preview| {
                    let mut split = 0.5;
                    preview.show_images(ui, &mut split);
                },
                Preview::default(),
            );
        let ctx = harness.ctx.clone();
        let mut loaded = loaded(None, Some(image(40, 20)));
        harness.state_mut().textures = upload(&ctx, &mut loaded, &png(ChangeKind::Added), 0.0);
        harness.run_steps(2);
        harness.get_by_label("Added");
        harness.get_by_label("40 × 20");
        for absent in ["Before", "After", "2-up", "No file on this side."] {
            assert!(harness.query_by_label(absent).is_none(), "{absent} is not shown");
        }
    }

    /// Regression: the preview decoded only an animated GIF's first frame, so it stood still.
    #[test]
    fn an_animated_gif_plays_in_the_preview() {
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        for shade in [0, 255] {
            let buffer = image::RgbaImage::from_pixel(4, 4, image::Rgba([shade, 0, 0, 255]));
            let delay = image::Delay::from_numer_denom_ms(100, 1);
            encoder.encode_frame(image::Frame::from_parts(buffer, 0, 0, delay)).expect("frame");
        }
        drop(encoder);
        let ctx = egui::Context::default();
        let side = decode(&bytes, PreviewKind::Raster).expect("gif");
        let mut loaded = loaded(None, Some(side));
        let textures = upload(&ctx, &mut loaded, &png(ChangeKind::Added), 0.0);
        let picture = textures.new.expect("uploaded");
        let (first, next) = picture.frame_at(Some(0.0));
        assert!(next.is_some(), "a repaint is due for the next frame");
        assert_ne!(picture.frame_at(Some(0.15)).0, first, "the second frame shows");
    }

    /// Regression: a floor on the raster scale let an SVG declaring a huge size rasterize past the
    /// texture cap (`width="1000000"` made a pixmap 10,000 pixels wide).
    #[test]
    fn a_huge_svg_rasterizes_within_the_texture_cap() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1000000" height="10"/>"#;
        let Side::Image(decoded) = decode(svg, PreviewKind::Svg).expect("svg") else {
            panic!("an SVG decodes to an image");
        };
        let [width, _] = decoded.frames[0].0.size;
        assert!(width <= MAX_TEXTURE_EDGE as usize, "rasterized {width} pixels wide");
    }

    #[test]
    fn hibernating_drops_what_loaded_for_a_file_no_longer_open_and_keeps_the_choices() {
        let mut preview = Preview::default();
        preview.restore(false, ImageCompare::Swipe);
        preview.source_for = Some("README.md".into());
        preview.mix = 0.3;
        preview.content = Load::Ready(Arc::new(loaded(None, Some(image(4, 4)))));
        preview.hibernate(Some("assets/logo.png"));
        assert!(preview.content.ready().is_some(), "the open image keeps its preview");
        preview.hibernate(Some("src/main.rs"));
        assert!(preview.content.is_idle(), "a file no longer open lets go of its preview");
        assert!(!preview.is_enabled() && preview.compare() == ImageCompare::Swipe);
        assert_eq!(preview.source_for.as_deref(), Some("README.md"));
        assert!((preview.mix - 0.3).abs() < f32::EPSILON, "the image compare keeps its mix");
    }
}
