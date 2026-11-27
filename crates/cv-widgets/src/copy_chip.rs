//! A chip that copies its text: a branch name, a commit, an id. It shows the text in the code font
//! after an icon on a quiet fill; a click, Enter or Space puts the exact text on the clipboard and
//! swaps the icon for a check mark for a moment, like "Copy code" and "Copy log" confirm their
//! copies. The chip keeps its size in every state, so hover and the confirmation never move the
//! line it sits in. Text wider than the chip's room ends in an ellipsis; the tooltip then names it
//! in full.

use std::time::Duration;

use cv_base::theme::{self, fonts, icons, style};
use egui::{Align2, Context, CursorIcon, FontId, Id, Response, Sense, Ui, WidgetInfo, pos2, vec2};

use super::{elide, focus, row};

/// Height of a chip: room above and below the text, inside a 28 px line.
pub(crate) const HEIGHT: f32 = 24.0;
/// The text's size, as inline code next to body text.
const TEXT_SIZE: f32 = style::BODY_SIZE - 1.0;
const ICON_GAP: f32 = 6.0;
/// How long the check mark replaces the icon after a copy.
const COPIED_SECONDS: f64 = 1.5;

/// The width a chip with `text` needs to show it whole.
pub fn width(ui: &Ui, text: &str) -> f32 {
    chrome_width() + elide::text_width(ui, text, &font())
}

/// The narrowest a chip gets: its icon and an ellipsis.
pub fn min_width(ui: &Ui) -> f32 {
    width(ui, "…")
}

/// A chip showing `icon` and `text`, at most `max_width` wide. A click (or Enter and Space while
/// it has focus) copies `text`; `tooltip` names that action ("Copy branch name").
pub fn show(ui: &mut Ui, icon: &str, text: &str, tooltip: &str, max_width: f32) -> Response {
    let palette = theme::palette(ui.ctx());
    let galley = elide::line(ui.painter(), text, font(), palette.text, max_width - chrome_width());
    let is_elided = galley.elided;
    let size = vec2(chrome_width() + galley.size().x, HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let is_copied = copy_on_click(ui, &response, text);
    response.widget_info(|| {
        let label = if is_copied { format!("Copied {text}") } else { format!("{tooltip}: {text}") };
        WidgetInfo::labeled(egui::Role::Button, true, label)
    });

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
    response.on_hover_cursor(CursorIcon::PointingHand).on_hover_ui(|ui| {
        if is_elided {
            ui.label(format!("{text}\n{tooltip}"));
        } else {
            ui.label(tooltip);
        }
    })
}

/// Copies `text` when `response` was clicked (or activated with Enter or Space) and tells whether
/// the copy is recent enough that the widget still shows its confirmation. Schedules the repaint
/// that takes the confirmation away again.
pub fn copy_on_click(ui: &Ui, response: &Response, text: &str) -> bool {
    if response.clicked() {
        mark_copied(ui.ctx(), response.id, text.to_owned());
    }
    is_showing_copied(ui, response.id)
}

/// Copies `text` and starts the "Copied" confirmation of `id`.
pub fn mark_copied(ctx: &Context, id: Id, text: String) {
    let now = ctx.input(|input| input.time);
    ctx.copy_text(text);
    ctx.data_mut(|data| data.insert_temp(copied_id(id), now));
}

/// Whether `id` still shows "Copied"; asks for the frame that ends it.
pub fn is_showing_copied(ui: &Ui, id: Id) -> bool {
    let now = ui.input(|input| input.time);
    let copied_at: Option<f64> = ui.data(|data| data.get_temp(copied_id(id)));
    let Some(at) = copied_at.filter(|at| now - at < COPIED_SECONDS) else {
        return false;
    };
    ui.ctx().request_repaint_after(Duration::from_secs_f64(at + COPIED_SECONDS - now));
    true
}

fn copied_id(id: Id) -> Id {
    id.with("copied_at")
}

/// Padding on both sides, the icon and the gap after it.
fn chrome_width() -> f32 {
    style::SPACE_S * 2.0 + icons::SIZE_SMALL + ICON_GAP
}

fn font() -> FontId {
    fonts::code_font(TEXT_SIZE)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use egui::{Context, Id, RawInput, ViewportId};

    /// One frame at `time`: whether `id` shows "Copied", and when egui paints next.
    fn frame(ctx: &Context, time: f64, copy: bool) -> (bool, Duration) {
        let id = Id::unique("copy");
        let mut is_copied = false;
        let input = RawInput { time: Some(time), ..RawInput::default() };
        let output = ctx.run_ui(input, |ui| {
            if copy {
                super::mark_copied(ui.ctx(), id, "a1b2c3d".to_owned());
            }
            is_copied = super::is_showing_copied(ui, id);
        });
        let wake = output.viewport_output[&ViewportId::ROOT].repaint_delay;
        output.drop_without_applying_deltas();
        (is_copied, wake)
    }

    #[test]
    fn copied_shows_for_a_moment_and_wakes_the_frame_that_ends_it() {
        let ctx = Context::default();
        assert!(!frame(&ctx, 0.0, false).0, "nothing copied yet");
        assert!(frame(&ctx, 1.0, true).0, "copied this frame");
        let (is_copied, wake) = frame(&ctx, 2.0, false);
        assert!(is_copied);
        assert!(
            (wake.as_secs_f64() - 0.5).abs() < 0.01,
            "the next frame comes when the confirmation ends, not {wake:?}"
        );
        assert!(!frame(&ctx, 2.6, false).0, "gone after {}s", super::COPIED_SECONDS);
    }
}
