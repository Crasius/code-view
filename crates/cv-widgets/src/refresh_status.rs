//! The quiet "Updating…" note of stale-while-revalidate pages. It lives in a slot of fixed size
//! that is reserved whether anything shows or not, so appearing and fading never move the layout.

use cv_base::theme::{self, icons, motion, style};
use egui::{Rect, Sense, Ui, pos2, vec2};

use crate::states;

pub const WIDTH: f32 = 168.0;
const HEIGHT: f32 = 28.0;
const SPINNER: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshStatus<'a> {
    /// Nothing to say: the shown data is the latest answer.
    Current,
    /// A background refresh runs behind the data on screen.
    Updating,
    /// The last refresh failed; the older data stays. Carries the reason for the tooltip.
    Failed(&'a str),
}

impl<'a> RefreshStatus<'a> {
    /// Updating wins over an older failure: the new attempt may still succeed.
    pub fn of(is_refreshing: bool, failure: Option<&'a str>) -> Self {
        match (is_refreshing, failure) {
            (true, _) => RefreshStatus::Updating,
            (false, Some(message)) => RefreshStatus::Failed(message),
            (false, None) => RefreshStatus::Current,
        }
    }
}

/// Allocates the slot and paints the note right-aligned in it. Returns true when "Retry" was clicked.
pub fn show(ui: &mut Ui, status: RefreshStatus<'_>) -> bool {
    let (rect, slot) = ui.allocate_exact_size(vec2(WIDTH, HEIGHT), Sense::hover());
    let palette = theme::palette(ui.ctx());
    let id = slot.id;
    let updating = motion::toggle(
        ui.ctx(),
        id.with("updating"),
        status == RefreshStatus::Updating,
        motion::MEDIUM,
    );
    let font = egui::FontId::proportional(style::SMALL_SIZE);
    let mut retry = false;
    if updating > 0.0 {
        let painter = ui.painter_at(rect);
        let text = "Updating…";
        let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), palette.text_secondary);
        let text_left = rect.right() - galley.size().x;
        let spinner = Rect::from_center_size(
            pos2(text_left - style::SPACE_S - SPINNER / 2.0, rect.center().y),
            vec2(SPINNER, SPINNER),
        );
        let alpha = updating;
        painter.galley(
            pos2(text_left, rect.center().y - galley.size().y / 2.0),
            galley,
            palette.text_secondary.gamma_multiply(alpha),
        );
        if status == RefreshStatus::Updating {
            states::spinner(ui, spinner, palette.text_secondary);
            let response = ui.interact(rect, id.with("label"), Sense::hover());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, text));
        }
    }
    if let RefreshStatus::Failed(message) = status {
        let label = format!("{}  Not updated · Retry", icons::WARNING);
        let galley = ui.painter().layout_no_wrap(label, font, palette.warning);
        let target = Rect::from_min_size(
            pos2(rect.right() - galley.size().x, rect.center().y - galley.size().y / 2.0),
            galley.size(),
        );
        let response = ui.interact(target.expand(4.0), id.with("retry"), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, "Not updated · Retry")
        });
        let color = if response.hovered() { palette.text } else { palette.warning };
        ui.painter().galley_with_override_text_color(target.min, galley, color);
        crate::focus::ring(ui, &response, target.expand(4.0));
        retry = response.on_hover_text(format!("The last refresh failed: {message}")).clicked();
    }
    retry
}

/// The width of [`show_compact`]'s slot: one icon button.
pub const COMPACT_WIDTH: f32 = HEIGHT;

/// The same note as [`show`] in a square slot for crowded rows: a spinner while updating, a
/// warning to click for a retry after a failure. The words move into the tooltip and stay the
/// accessible name. Returns true when "Retry" was clicked.
pub fn show_compact(ui: &mut Ui, status: RefreshStatus<'_>) -> bool {
    let (rect, slot) = ui.allocate_exact_size(vec2(COMPACT_WIDTH, HEIGHT), Sense::hover());
    let palette = theme::palette(ui.ctx());
    let id = slot.id;
    let updating = motion::toggle(
        ui.ctx(),
        id.with("updating"),
        status == RefreshStatus::Updating,
        motion::MEDIUM,
    );
    match status {
        RefreshStatus::Updating | RefreshStatus::Current => {
            if updating > 0.0 {
                let spinner = Rect::from_center_size(rect.center(), vec2(SPINNER, SPINNER));
                let color = palette.text_secondary.gamma_multiply(updating);
                states::spinner(ui, spinner, color);
            }
            if status == RefreshStatus::Updating {
                let response = ui.interact(rect, id.with("label"), Sense::hover());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Status, true, "Updating…")
                });
                response.on_hover_text("Updating…");
            }
            false
        }
        RefreshStatus::Failed(message) => {
            let response = ui.interact(rect, id.with("retry"), Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, "Not updated · Retry")
            });
            let fill = crate::row::state_fill(ui, &response, egui::Color32::TRANSPARENT);
            ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
            let color = if response.hovered() { palette.text } else { palette.warning };
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icons::WARNING,
                icons::rest_font(&palette, icons::SIZE),
                color,
            );
            crate::focus::ring(ui, &response, rect);
            response.on_hover_text(format!("Not updated: {message}\nClick to retry.")).clicked()
        }
    }
}
