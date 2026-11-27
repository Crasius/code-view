//! Moving the window from egui, through the system's own window drag.
//!
//! [`DragArea`]: an app-drawn title bar where the system has no hit-test hook for it (macOS).
//! Pressing on the bar and dragging starts the drag, a double click zooms the window (maximizes or
//! restores it). Presses on the bar's widgets (the holes) stay theirs, as on the Windows title bar.
//!
//! [`long_press_moves_window`]: a widget on the bar (a breadcrumb) that leads somewhere on a click
//! but moves the window when held still for [`LONG_PRESS`]. On Windows such widgets are hit-test
//! holes, so egui sees the press; everywhere the drag goes through the same
//! [`ViewportCommand::StartDrag`]. On Windows the system's move loop swallows the release and winit
//! posts it once the window is let go, so the widget still sees a click: this module swallows it.

use std::time::Duration;

use egui::{CursorIcon, Id, PointerButton, Response, ViewportCommand};
#[cfg(any(target_os = "macos", test))]
use egui::{Rect, Sense, Ui};

/// How long a press on a bar widget rests, without moving past egui's click distance, before it
/// moves the window instead of clicking the widget. Below egui's `max_click_duration` (0.8 s), so a
/// press released just before it is still a click.
pub const LONG_PRESS: Duration = Duration::from_millis(350);

/// The bar's pointer sense, from before its widgets were laid out until they are known.
#[cfg(any(target_os = "macos", test))]
pub(crate) struct DragArea {
    response: Response,
}

#[cfg(any(target_os = "macos", test))]
impl DragArea {
    /// Senses the pointer on `bar`. Call before laying out the bar's widgets, so they take clicks
    /// over their own rects.
    pub fn sense(ui: &Ui, bar: Rect) -> Self {
        // Not focusable: the bar is no control that Tab should stop at.
        let response =
            ui.interact(bar, ui.scope_id().with("window_drag"), Sense::CLICK | Sense::DRAG);
        ui.ctx().data_mut(|data| data.insert_temp(drag_area_mark(response.id), true));
        Self { response }
    }

    /// Moves or zooms the window, unless the press began on one of `holes` (the widgets on the bar:
    /// egui hands a drag that starts on a click-only widget to the area below it). Returns the
    /// command sent to the window.
    pub(crate) fn apply(self, holes: &[Rect]) -> Option<ViewportCommand> {
        let ctx = &self.response.ctx;
        // A release (the second click) has no press origin left; the pointer is still where it was.
        let origin =
            ctx.input(|input| input.pointer.press_origin().or(input.pointer.interact_pos()))?;
        if holes.iter().any(|hole| hole.contains(origin)) {
            return None;
        }
        let command = if self.response.double_clicked_by(PointerButton::Primary) {
            let is_maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
            ViewportCommand::Maximized(!is_maximized)
        } else if self.response.drag_started_by(PointerButton::Primary) {
            ViewportCommand::StartDrag
        } else {
            return None;
        };
        ctx.send_viewport_cmd(command.clone());
        Some(command)
    }
}

fn drag_area_mark(id: Id) -> Id {
    id.with("is_window_drag_area")
}

/// `id` is a [`DragArea`]: it senses clicks for the double click that zooms, but it is the title
/// bar, no control, so it keeps the arrow cursor instead of the pointing hand.
pub(crate) fn is_drag_area(ctx: &egui::Context, id: Id) -> bool {
    ctx.data(|data| data.get_temp::<bool>(drag_area_mark(id))).unwrap_or(false)
}

/// The press the long-press check follows; one at a time, so one slot for the whole app.
#[derive(Clone, Copy)]
struct Press {
    widget: Id,
    /// The press's start (egui's input time): a new press on the same widget starts over.
    start: f64,
    phase: Phase,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Held still, waiting for [`LONG_PRESS`].
    Pending,
    /// Moved past the click distance first: neither a click nor a window move.
    Moved,
    /// Held long enough: the window moves, and the press is no click.
    Moving,
}

fn press_id() -> Id {
    Id::unique("window_drag_long_press")
}

/// Follows a press on `response` (a widget sensing clicks or drags): held still for
/// [`LONG_PRESS`], it sends [`ViewportCommand::StartDrag`] once. Returns true from then until the
/// release frame included: the caller drops its pressed and hover look and ignores the click.
/// While armed the cursor is the grabbing hand. Repaints only while a press is pending, once at
/// its deadline.
#[expect(clippy::float_cmp, reason = "the press's start time, taken unchanged from egui's input")]
pub fn long_press_moves_window(response: &Response) -> bool {
    let ctx = &response.ctx;
    let (start, now, origin, latest, is_down, is_released) = ctx.input(|input| {
        let pointer = &input.pointer;
        (
            pointer.press_start_time(),
            input.time,
            pointer.press_origin(),
            pointer.latest_pos(),
            pointer.button_down(PointerButton::Primary),
            pointer.any_released(),
        )
    });
    let old = ctx.data(|data| data.get_temp::<Press>(press_id()));
    if !is_down && !is_released {
        if old.is_some_and(|press| press.widget == response.id) {
            ctx.data_mut(|data| data.remove::<Press>(press_id()));
        }
        return false;
    }
    let held = |press: &Press| press.widget == response.id && press.phase == Phase::Moving;
    if !(is_down && response.is_pointer_button_down_on()) {
        // The release frame: the press that moved the window clicks nothing.
        return is_released && old.as_ref().is_some_and(held);
    }
    let Some(start) = start else {
        return false;
    };
    let mut press = old
        .filter(|press| press.widget == response.id && press.start == start)
        .unwrap_or(Press { widget: response.id, start, phase: Phase::Pending });
    if press.phase == Phase::Pending {
        let max_click_dist = ctx.options(|options| options.input_options.max_click_dist);
        let has_moved = origin
            .zip(latest)
            .is_some_and(|(origin, latest)| origin.distance(latest) > max_click_dist);
        let left = LONG_PRESS.as_secs_f64() - (now - start);
        if has_moved {
            press.phase = Phase::Moved;
        } else if left <= 0.0 {
            press.phase = Phase::Moving;
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        } else {
            ctx.request_repaint_after(Duration::from_secs_f64(left));
        }
    }
    ctx.data_mut(|data| data.insert_temp(press_id(), press));
    let is_moving = held(&press);
    if is_moving {
        ctx.set_cursor_icon(CursorIcon::Grabbing);
    }
    is_moving
}

#[cfg(test)]
mod tests {
    use egui::{
        CursorIcon, Event, Modifiers, PointerButton, Pos2, Rect, ViewportCommand, pos2, vec2,
    };
    use egui_kittest::Harness;

    use super::DragArea;

    const BAR: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 40.0));
    /// A widget on the bar, laid out after the drag area.
    const BUTTON: Rect = Rect::from_min_max(pos2(300.0, 8.0), pos2(380.0, 32.0));

    fn harness() -> Harness<'static, Vec<ViewportCommand>> {
        Harness::builder().with_size([400.0, 200.0]).with_step_dt(1.0 / 60.0).build_ui_state(
            |ui, commands: &mut Vec<ViewportCommand>| {
                let area = DragArea::sense(ui, BAR);
                ui.place(BUTTON, egui::Button::new("Search"));
                commands.extend(area.apply(&[BUTTON]));
                crate::platform::pointer::pointing_hand_on_clickables(ui.ctx());
            },
            Vec::new(),
        )
    }

    fn press(harness: &mut Harness<'_, Vec<ViewportCommand>>, pos: Pos2, pressed: bool) {
        harness.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }

    fn drag(harness: &mut Harness<'_, Vec<ViewportCommand>>, from: Pos2) {
        harness.hover_at(from);
        harness.step();
        press(harness, from, true);
        harness.hover_at(from + vec2(40.0, 4.0));
        harness.step();
        press(harness, from + vec2(40.0, 4.0), false);
    }

    #[test]
    fn dragging_the_free_bar_starts_the_window_drag_once() {
        let mut harness = harness();
        harness.step();
        drag(&mut harness, pos2(100.0, 20.0));
        let drags = harness
            .state()
            .iter()
            .filter(|command| matches!(command, ViewportCommand::StartDrag))
            .count();
        assert_eq!(drags, 1, "{:?}", harness.state());
    }

    #[test]
    fn widgets_on_the_bar_keep_the_pointer() {
        let mut harness = harness();
        harness.step();
        drag(&mut harness, BUTTON.center());
        assert!(harness.state().is_empty(), "{:?}", harness.state());
    }

    #[test]
    fn the_free_bar_keeps_the_arrow_and_its_widgets_show_the_hand() {
        let mut harness = harness();
        let cursor = |harness: &mut Harness<'_, Vec<ViewportCommand>>, at: Pos2| {
            harness.hover_at(at);
            harness.run_steps(2);
            harness.output().platform_output.cursor_icon
        };
        assert_eq!(cursor(&mut harness, pos2(100.0, 20.0)), CursorIcon::Default);
        assert_eq!(cursor(&mut harness, BUTTON.center()), CursorIcon::PointingHand);
    }

    #[test]
    fn double_click_zooms_the_window() {
        let mut harness = harness();
        harness.step();
        let at = pos2(100.0, 20.0);
        harness.hover_at(at);
        harness.step();
        for pressed in [true, false, true, false] {
            press(&mut harness, at, pressed);
        }
        assert!(
            harness
                .state()
                .iter()
                .any(|command| matches!(command, ViewportCommand::Maximized(true))),
            "{:?}",
            harness.state()
        );
        assert!(
            !harness.state().iter().any(|command| matches!(command, ViewportCommand::StartDrag))
        );
    }
}
