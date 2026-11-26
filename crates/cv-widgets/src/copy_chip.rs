//! A chip that copies its text: a branch name, a commit, an id. It shows the text in the code font
//! after an icon on a quiet fill; a click, Enter or Space puts the exact text on the clipboard and
//! swaps the icon for a check mark for a moment, like "Copy code" and "Copy log" confirm their
//! copies. The chip keeps its size in every state, so hover and the confirmation never move the
//! line it sits in. Text wider than the chip's room ends in an ellipsis; the tooltip then names it
//! in full.

use std::sync::Arc;
use std::time::Duration;

use cv_base::theme::{self, fonts, icons, style};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, CursorIcon, FontId, Galley, Response, Sense, Ui, WidgetInfo, pos2, vec2};

use super::{focus, row};

/// Height of a chip: room above and below the text, inside a 28 px line.
pub const HEIGHT: f32 = 24.0;
/// The text's size, as inline code next to body text.
const TEXT_SIZE: f32 = style::BODY_SIZE - 1.0;
const ICON_GAP: f32 = 6.0;
/// How long the check mark replaces the icon after a copy.
const COPIED_SECONDS: f64 = 1.5;

/// The width a chip with `text` needs to show it whole.
pub fn width(ui: &Ui, text: &str) -> f32 {
    let text_width = ui
        .fonts_mut(|fonts| {
            fonts.layout_no_wrap(text.to_owned(), font(), egui::Color32::PLACEHOLDER)
        })
        .size()
        .x;
    chrome_width() + text_width
}

/// The narrowest a chip gets: its icon and an ellipsis.
pub fn min_width(ui: &Ui) -> f32 {
    width(ui, "…")
}

/// A chip showing `icon` and `text`, at most `max_width` wide. A click (or Enter and Space while
/// it has focus) copies `text`; `tooltip` names that action ("Copy branch name").
pub fn show(ui: &mut Ui, icon: &str, text: &str, tooltip: &str, max_width: f32) -> Response {
    let palette = theme::palette(ui.ctx());
    let galley = text_galley(ui, text, (max_width - chrome_width()).max(0.0), palette.text);
    let is_elided = galley.elided;
    let size = vec2(chrome_width() + galley.size().x, HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let is_copied = copy_on_click(ui, &response, text);
    let label = if is_copied { format!("Copied {text}") } else { format!("{tooltip}: {text}") };
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, &label));

    if ui.is_rect_visible(rect) {
        let fill = row::state_fill(ui, &response, palette.surface_subtle);
        let painter = ui.painter();
        painter.rect_filled(rect, style::RADIUS_CONTROL, fill);
        let (glyph, color) = if is_copied {
            (icons::CHECK, palette.success)
        } else {
            (icon, palette.text_secondary)
        };
        let icon_center =
            pos2(rect.left() + style::SPACE_S + icons::SIZE_SMALL / 2.0, rect.center().y);
        painter.text(
            icon_center,
            Align2::CENTER_CENTER,
            glyph,
            icons::rest_font(&palette, icons::SIZE_SMALL),
            color,
        );
        let text_left = icon_center.x + icons::SIZE_SMALL / 2.0 + ICON_GAP;
        let text_top = rect.center().y - galley.size().y / 2.0;
        painter.galley(pos2(text_left, text_top), galley, palette.text);
    }
    focus::ring(ui, &response, rect);
    let hover = if is_elided { format!("{text}\n{tooltip}") } else { tooltip.to_owned() };
    response.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(hover)
}

/// Copies `text` when `response` was clicked (or activated with Enter or Space) and tells whether
/// the copy is recent enough that the widget still shows its confirmation. Schedules the repaint
/// that takes the confirmation away again.
pub fn copy_on_click(ui: &Ui, response: &Response, text: &str) -> bool {
    let copied_id = response.id.with("copied_at");
    let now = ui.input(|input| input.time);
    if response.clicked() {
        ui.ctx().copy_text(text.to_owned());
        ui.data_mut(|data| data.insert_temp(copied_id, now));
    }
    let copied_at: Option<f64> = ui.data(|data| data.get_temp(copied_id));
    let is_copied = copied_at.is_some_and(|at| now - at < COPIED_SECONDS);
    if let Some(at) = copied_at.filter(|_| is_copied) {
        ui.ctx().request_repaint_after(Duration::from_secs_f64(at + COPIED_SECONDS - now));
    }
    is_copied
}

/// Padding on both sides, the icon and the gap after it.
fn chrome_width() -> f32 {
    style::SPACE_S * 2.0 + icons::SIZE_SMALL + ICON_GAP
}

fn font() -> FontId {
    fonts::code_font(TEXT_SIZE)
}

/// `text` on one line, cut with an ellipsis where it is wider than `max_width`.
fn text_galley(ui: &Ui, text: &str, max_width: f32, color: egui::Color32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font(), color));
    job.wrap = TextWrapping::truncate_at_width(max_width);
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}
