//! Draggable boundary between two panes, side by side or stacked, in the Azure DevOps style: a
//! hairline at rest, a 2 px accent bar and a resize cursor while hovered, focused or dragged.
//!
//! Presentational: the caller owns the size (points or a share of a width), passes it in by
//! `&mut`, and persists it when the response says the user finished.
//!
//! ```ignore
//! let response = Splitter::new("file_tree", "Resize file tree")
//!     .default_value(340.0)
//!     .range(220.0..=720.0)
//!     .show(ui, panel_rect.right(), panel_rect.y_range(), &mut sizes.file_tree);
//! if response.committed {
//!     save();
//! }
//! ```
//!
//! * `per_point` maps pointer travel to the value: `1.0` (default) for a pane left of (or above)
//!   the boundary, `-1.0` for a pane right of (or below) it, `1.0 / width` for a share of `width`.
//! * Drag moves the boundary; the value follows the pointer exactly (no drift at the limits).
//! * Double-click, or Enter while focused, resets to `default_value`.
//! * Keyboard (Tab to focus): ←/→ (↑/↓ for a horizontal boundary) move by 8 points (Shift: 32),
//!   Home/End jump to the limits.
//! * Call it after laying out both panes: the boundary then sits on top of their widgets.
//! * [`Splitter::show_columns`] lays out two columns around a boundary for content that is not a
//!   panel (previews, overview columns).
//! * [`Splitter::show_horizontal`] is the same boundary lying flat, between stacked panes: the
//!   pointer's y moves it (the assistant card's conversation height).

use std::borrow::Cow;
use std::ops::RangeInclusive;

use cv_base::theme::{self, motion};
use egui::{
    CursorIcon, Id, IdSalt, Key, Modifiers, Rangef, Rect, Response, Sense, Stroke, Ui, UiBuilder,
    pos2, vec2,
};

/// Thickness of the grab area around the line; the visible line stays 1 point.
const GRAB_WIDTH: f32 = 8.0;
/// Thickness of the accent bar while hovered, focused or dragged.
const ACTIVE_WIDTH: f32 = 2.0;
const KEY_STEP: f32 = 8.0;
const KEY_STEP_LARGE: f32 = 32.0;

/// Space the pane on the far side of a boundary always keeps, as a share of the available width.
const FAR_SIDE_SHARE: f32 = 0.05;
/// ... and never less than this many points, so the handle stays easy to grab.
const FAR_SIDE_MIN: f32 = 48.0;

/// Range for the width of the pane next to a boundary: at least `min`, at most `available` minus
/// what the other side keeps (5 %, at least 48 points), so the boundary can be dragged almost to
/// the edge and still be grabbed again. Never below `min`.
pub fn pane_range(min: f32, available: f32) -> RangeInclusive<f32> {
    let reserved = (available * FAR_SIDE_SHARE).max(FAR_SIDE_MIN);
    min..=(available - reserved).max(min)
}

/// The same limit for a boundary stored as the first pane's share of `width`: each pane keeps 5 %
/// of the width, at least 48 points (but never more than half).
pub fn share_range(width: f32) -> RangeInclusive<f32> {
    let width = width.max(1.0);
    let edge = ((width * FAR_SIDE_SHARE).max(FAR_SIDE_MIN) / width).min(0.5);
    edge..=(1.0 - edge)
}

pub struct Splitter {
    id_salt: IdSalt,
    /// Built every frame from a literal at almost every call site: borrowed, not allocated.
    label: Cow<'static, str>,
    default_value: f32,
    range: RangeInclusive<f32>,
    per_point: f32,
    draws_line: bool,
}

#[derive(Debug)]
pub struct SplitterResponse {
    /// The grab area's response (`dragged()` while the user moves the boundary).
    pub response: Response,
    /// The user finished an adjustment (drag released, reset, key press): time to persist.
    pub committed: bool,
    /// The adjustment was a reset to `default_value` (double-click or Enter), for a caller whose
    /// default is not a fixed number.
    pub is_reset: bool,
}

/// Where a drag started: the value and the pointer's coordinate along the axis at that moment.
#[derive(Clone, Copy)]
struct DragStart {
    value: f32,
    pointer: f32,
}

/// Which way the boundary lies.
#[derive(Clone, Copy, PartialEq)]
enum Axis {
    /// Upright, between panes side by side: the pointer's x moves it.
    Vertical,
    /// Flat, between stacked panes: the pointer's y moves it.
    Horizontal,
}

impl Axis {
    /// The coordinate of `pos` that moves the boundary.
    fn along(self, pos: egui::Pos2) -> f32 {
        match self {
            Self::Vertical => pos.x,
            Self::Horizontal => pos.y,
        }
    }

    /// The band `thickness` across around the boundary at `at`, spanning `span`.
    fn band(self, at: f32, thickness: f32, span: Rangef) -> Rect {
        let across = Rangef::new(at - thickness / 2.0, at + thickness / 2.0);
        match self {
            Self::Vertical => Rect::from_x_y_ranges(across, span),
            Self::Horizontal => Rect::from_x_y_ranges(span, across),
        }
    }
}

impl Splitter {
    /// `label` names what the boundary resizes, for screen readers and the tooltip.
    pub fn new(id_salt: impl egui::AsIdSalt, label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            id_salt: IdSalt::new(id_salt),
            label: label.into(),
            default_value: 0.5,
            range: 0.0..=f32::INFINITY,
            per_point: 1.0,
            draws_line: true,
        }
    }

    #[must_use]
    pub fn default_value(mut self, value: f32) -> Self {
        self.default_value = value;
        self
    }

    /// Smallest and largest value; also bounds keyboard steps and the reset.
    #[must_use]
    pub fn range(mut self, range: RangeInclusive<f32>) -> Self {
        self.range = range;
        self
    }

    /// Value change per point the pointer moves to the right (down, for a horizontal boundary).
    #[must_use]
    pub fn per_point(mut self, per_point: f32) -> Self {
        self.per_point = per_point;
        self
    }

    /// Whether the resting hairline is painted (off where the panes draw their own border or a gap).
    #[must_use]
    pub fn draws_line(mut self, draws_line: bool) -> Self {
        self.draws_line = draws_line;
        self
    }

    /// The upright boundary at `x`, spanning `y_range`; reads and writes `value`.
    pub fn show(self, ui: &mut Ui, x: f32, y_range: Rangef, value: &mut f32) -> SplitterResponse {
        self.show_on(ui, Axis::Vertical, x, y_range, value)
    }

    /// The flat boundary at `y`, spanning `x_range`; reads and writes `value`.
    pub fn show_horizontal(
        self,
        ui: &mut Ui,
        y: f32,
        x_range: Rangef,
        value: &mut f32,
    ) -> SplitterResponse {
        self.show_on(ui, Axis::Horizontal, y, x_range, value)
    }

    fn show_on(
        self,
        ui: &mut Ui,
        axis: Axis,
        at: f32,
        span: Rangef,
        value: &mut f32,
    ) -> SplitterResponse {
        let id = ui.make_persistent_id(self.id_salt);
        let (min, max) = self.limits();
        let grab = axis.band(at, GRAB_WIDTH, span);
        let response = ui.interact(grab, id, Sense::click_and_drag());
        let before = *value;
        let mut committed = false;
        let mut is_reset = false;

        // egui reports the drag once the pointer passed its threshold; measure from the press.
        if response.drag_started()
            && let Some(pointer) =
                ui.input(|input| input.pointer.press_origin()).or(response.interact_pointer_pos())
        {
            ui.data_mut(|data| {
                data.insert_temp(id, DragStart { value: *value, pointer: axis.along(pointer) })
            });
        }
        if response.dragged()
            && let (Some(start), Some(pointer)) =
                (ui.data(|data| data.get_temp::<DragStart>(id)), response.interact_pointer_pos())
        {
            let travel = axis.along(pointer) - start.pointer;
            *value = (start.value + travel * self.per_point).clamp(min, max);
        }
        if response.drag_stopped() {
            ui.data_mut(|data| data.remove::<DragStart>(id));
            committed = true;
        }
        if response.double_clicked() {
            *value = self.default_value.clamp(min, max);
            committed = true;
            is_reset = true;
        }
        if response.has_focus()
            && let Some(reset) = self.keyboard(ui, axis, value)
        {
            committed = true;
            is_reset |= reset;
        }

        let is_active = response.hovered()
            || response.dragged()
            || (response.has_focus() && crate::focus::is_visible(ui.ctx()));
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(match axis {
                Axis::Vertical => CursorIcon::ResizeColumn,
                Axis::Horizontal => CursorIcon::ResizeRow,
            });
        }
        // Painted where the boundary is now, so it never trails the pointer by a frame.
        let moved = if self.per_point == 0.0 { 0.0 } else { (*value - before) / self.per_point };
        self.paint(ui, id, axis, at + moved, span, is_active);
        let label = self.label;
        let shown = *value;
        response.widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(egui::Role::Splitter, true, &label);
            info.value = Some(f64::from(shown));
            info
        });
        let response = if response.dragged() {
            response
        } else {
            response.on_hover_text("Drag to resize, double-click to reset")
        };
        SplitterResponse { response, committed, is_reset }
    }

    /// Two columns around a boundary inside `ui`, separated by `gap` points. `share` is the left
    /// column's share of the width without the gap. The boundary spans the taller column.
    pub fn show_columns(
        self,
        ui: &mut Ui,
        share: &mut f32,
        gap: f32,
        left: impl FnOnce(&mut Ui),
        right: impl FnOnce(&mut Ui),
    ) -> SplitterResponse {
        let available = ui.available_rect_before_wrap();
        let width = (available.width() - gap).max(1.0);
        let range = share_range(width);
        let left_width = (width * share.clamp(*range.start(), *range.end())).round();
        let top = available.top();
        let left_rect = Rect::from_min_size(available.min, vec2(left_width, available.height()));
        let right_rect = Rect::from_min_max(pos2(left_rect.right() + gap, top), available.max);
        let column = |rect: Rect| {
            UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min))
        };
        let left_height = ui.scope_builder(column(left_rect), left).response.rect.height();
        let right_height = ui.scope_builder(column(right_rect), right).response.rect.height();
        let height = left_height.max(right_height);
        ui.allocate_rect(
            Rect::from_min_size(available.min, vec2(available.width(), height)),
            Sense::hover(),
        );
        let x = left_rect.right() + gap / 2.0;
        self.range(range).per_point(1.0 / width).show(ui, x, Rangef::new(top, top + height), share)
    }

    fn limits(&self) -> (f32, f32) {
        let (min, max) = (*self.range.start(), *self.range.end());
        (min.min(max), max.max(min))
    }

    /// Arrow keys, Home/End and Enter while the boundary has keyboard focus: whether one moved it,
    /// and whether that was the reset.
    fn keyboard(&self, ui: &Ui, axis: Axis, value: &mut f32) -> Option<bool> {
        let (min, max) = self.limits();
        let direction = self.per_point.signum();
        let step = self.per_point.abs();
        let (forward, back) = match axis {
            Axis::Vertical => (Key::ArrowRight, Key::ArrowLeft),
            Axis::Horizontal => (Key::ArrowDown, Key::ArrowUp),
        };
        let target = ui.input_mut(|input| {
            let mut target = None;
            for (modifiers, distance) in
                [(Modifiers::NONE, KEY_STEP), (Modifiers::SHIFT, KEY_STEP_LARGE)]
            {
                if input.consume_key(modifiers, forward) {
                    target = Some((*value + distance * step * direction, false));
                }
                if input.consume_key(modifiers, back) {
                    target = Some((*value - distance * step * direction, false));
                }
            }
            if input.consume_key(Modifiers::NONE, Key::Home) {
                target = Some((if direction > 0.0 { min } else { max }, false));
            }
            if input.consume_key(Modifiers::NONE, Key::End) {
                target = Some((if direction > 0.0 { max } else { min }, false));
            }
            if input.consume_key(Modifiers::NONE, Key::Enter) {
                target = Some((self.default_value, true));
            }
            target
        });
        let (target, is_reset) = target?;
        *value = target.clamp(min, max);
        Some(is_reset)
    }

    fn paint(&self, ui: &Ui, id: Id, axis: Axis, at: f32, span: Rangef, is_active: bool) {
        let palette = theme::palette(ui.ctx());
        let painter = ui.painter();
        let at = at.round();
        if self.draws_line {
            let stroke = Stroke::new(1.0, palette.border);
            match axis {
                Axis::Vertical => painter.vline(at - 0.5, span, stroke),
                Axis::Horizontal => painter.hline(span, at - 0.5, stroke),
            };
        }
        let amount = motion::toggle(ui.ctx(), id.with("active"), is_active, motion::FAST);
        if amount > 0.0 {
            let bar = axis.band(at, ACTIVE_WIDTH, span);
            painter.rect_filled(bar, 0, palette.accent.gamma_multiply(amount));
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Modifiers, PointerButton, Pos2, pos2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::{Axis, Splitter, pane_range, share_range};

    struct State {
        width: f32,
        committed: usize,
        resets: usize,
    }

    /// A splitter across the whole window along `axis`, `width` from the near edge.
    fn harness(width: f32, axis: Axis) -> Harness<'static, State> {
        let size = match axis {
            Axis::Vertical => [800.0, 400.0],
            Axis::Horizontal => [400.0, 800.0],
        };
        Harness::builder()
            .with_size(size)
            .with_step_dt(1.0 / 60.0)
            .with_max_steps(240)
            .build_ui_state(
                move |ui, state: &mut State| {
                    let rect = ui.max_rect();
                    let splitter = Splitter::new("test", "Resize test pane")
                        .default_value(300.0)
                        .range(100.0..=600.0);
                    let response = match axis {
                        Axis::Vertical => splitter.show(
                            ui,
                            rect.left() + state.width,
                            rect.y_range(),
                            &mut state.width,
                        ),
                        Axis::Horizontal => splitter.show_horizontal(
                            ui,
                            rect.top() + state.width,
                            rect.x_range(),
                            &mut state.width,
                        ),
                    };
                    state.committed += usize::from(response.committed);
                    state.resets += usize::from(response.is_reset);
                },
                State { width, committed: 0, resets: 0 },
            )
    }

    fn handle(harness: &Harness<'_, State>) -> Pos2 {
        harness.get_by_label("Resize test pane").rect().center()
    }

    fn press(harness: &mut Harness<'_, State>, pos: Pos2, pressed: bool) {
        harness.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.run();
    }

    #[test]
    fn pane_range_leaves_the_far_side_reachable() {
        assert_eq!(pane_range(220.0, 1000.0), 220.0..=950.0);
        assert_eq!(pane_range(220.0, 2000.0), 220.0..=1900.0);
        // Tiny window: the far side keeps 48 points, the near side its minimum.
        assert_eq!(pane_range(220.0, 300.0), 220.0..=252.0);
        // The minimum wins over the far side's reserve.
        assert_eq!(pane_range(220.0, 200.0), 220.0..=220.0);
    }

    #[test]
    fn share_range_keeps_both_panes_reachable() {
        let wide = share_range(2000.0);
        assert!((wide.start() - 0.05).abs() < 1e-6 && (wide.end() - 0.95).abs() < 1e-6);
        let narrow = share_range(400.0);
        assert!((narrow.start() - 0.12).abs() < 1e-6 && (narrow.end() - 0.88).abs() < 1e-6);
        let tiny = share_range(60.0);
        assert_eq!((*tiny.start(), *tiny.end()), (0.5, 0.5));
    }

    #[test]
    fn dragging_moves_the_boundary_within_its_range() {
        let mut harness = harness(300.0, Axis::Vertical);
        harness.run();
        let start = handle(&harness);
        harness.hover_at(start);
        harness.run();
        press(&mut harness, start, true);
        harness.hover_at(start + egui::vec2(60.0, 0.0));
        harness.run();
        harness.hover_at(start + egui::vec2(120.0, 0.0));
        harness.run();
        press(&mut harness, start + egui::vec2(120.0, 0.0), false);
        assert_eq!(harness.state().width, 420.0);
        assert_eq!(harness.state().committed, 1);

        let start = handle(&harness);
        press(&mut harness, start, true);
        harness.hover_at(pos2(2000.0, start.y));
        harness.run();
        press(&mut harness, pos2(2000.0, start.y), false);
        assert_eq!(harness.state().width, 600.0, "clamped to the maximum");
    }

    #[test]
    fn double_click_and_enter_reset_to_the_default() {
        let mut harness = harness(480.0, Axis::Vertical);
        harness.run();
        let center = handle(&harness);
        harness.hover_at(center);
        harness.run();
        // Both clicks within egui's double-click window: one frame per event.
        for pressed in [true, false, true, false] {
            harness.event(Event::PointerButton {
                pos: center,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        harness.step();
        harness.run();
        assert_eq!(harness.state().width, 300.0);
        assert_eq!(harness.state().committed, 1);
        assert_eq!(harness.state().resets, 1, "the double-click is a reset");

        harness.state_mut().width = 200.0;
        harness.get_by_label("Resize test pane").focus();
        harness.run();
        harness.key_press(egui::Key::ArrowRight);
        harness.run();
        assert_eq!(harness.state().width, 208.0);
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(harness.state().width, 300.0);
    }

    /// The flat boundary follows the pointer's y, clamps at both ends and steps with ↑/↓.
    #[test]
    fn a_horizontal_boundary_moves_with_the_pointer_and_the_arrow_keys() {
        let mut harness = harness(300.0, Axis::Horizontal);
        harness.run();
        let start = handle(&harness);
        harness.hover_at(start);
        harness.run();
        press(&mut harness, start, true);
        harness.hover_at(start + egui::vec2(0.0, 100.0));
        harness.run();
        press(&mut harness, start + egui::vec2(0.0, 100.0), false);
        assert_eq!(harness.state().width, 400.0);
        assert_eq!(harness.state().committed, 1);

        for (to, expected) in [(2000.0, 600.0), (-2000.0, 100.0)] {
            let start = handle(&harness);
            press(&mut harness, start, true);
            harness.hover_at(pos2(start.x, to));
            harness.run();
            press(&mut harness, pos2(start.x, to), false);
            assert_eq!(harness.state().width, expected, "clamped");
        }

        harness.get_by_label("Resize test pane").focus();
        harness.run();
        harness.key_press(egui::Key::ArrowDown);
        harness.run();
        assert_eq!(harness.state().width, 108.0);
        harness.key_press(egui::Key::ArrowUp);
        harness.run();
        assert_eq!(harness.state().width, 100.0);
        harness.key_press(egui::Key::End);
        harness.run();
        assert_eq!(harness.state().width, 600.0);
        assert_eq!(harness.state().resets, 0, "steps are no reset");
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!((harness.state().width, harness.state().resets), (300.0, 1), "Enter resets");
    }
}
