//! A thin horizontal scrollbar per code pane, floating over the bottom of the diff like egui's
//! own scrollbars. Each pane of a side-by-side diff scrolls sideways on its own.

use egui::{CursorIcon, Id, Rect, Sense, Ui, pos2, vec2};

use cv_base::theme;

const THIN: f32 = 4.0;
const THICK: f32 = 8.0;
const MIN_THUMB: f32 = 32.0;

/// `track` spans the pane's code area at the bottom of the viewport.
pub fn show(ui: &Ui, track: Rect, id: Id, content_width: f32, offset: &mut f32) {
    let overflow = content_width - track.width();
    if overflow <= 0.0 {
        *offset = 0.0;
        return;
    }
    *offset = offset.clamp(0.0, overflow);
    let thumb_width = (track.width() * track.width() / content_width).max(MIN_THUMB);
    let travel = track.width() - thumb_width;
    let hit_area = Rect::from_min_max(pos2(track.left(), track.bottom() - THICK - 4.0), track.max);
    let response = ui.interact(hit_area, id, Sense::drag()).on_hover_cursor(CursorIcon::Default);
    if response.dragged() && travel > 0.0 {
        *offset = (*offset + response.drag_delta().x * overflow / travel).clamp(0.0, overflow);
    }
    let is_active = response.hovered() || response.dragged();
    let thickness = if is_active { THICK } else { THIN };
    let left = track.left() + travel * (*offset / overflow);
    let thumb = Rect::from_min_size(
        pos2(left, track.bottom() - thickness - 2.0),
        vec2(thumb_width, thickness),
    );
    let palette = theme::palette(ui.ctx());
    let alpha = if is_active { 0.55 } else { 0.35 };
    ui.painter().rect_filled(thumb, thickness / 2.0, palette.text_secondary.gamma_multiply(alpha));
}
