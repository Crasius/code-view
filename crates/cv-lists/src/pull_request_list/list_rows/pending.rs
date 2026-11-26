//! A row whose pull request has a write of the user's on its way
//! ([`cv_services::writes::flights`]): the row stays clickable but dims
//! ([`style::PENDING_OPACITY`]), and a marker leads its meta line that names the write ("Merging…")
//! with a spinner, so the row reads as busy, not as disabled. The marker keeps the full strength
//! the dimmed row gives up. Once the write lands the row is normal again, or leaves the list
//! ([`super::leaving`]).

use cv_base::theme::{self, icons, motion, style};
use egui::{Align2, Id, Pos2, Rect, Sense, Ui, pos2, vec2};

const HEIGHT: f32 = 18.0;
const INSET: f32 = 7.0;
const SPINNER_GAP: f32 = 5.0;

/// A write on its way to the row's pull request.
pub struct Marker<'a> {
    /// The write in words, "Merging…".
    pub doing: &'a str,
    /// The pull request as the row writes it, "#7".
    pub number: &'a str,
    /// The card's fade, which the marker follows.
    pub card_opacity: f32,
}

/// Paints the marker on the meta line of `row`, its left edge at `at` (and centred on its y);
/// returns its right edge.
pub fn paint(ui: &Ui, row: Rect, id: Id, at: Pos2, marker: &Marker<'_>) -> f32 {
    let palette = theme::palette(ui.ctx());
    let mut painter = ui.painter_at(row);
    painter.multiply_opacity(marker.card_opacity);
    let font = theme::fonts::semibold_font(style::SMALL_SIZE);
    let optical_center = theme::fonts::optical_center(&painter, &font);
    let words = painter.layout_no_wrap(marker.doing.to_owned(), font, palette.accent_text);
    let glyph = icons::SIZE_SMALL;
    let width = INSET + glyph + SPINNER_GAP + words.size().x + INSET;
    // The words share the meta line's baseline; the pill and the spinner centre on the words'
    // capitals, so the room above and below them is the same.
    let words_top = at.y - words.size().y / 2.0;
    let middle = words_top + optical_center;
    let pill = Rect::from_min_size(pos2(at.x, middle - HEIGHT / 2.0), vec2(width, HEIGHT));
    painter.rect_filled(pill, HEIGHT / 2.0, palette.selection);
    let busy =
        Rect::from_center_size(pos2(pill.left() + INSET + glyph / 2.0, middle), vec2(glyph, glyph));
    if motion::is_reduced(ui.ctx()) {
        // Still, but still saying "busy".
        let font = icons::rest_font(&palette, glyph);
        let color = palette.accent_text;
        painter.text(busy.center(), Align2::CENTER_CENTER, icons::CIRCLE_NOTCH, font, color);
    } else {
        egui::Spinner::new().size(glyph).color(palette.accent_text).paint_at(ui, busy);
    }
    let text_left = busy.right() + SPINNER_GAP;
    painter.galley(pos2(text_left, words_top), words, palette.accent_text);
    let name = format!("Pending on {}: {}", marker.number, marker.doing);
    let response = ui.interact(pill, id.with("pending"), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &name));
    response.on_hover_text("Sent; the row updates once the platform answers");
    pill.right()
}
