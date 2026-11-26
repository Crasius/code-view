//! Thread markers in the diff's gutter: the avatar of who opened an active thread, a check for a
//! resolved one, with a ring while it has new replies. A click opens or collapses the card. While
//! comments are hidden in the diff, a count stands for the threads of a line or block instead, and
//! a click shows them there (see [`show_count`]).

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, motion};
use cv_widgets::{avatar, focus};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, vec2};
use review_core::{ReviewThread, ThreadStatus};

use super::ThreadsUi;
use super::card::excerpt;

const SIZE: f32 = 18.0;
/// The viewer's switch that shows or hides every thread, as the count's hint names it.
const COMMENTS_HINT: &str = concat!("Show comments in the diff (", shortcut!(alt "H"), ")");

/// The threads a count in the gutter stands for while comments are hidden in the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Hidden {
    pub count: usize,
    /// The user showed them here (a click on the count, or a jump to one of them).
    pub is_shown: bool,
    pub has_news: bool,
}

/// The count of `hidden` threads on a speech bubble, centred in `slot`; true when clicked (show
/// them here, or hide them again).
pub fn show_count(ui: &mut Ui, slot: Rect, id: egui::Id, hidden: &Hidden) -> bool {
    let palette = theme::palette(ui.ctx());
    let rect = Rect::from_center_size(slot.center(), vec2(SIZE, SIZE));
    let response = ui.interact(rect.expand(2.0), id, Sense::click());
    let what = if hidden.count == 1 {
        "1 comment".to_owned()
    } else {
        format!("{} comments", hidden.count)
    };
    let label = if hidden.is_shown {
        format!("{what} shown here, click to hide")
    } else {
        format!("{what} hidden here, click to show")
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    let (fill, text) = match (hidden.is_shown, response.hovered()) {
        (true, true) => (palette.accent_hover, palette.on_accent),
        (true, false) => (palette.accent, palette.on_accent),
        (false, true) => (
            motion::mix(palette.surface, palette.accent, if palette.is_dark { 0.34 } else { 0.24 }),
            palette.accent_text,
        ),
        (false, false) => (
            motion::mix(palette.surface, palette.accent, if palette.is_dark { 0.22 } else { 0.14 }),
            palette.accent_text,
        ),
    };
    ui.painter().rect_filled(rect, 5, fill);
    let digits = if hidden.count > 9 { "9+".to_owned() } else { hidden.count.to_string() };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        digits,
        FontId::proportional(11.0),
        text,
    );
    if hidden.has_news {
        ui.painter().rect_stroke(
            rect.expand(1.5),
            6,
            Stroke::new(1.5, palette.accent),
            egui::StrokeKind::Outside,
        );
    }
    focus::ring(ui, &response, rect.expand(3.0));
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("{label}. {COMMENTS_HINT} shows them all."));
    response.clicked()
}

/// Paints `thread`'s marker centred in `slot` (the marker column of its last line) and toggles
/// its card on a click.
pub fn show(ui: &mut Ui, slot: Rect, id: egui::Id, thread: &ReviewThread, threads: &mut ThreadsUi) {
    let palette = theme::palette(ui.ctx());
    let rect = Rect::from_center_size(slot.center(), vec2(SIZE, SIZE));
    let response = ui.interact(rect.expand(2.0), id, Sense::click());
    let is_resolved = thread.status == ThreadStatus::Resolved;
    let is_collapsed = threads.is_collapsed(thread);
    let first = thread.comments.first();
    let who = first.map_or_else(String::new, |comment| comment.author.display_name.clone());
    let state = if is_resolved { "Resolved thread" } else { "Thread" };
    let action = if is_collapsed { "show" } else { "collapse" };
    let label = format!("{state} by {who}, click to {action} it");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    if is_resolved {
        let fill = if palette.is_dark {
            Color32::from_rgb(0x2E, 0x5D, 0x33)
        } else {
            palette.success.gamma_multiply(0.18)
        };
        let check =
            if palette.is_dark { Color32::from_rgb(0x9F, 0xE0, 0x9F) } else { palette.success };
        ui.painter().circle_filled(rect.center(), SIZE / 2.0, fill);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            icons::CHECK,
            icons::state_font(&palette, 11.0, true),
            check,
        );
    } else if let Some(comment) = first {
        avatar::paint(ui.painter(), rect.center(), SIZE, &comment.author);
    }
    if threads.is_unread(thread) {
        ui.painter().circle_stroke(
            rect.center(),
            SIZE / 2.0 + 1.5,
            Stroke::new(1.5, palette.accent),
        );
    } else if response.hovered() {
        ui.painter().circle_stroke(
            rect.center(),
            SIZE / 2.0 + 1.5,
            Stroke::new(1.5, palette.border_strong),
        );
    }
    focus::ring(ui, &response, rect.expand(3.0));
    let tip =
        first.map_or_else(String::new, |comment| format!("{who}: {}", excerpt(&comment.body, 120)));
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
    if response.clicked() {
        threads.toggle_collapsed(thread);
    }
}
