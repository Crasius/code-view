//! Two side-by-side panes of the pipelines tab, the left one resizable through a
//! [`Splitter`] boundary between them.

use cv_widgets::splitter::{Splitter, pane_range};
use egui::{Align, Layout, Rect, Sense, Ui, UiBuilder, pos2};

/// Lays out `first | second` over the remaining space of `ui`; `width` is the left pane's width,
/// at least `min` and at most what leaves the right pane its grab strip. Returns true when the user finished resizing, the moment to persist it.
pub fn show(
    ui: &mut Ui,
    splitter: Splitter,
    min: f32,
    width: &mut f32,
    first: impl FnOnce(&mut Ui),
    second: impl FnOnce(&mut Ui),
) -> bool {
    let rect = ui.available_rect_before_wrap();
    let range = pane_range(min, rect.width());
    let shown = width.clamp(*range.start(), *range.end()).round();
    let line_x = rect.left() + shown;
    pane(ui, "first", Rect::from_min_max(rect.min, pos2(line_x, rect.bottom())), first);
    pane(ui, "second", Rect::from_min_max(pos2(line_x + 1.0, rect.top()), rect.max), second);
    ui.allocate_rect(rect, Sense::hover());
    *width = shown;
    // After both panes, so the boundary sits on top of their widgets.
    splitter.range(range).show(ui, line_x + 1.0, rect.y_range(), width).committed
}

/// Only the second pane, over the remaining space of `ui` (focus mode folds the first away). It
/// keeps the `Ui` ids it has beside the first, so its scroll positions stay.
pub fn show_second(ui: &mut Ui, second: impl FnOnce(&mut Ui)) {
    let rect = ui.available_rect_before_wrap();
    pane(ui, "second", rect, second);
    ui.allocate_rect(rect, Sense::hover());
}

fn pane(ui: &mut Ui, salt: &str, rect: Rect, content: impl FnOnce(&mut Ui)) {
    let mut child = ui.new_child(
        UiBuilder::new().id_salt(salt).max_rect(rect).layout(Layout::top_down(Align::Min)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    content(&mut child);
}
