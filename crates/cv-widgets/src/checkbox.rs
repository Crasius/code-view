//! The app's one checkbox: a Phosphor square that fills with the accent and a check when on, the
//! same glyphs the file tree's viewed marks and the category filter paint. egui's own checkbox
//! draws a 14 px box with a polyline check, a second look beside those, and sizing it through
//! `spacing.icon_width` would also grow radio buttons and collapsing-header arrows.

use cv_base::theme::{self, Palette, icons, style};
use egui::{
    Align2, Color32, Painter, Pos2, Rect, Response, Sense, TextStyle, Ui, WidgetInfo, WidgetText,
    pos2, vec2,
};

use crate::focus;

/// Glyph size of a checkbox beside body text. Phosphor draws the box at about four fifths of the
/// glyph, so the box shows about 19 px, the size of a web form checkbox.
pub(crate) const SIZE: f32 = icons::SIZE_LARGE + style::SPACE_XS;

/// What a checkbox shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Off,
    On,
    /// Some of what it stands for is on (a folder with some files viewed).
    Mixed,
}

impl From<bool> for Mark {
    fn from(is_on: bool) -> Self {
        if is_on { Mark::On } else { Mark::Off }
    }
}

/// The accent when on or mixed; an empty box that darkens while hovered.
pub fn color(palette: &Palette, mark: Mark, is_hovered: bool) -> Color32 {
    match mark {
        Mark::On | Mark::Mixed => palette.accent,
        Mark::Off if is_hovered => palette.text,
        Mark::Off => palette.text_secondary,
    }
}

/// Paints `mark` as a `size` glyph centered on `center`: filled when on or mixed, outlined when off.
pub fn paint(
    painter: &Painter,
    palette: &Palette,
    center: Pos2,
    size: f32,
    mark: Mark,
    color: Color32,
) {
    let glyph = match mark {
        Mark::On => icons::CHECK_SQUARE,
        Mark::Mixed => icons::MINUS_SQUARE,
        Mark::Off => icons::SQUARE,
    };
    let font = icons::state_font(palette, size, mark != Mark::Off);
    painter.text(center, Align2::CENTER_CENTER, glyph, font, color);
}

/// A checkbox with `label` after it (empty for a box named elsewhere, such as by its row), as tall
/// as a button so it lines up with controls beside it. A click, Space or Enter flips `value`.
pub fn checkbox(ui: &mut Ui, value: &mut bool, label: impl Into<WidgetText>) -> Response {
    let palette = theme::palette(ui.ctx());
    let label: WidgetText = label.into();
    let gap = ui.spacing().icon_spacing;
    let galley = (!label.is_empty()).then(|| {
        let wrap_width = ui.available_width() - SIZE - gap;
        label.into_galley(ui, None, wrap_width, TextStyle::Body)
    });
    let text = galley.as_ref().map_or(egui::Vec2::ZERO, |galley| galley.size());
    let width = SIZE + if galley.is_some() { gap + text.x } else { 0.0 };
    let height = ui.spacing().interact_size.y.max(text.y);
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let name = galley.as_ref().map_or_else(String::new, |galley| galley.text().to_owned());
    response
        .widget_info(|| WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *value, &name));
    if ui.is_rect_visible(rect) {
        let mark = Mark::from(*value);
        let center = pos2(rect.left() + SIZE / 2.0, rect.center().y);
        paint(
            ui.painter(),
            &palette,
            center,
            SIZE,
            mark,
            color(&palette, mark, response.hovered()),
        );
        focus::ring(ui, &response, Rect::from_center_size(center, vec2(SIZE, SIZE)).expand(3.0));
        if let Some(galley) = galley {
            let top_left = pos2(center.x + SIZE / 2.0 + gap, rect.center().y - text.y / 2.0);
            ui.painter().galley(top_left, galley, palette.text);
        }
    }
    response
}
