//! The miniature of a pull request page on a card of Settings › Appearance › Pull request header:
//! the app's top bar, the header, the tab bar with the card's tab underlined, the content with its
//! scroll bar, and a caption under it. It holds a still frame of a mode ([`Phase::Still`]) until a
//! choice asks to watch it; then it plays that mode's story in a loop ([`super::story`]), the
//! captions saying what happens ([`Phase::Playing`]). Under reduced motion a watched mode keeps its
//! still frame and shows its main caption ([`Phase::Frozen`]). Sizes are in points of the design.

use cv_base::theme::{self, Palette, icons, style};
use egui::epaint::Mesh;
use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use review_store::HeaderMode;

use super::story::{self, Caption, Scene};

const HEIGHT: f32 = 172.0;
const CAPTION_HEIGHT: f32 = 20.0;
/// The miniature with its caption, as [`show`] lays them out.
pub const HEIGHT_WITH_CAPTION: f32 = HEIGHT + style::SPACE_S + CAPTION_HEIGHT;
const APP_BAR: f32 = 12.0;
/// The header's height: how far it slides.
pub(super) const HEADER: f32 = 34.0;
const TAB_BAR: f32 = 18.0;
const CONTENT: f32 = 150.0;
/// Room right of the page for its scroll bar.
const TRACK_ROOM: f32 = 12.0;
/// The inset of the bars inside the header, the tab bar and the content.
const INSET: f32 = 12.0;
/// A tab's mark in the tab bar, and the distance from one tab to the next.
const TAB_WIDTH: f32 = 22.0;
const TAB_STEP: f32 = 30.0;
/// The content's lines, as shares of its width.
const LINES: [f32; 14] =
    [0.90, 0.76, 0.84, 0.60, 0.88, 0.70, 0.82, 0.64, 0.90, 0.74, 0.86, 0.58, 0.80, 0.68];
const THUMB_HEIGHT: f32 = 48.0;

/// What the miniature shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// The mode's still frame, inviting to watch it.
    Still,
    /// Watched under reduced motion: the still frame with the main caption.
    Frozen,
    /// Watched: the story this many seconds after it started.
    Playing(f64),
}

/// The miniature of the card for tab `tab` (its place in the tab bar, 0 for the first) named
/// `name`, showing `mode` as `phase` says, with its caption.
pub fn show(ui: &mut Ui, tab: usize, name: &str, mode: HeaderMode, phase: Phase) {
    let palette = theme::palette(ui.ctx());
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::hover());
    let scene = match phase {
        Phase::Playing(seconds) => story::scene_at(mode, tab, width, story::share(mode, seconds)),
        Phase::Still | Phase::Frozen => story::still(mode),
    };
    paint_page(ui, rect, tab, &scene, &palette);
    ui.add_space(style::SPACE_S);
    let (line, response) = ui.allocate_exact_size(vec2(width, CAPTION_HEIGHT), Sense::hover());
    let (captions, color) = match phase {
        Phase::Still => (vec![(story::idle_caption(), 1.0)], palette.text_secondary),
        Phase::Frozen => (vec![(story::main_caption(mode, name), 1.0)], palette.text),
        Phase::Playing(seconds) => {
            (story::captions(mode, name, story::share(mode, seconds)), palette.text)
        }
    };
    let read = story::shown(&captions).map_or("", |caption| caption.text.as_str());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, read));
    for (caption, opacity) in &captions {
        paint_caption(ui, line, caption, color, *opacity);
    }
}

/// The left edge of tab `tab`'s mark in the tab bar.
pub(super) fn tab_left(tab: usize) -> f32 {
    INSET + TAB_STEP * tab as f32
}

/// `caption` on the caption `line` in `color` at `opacity`, its arrow in the accent; text past
/// the line's end elides.
fn paint_caption(ui: &Ui, line: Rect, caption: &Caption, color: Color32, opacity: f32) {
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter_at(line);
    let mut left = line.left();
    if let Some(icon) = caption.icon {
        let at = pos2(left + icons::SIZE_SMALL / 2.0, line.center().y);
        let font = icons::rest_font(&palette, icons::SIZE_SMALL);
        let ink = palette.accent_text.gamma_multiply(opacity);
        painter.text(at, Align2::CENTER_CENTER, icon, font, ink);
        left += icons::SIZE_SMALL + style::SPACE_S;
    }
    let font = egui::FontId::proportional(style::SMALL_SIZE);
    let color = color.gamma_multiply(opacity);
    let mut job = LayoutJob::simple_singleline(caption.text.clone(), font, color);
    job.wrap = TextWrapping::truncate_at_width(line.right() - left);
    let galley = painter.layout_job(job);
    let top = line.center().y - galley.size().y / 2.0;
    painter.galley(pos2(left, top), galley, color);
}

/// The page: the app's top bar, the header slid by the scene, the tab bar, the content and the
/// scroll bar, clipped to the miniature's rounded frame.
fn paint_page(ui: &Ui, rect: Rect, tab: usize, scene: &Scene, palette: &Palette) {
    let radius = style::RADIUS_POPUP;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, radius, palette.ground);
    let app_bar = Rect::from_min_size(rect.min, vec2(rect.width(), APP_BAR));
    let top_corners = CornerRadius { nw: radius, ne: radius, sw: 0, se: 0 };
    painter.rect_filled(app_bar, top_corners, palette.rail);

    let page_width = rect.width() - TRACK_ROOM;
    let below_bar = Rect::from_min_max(pos2(rect.left(), app_bar.bottom()), rect.max);
    let page = painter.with_clip_rect(below_bar.intersect(painter.clip_rect()));
    let line = Stroke::new(1.0, palette.border);
    let header = Rect::from_min_size(
        pos2(rect.left(), app_bar.bottom() - scene.header_hidden),
        vec2(page_width, HEADER),
    );
    page.rect_filled(header, 0, palette.surface);
    page.hline(header.x_range(), header.bottom() - 0.5, line);
    let inner = page_width - 2.0 * INSET;
    let title = Rect::from_min_size(
        pos2(header.left() + INSET, header.center().y - 3.5),
        vec2(inner * 0.44, 7.0),
    );
    page.rect_filled(title, 4, palette.border_strong);
    let subtitle = Rect::from_min_size(
        pos2(title.right() + style::SPACE_S, title.top()),
        vec2(inner * 0.16, 7.0),
    );
    page.rect_filled(subtitle, 4, palette.border_strong);
    let action = Rect::from_min_size(
        pos2(header.right() - INSET - 30.0, header.center().y - 6.0),
        vec2(30.0, 12.0),
    );
    page.rect_filled(action, 3, palette.brand);

    let tabs = Rect::from_min_size(header.left_bottom(), vec2(page_width, TAB_BAR));
    page.rect_filled(tabs, 0, palette.surface);
    for index in 0..4 {
        let mark = Rect::from_min_size(
            pos2(tabs.left() + tab_left(index), tabs.center().y - 2.5),
            vec2(TAB_WIDTH, 5.0),
        );
        page.rect_filled(mark, 3, palette.border);
    }
    let content = Rect::from_min_size(tabs.left_bottom(), vec2(page_width, CONTENT));
    page.rect_filled(content, 0, palette.surface);
    let text = page.with_clip_rect(content.intersect(page.clip_rect()));
    let top = content.top() + 10.0 - scene.lines;
    for (index, share) in LINES.iter().enumerate() {
        let at = pos2(content.left() + INSET, top + index as f32 * 14.0);
        text.rect_filled(Rect::from_min_size(at, vec2(inner * share, 6.0)), 3, palette.border);
    }
    page.hline(tabs.x_range(), tabs.bottom() - 0.5, line);
    let before = usize::from(tab == 0);
    let underline_left = tab_left(before) + (tab_left(tab) - tab_left(before)) * scene.underline;
    let underline = Rect::from_min_size(
        pos2(tabs.left() + underline_left, tabs.bottom() - 1.0),
        vec2(22.0, 2.0),
    );
    page.rect_filled(underline, 0, palette.accent);

    let track = Rect::from_min_max(
        pos2(rect.right() - 8.0, rect.top() + 16.0),
        pos2(rect.right() - 3.0, rect.bottom() - 4.0),
    );
    painter.rect_filled(track, 3, palette.hover);
    let thumb =
        Rect::from_min_size(track.min + vec2(0.0, scene.thumb), vec2(track.width(), THUMB_HEIGHT));
    painter.rect_filled(thumb, 3, palette.text_disabled);
    if let Some(tip) = scene.pointer {
        paint_pointer(&painter, rect.min + tip, palette);
    }
    painter.rect_stroke(rect, radius, Stroke::new(1.0, palette.border), StrokeKind::Inside);
}

/// The arrow pointer with its tip at `tip`: filled in the text color, outlined in the surface's.
fn paint_pointer(painter: &egui::Painter, tip: Pos2, palette: &Palette) {
    // The design's arrow, 14 by 18, its tip at (1, 1). Concave, so it fills as triangles.
    const OUTLINE: [(f32, f32); 7] = [
        (1.0, 1.0),
        (12.0, 10.0),
        (7.0, 11.0),
        (10.0, 16.0),
        (8.0, 17.0),
        (5.0, 12.0),
        (1.0, 15.0),
    ];
    const TRIANGLES: [[usize; 3]; 5] = [[0, 1, 2], [0, 2, 5], [0, 5, 6], [2, 3, 4], [2, 4, 5]];
    let at = |(x, y): (f32, f32)| tip + vec2(x - 1.0, y - 1.0);
    let points: Vec<Pos2> = OUTLINE.iter().copied().map(at).collect();
    painter.add(egui::Shape::closed_line(points.clone(), Stroke::new(2.0, palette.surface)));
    let mut mesh = Mesh::default();
    for point in &points {
        mesh.colored_vertex(*point, palette.text);
    }
    for [a, b, c] in TRIANGLES {
        mesh.add_triangle(a as u32, b as u32, c as u32);
    }
    painter.add(egui::Shape::mesh(mesh));
}
