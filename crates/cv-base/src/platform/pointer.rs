use egui::{Context, CursorIcon};

use crate::platform;

/// Browser convention: everything clickable shows the pointing hand. egui does this only for its
/// own buttons, so custom rows, tabs, chips and links get it here, once per frame, after the UI
/// ran. Widgets that chose a cursor themselves (text fields, resize handles) keep theirs, and so
/// does an app-drawn title bar, whose clicks are the window's double click.
pub fn pointing_hand_on_clickables(ctx: &Context) {
    if ctx.output(|output| output.cursor_icon) != CursorIcon::Default {
        return;
    }
    let hovered = ctx.interaction_snapshot(|snapshot| snapshot.hovered.clone());
    let is_clickable = hovered.iter().any(|id| {
        !platform::is_window_drag_area(ctx, *id)
            && ctx
                .read_response(*id)
                .is_some_and(|response| response.enabled() && response.sense.senses_click())
    });
    if is_clickable {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
}
