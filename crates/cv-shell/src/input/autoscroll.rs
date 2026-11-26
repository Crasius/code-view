//! Middle-click autoscroll, as in Chrome and Edge on Windows.
//!
//! A middle click on the page content drops an origin marker; the content under the marker then
//! scrolls at a speed that grows with the pointer's distance from it (a small dead zone first,
//! then roughly linear, then faster). A canvas that pans itself (the stage graph) keeps its
//! middle-button drags. A short click makes the mode sticky until any click, Esc or
//! a wheel turn; pressing, dragging and releasing scrolls only while the button is held.
//!
//! It works on raw input, like `smooth_wheel`: while active, egui keeps seeing the pointer at the
//! origin, so the scroll area (or the virtualized diff) under the origin receives the scrolling,
//! not whatever the pointer passes over. The scrolling itself is a continuous wheel gesture
//! (start, moves, end) that every scroll area already understands. Repaints are requested only
//! while the mode is active.

use egui::{
    Color32, Context, CursorIcon, Event, Id, Key, LayerId, Modifiers, MouseWheelSource,
    MouseWheelUnit, Order, PointerButton, Pos2, RawInput, Rect, Stroke, TouchPhase, Vec2, vec2,
};

use cv_base::theme;
use cv_widgets::surfaces;

/// Pointer distance from the origin, per axis, that does not scroll yet.
const DEAD_ZONE: f32 = 12.0;
/// Points per second at the far end; also a safety cap.
const MAX_SPEED: f32 = 8_000.0;
const MARKER_RADIUS: f32 = 15.0;
const FALLBACK_DT: f32 = 1.0 / 60.0;

#[derive(Default)]
pub struct Autoscroll {
    active: Option<Active>,
    /// Where a middle click may start the mode: the page content, without rail and top bar.
    content: Option<Rect>,
    /// Last real pointer position.
    pointer: Option<Pos2>,
    /// The release of the click that ended the mode, kept from egui so it clicks nothing.
    swallow_release: Option<PointerButton>,
    last_time: Option<f64>,
}

#[derive(Clone, Copy)]
struct Active {
    origin: Pos2,
    pointer: Pos2,
    middle: Middle,
    is_horizontal: bool,
    has_started_gesture: bool,
}

/// The middle button since the click that started the mode.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Middle {
    /// Still down, the pointer within the dead zone: releasing keeps the mode on.
    Held,
    /// Still down, the pointer left the dead zone: releasing ends the mode.
    Dragged,
    /// Released: the mode runs until the next click.
    Released,
}

impl Autoscroll {
    /// The area where a middle click starts autoscroll, from the last frame's layout.
    pub fn set_content_rect(&mut self, rect: Rect) {
        self.content = Some(rect);
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Rewrites this frame's raw input: starts and ends the mode and adds the scroll step.
    pub fn process(&mut self, ctx: &Context, raw_input: &mut RawInput) {
        let dt = self.frame_time(raw_input);
        let events = std::mem::take(&mut raw_input.events);
        let mut kept = Vec::with_capacity(events.len() + 2);
        for event in events {
            self.handle(ctx, event, &mut kept);
        }
        if let Some(active) = self.active.as_mut() {
            if !active.has_started_gesture {
                kept.push(wheel(Vec2::ZERO, TouchPhase::Start));
                active.has_started_gesture = true;
            }
            kept.push(wheel(step(active, dt), TouchPhase::Move));
        }
        raw_input.events = kept;
    }

    /// Paints the origin marker and sets the direction cursor; call at the end of the frame.
    pub fn show(&self, ctx: &Context) {
        let Some(active) = self.active else {
            return;
        };
        ctx.request_repaint();
        ctx.set_cursor_icon(cursor(&active));
        paint_marker(ctx, &active);
    }

    fn handle(&mut self, ctx: &Context, event: Event, kept: &mut Vec<Event>) {
        match event {
            Event::PointerMoved(pos) => {
                self.pointer = Some(pos);
                match self.active.as_mut() {
                    Some(active) => {
                        active.pointer = pos;
                        let offset = pos - active.origin;
                        let is_beyond_dead_zone = offset.x.abs().max(offset.y.abs()) > DEAD_ZONE;
                        if active.middle == Middle::Held && is_beyond_dead_zone {
                            active.middle = Middle::Dragged;
                        }
                    }
                    None => kept.push(event),
                }
            }
            Event::PointerButton { button, pressed, pos, .. } => {
                if !pressed && self.swallow_release == Some(button) {
                    self.swallow_release = None;
                    return;
                }
                let Some(active) = self.active.as_mut() else {
                    let is_autoscroll_area =
                        self.content.is_some_and(|content| content.contains(pos))
                            && !surfaces::pans_at(ctx, pos)
                            && !surfaces::takes_middle_click_at(ctx, pos);
                    if button == PointerButton::Middle && pressed && is_autoscroll_area {
                        self.start(ctx, pos);
                    } else {
                        kept.push(event);
                    }
                    return;
                };
                let is_middle_release = button == PointerButton::Middle && !pressed;
                if is_middle_release && active.middle == Middle::Dragged {
                    self.stop(kept);
                } else if is_middle_release && active.middle == Middle::Held {
                    active.middle = Middle::Released;
                } else if pressed {
                    self.stop(kept);
                    self.swallow_release = Some(button);
                } else {
                    kept.push(event);
                }
            }
            Event::Key { key: Key::Escape, pressed: true, .. } if self.active.is_some() => {
                self.stop(kept);
            }
            Event::MouseWheel { .. } | Event::PointerGone | Event::WindowFocused(false)
                if self.active.is_some() =>
            {
                self.stop(kept);
                kept.push(event);
            }
            _ => kept.push(event),
        }
    }

    fn start(&mut self, ctx: &Context, origin: Pos2) {
        self.active = Some(Active {
            origin,
            pointer: origin,
            middle: Middle::Held,
            is_horizontal: surfaces::scrolls_horizontally_at(ctx, origin),
            has_started_gesture: false,
        });
        ctx.request_repaint();
    }

    /// Ends the scroll gesture and hands egui the real pointer position again.
    fn stop(&mut self, kept: &mut Vec<Event>) {
        let Some(active) = self.active.take() else {
            return;
        };
        if active.has_started_gesture {
            kept.push(wheel(Vec2::ZERO, TouchPhase::End));
        }
        kept.push(Event::PointerMoved(self.pointer.unwrap_or(active.pointer)));
    }

    fn frame_time(&mut self, raw_input: &RawInput) -> f32 {
        let now = raw_input.time;
        let dt = match (now, self.last_time) {
            (Some(now), Some(last)) => (now - last) as f32,
            _ => raw_input.predicted_dt,
        };
        self.last_time = now;
        if dt.is_finite() && dt > 0.0 { dt.min(0.1) } else { FALLBACK_DT }
    }
}

/// Signed points per second for a pointer `distance` from the origin along one axis.
fn speed(distance: f32) -> f32 {
    let beyond = (distance.abs() - DEAD_ZONE).max(0.0);
    (beyond * 4.0 + beyond * beyond * 0.03).min(MAX_SPEED) * distance.signum()
}

/// This frame's wheel delta: content moves away from the pointer's side, like dragging a page.
fn step(active: &Active, dt: f32) -> Vec2 {
    let offset = active.pointer - active.origin;
    let x = if active.is_horizontal { speed(offset.x) } else { 0.0 };
    -vec2(x, speed(offset.y)) * dt
}

fn wheel(delta: Vec2, phase: TouchPhase) -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta,
        phase,
        modifiers: Modifiers::NONE,
        source: MouseWheelSource::Wheel,
    }
}

/// Which way the pointer points beyond the dead zone: -1, 0 or 1 per axis.
fn direction(active: &Active) -> (i8, i8) {
    let offset = active.pointer - active.origin;
    let sign = |value: f32| if value.abs() <= DEAD_ZONE { 0 } else { value.signum() as i8 };
    (if active.is_horizontal { sign(offset.x) } else { 0 }, sign(offset.y))
}

fn cursor(active: &Active) -> CursorIcon {
    match direction(active) {
        (0, 0) if active.is_horizontal => CursorIcon::AllScroll,
        (0, 0) => CursorIcon::ResizeVertical,
        (0, -1) => CursorIcon::ResizeNorth,
        (0, _) => CursorIcon::ResizeSouth,
        (-1, 0) => CursorIcon::ResizeWest,
        (_, 0) => CursorIcon::ResizeEast,
        (-1, -1) => CursorIcon::ResizeNorthWest,
        (_, -1) => CursorIcon::ResizeNorthEast,
        (-1, _) => CursorIcon::ResizeSouthWest,
        _ => CursorIcon::ResizeSouthEast,
    }
}

/// A round marker with arrows for every direction it can scroll; the moving direction is accented.
fn paint_marker(ctx: &Context, active: &Active) {
    let palette = theme::palette(ctx);
    let painter =
        ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("autoscroll_marker")));
    let center = active.origin;
    let bounds = Rect::from_center_size(center, Vec2::splat(MARKER_RADIUS * 2.0));
    // Lifted off the content like a popup, so it reads on code, cards and plain ground alike.
    painter.add(theme::style::popup_shadow(&palette).as_shape(bounds, MARKER_RADIUS));
    painter.circle(center, MARKER_RADIUS, palette.surface, Stroke::new(1.0, palette.border_strong));
    painter.circle_filled(center, 2.0, palette.text_secondary);
    let (horizontal, vertical) = direction(active);
    let mut arrows: Vec<(Vec2, bool)> =
        vec![(vec2(0.0, -1.0), vertical < 0), (vec2(0.0, 1.0), vertical > 0)];
    if active.is_horizontal {
        arrows.extend([(vec2(-1.0, 0.0), horizontal < 0), (vec2(1.0, 0.0), horizontal > 0)]);
    }
    for (along, is_moving) in arrows {
        let color = if is_moving { palette.accent } else { palette.text };
        painter.add(arrow(center, along, color));
    }
}

/// A small filled triangle pointing along `along`, inside the marker.
fn arrow(center: Pos2, along: Vec2, color: Color32) -> egui::Shape {
    let across = vec2(-along.y, along.x);
    let tip = center + along * 11.0;
    let base = center + along * 6.5;
    egui::Shape::convex_polygon(
        vec![tip, base + across * 4.0, base - across * 4.0],
        color,
        Stroke::NONE,
    )
}

/// Where egui should think the pointer is: the origin while active.
#[cfg(test)]
fn origin(autoscroll: &Autoscroll) -> Option<Pos2> {
    autoscroll.active.map(|active| active.origin)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use egui::{Event, Key, Modifiers, PointerButton, Pos2, ViewportId, pos2, vec2};
    use egui_kittest::Harness;

    use super::{Autoscroll, origin};

    struct State {
        autoscroll: Autoscroll,
        offset: f32,
    }

    /// A tall list in a scroll area that fills the window.
    fn harness() -> Harness<'static, State> {
        harness_with(false)
    }

    /// `is_pan_canvas`: the list is marked as a canvas that pans itself.
    fn harness_with(is_pan_canvas: bool) -> Harness<'static, State> {
        let state = State { autoscroll: Autoscroll::default(), offset: 0.0 };
        Harness::builder().with_size([400.0, 400.0]).build_ui_state(
            move |ui, state: &mut State| {
                state.autoscroll.set_content_rect(ui.ctx().content_rect());
                if is_pan_canvas {
                    cv_widgets::surfaces::mark_pan(ui, ui.max_rect());
                }
                let output = egui::ScrollArea::vertical().id_salt("list").auto_shrink(false).show(
                    ui,
                    |ui| {
                        for row in 0..2_000 {
                            ui.label(format!("Row {row}"));
                        }
                    },
                );
                state.offset = output.state.clamped_offset().y;
                state.autoscroll.show(ui.ctx());
            },
            state,
        )
    }

    /// One frame with `events`, through the raw-input step the app runs in `raw_input_hook`.
    fn frame(harness: &mut Harness<'_, State>, events: Vec<Event>) {
        let ctx = harness.ctx.clone();
        let mut input = std::mem::take(harness.input_mut());
        input.events.extend(events);
        harness.state_mut().autoscroll.process(&ctx, &mut input);
        *harness.input_mut() = input;
        harness.step();
    }

    fn middle(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Middle,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn repaint_delay(harness: &Harness<'_, State>) -> Duration {
        harness.output().viewport_output[&ViewportId::ROOT].repaint_delay
    }

    #[test]
    fn middle_click_then_pointer_offset_scrolls_the_area_under_the_origin() {
        let mut harness = harness();
        let start = pos2(200.0, 200.0);
        frame(&mut harness, vec![Event::PointerMoved(start)]);
        frame(&mut harness, vec![middle(start, true)]);
        frame(&mut harness, vec![middle(start, false)]);
        assert_eq!(
            origin(&harness.state().autoscroll),
            Some(start),
            "a short click makes the mode sticky"
        );
        frame(&mut harness, vec![Event::PointerMoved(start + vec2(0.0, 120.0))]);
        let mut previous = harness.state().offset;
        for _ in 0..20 {
            frame(&mut harness, Vec::new());
            let offset = harness.state().offset;
            assert!(offset > previous, "keeps scrolling down: {previous} -> {offset}");
            previous = offset;
        }
        assert_eq!(repaint_delay(&harness), Duration::ZERO, "repaints while active");

        // Moving further away scrolls faster.
        let slow = {
            let before = harness.state().offset;
            frame(&mut harness, Vec::new());
            harness.state().offset - before
        };
        frame(&mut harness, vec![Event::PointerMoved(start + vec2(0.0, 190.0))]);
        frame(&mut harness, Vec::new());
        let before = harness.state().offset;
        frame(&mut harness, Vec::new());
        assert!(harness.state().offset - before > slow * 1.5);
    }

    #[test]
    fn escape_stops_and_idle_requests_no_repaints() {
        let mut harness = harness();
        let start = pos2(200.0, 200.0);
        frame(&mut harness, vec![Event::PointerMoved(start)]);
        frame(&mut harness, vec![middle(start, true), middle(start, false)]);
        frame(&mut harness, vec![Event::PointerMoved(start + vec2(0.0, 150.0))]);
        for _ in 0..5 {
            frame(&mut harness, Vec::new());
        }
        assert!(harness.state().offset > 0.0);

        let escape = Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        frame(&mut harness, vec![escape]);
        assert!(!harness.state().autoscroll.is_active());
        for _ in 0..3 {
            frame(&mut harness, Vec::new());
        }
        let stopped = harness.state().offset;
        for _ in 0..10 {
            frame(&mut harness, Vec::new());
        }
        assert_eq!(harness.state().offset, stopped, "no scrolling after Esc");
        assert_eq!(repaint_delay(&harness), Duration::MAX, "an idle frame asks for no repaint");
    }

    #[test]
    fn a_pan_canvas_keeps_its_middle_drags() {
        let mut harness = harness_with(true);
        let start = pos2(200.0, 200.0);
        frame(&mut harness, vec![Event::PointerMoved(start)]);
        frame(&mut harness, vec![middle(start, true)]);
        assert!(!harness.state().autoscroll.is_active(), "no autoscroll on a pan canvas");
        assert!(
            harness.ctx.input(|input| input.pointer.middle_down()),
            "egui sees the middle press"
        );
        frame(&mut harness, vec![middle(start, false)]);
    }

    #[test]
    fn hold_and_drag_scrolls_until_release() {
        let mut harness = harness();
        let start = pos2(200.0, 200.0);
        frame(&mut harness, vec![Event::PointerMoved(start)]);
        frame(&mut harness, vec![middle(start, true)]);
        frame(&mut harness, vec![Event::PointerMoved(start + vec2(0.0, 100.0))]);
        for _ in 0..5 {
            frame(&mut harness, Vec::new());
        }
        assert!(harness.state().offset > 0.0);
        frame(&mut harness, vec![middle(start + vec2(0.0, 100.0), false)]);
        assert!(!harness.state().autoscroll.is_active(), "releasing after a drag ends the mode");
    }

    #[test]
    fn a_click_ends_sticky_mode_without_clicking_through() {
        let mut harness = harness();
        let start = pos2(200.0, 200.0);
        frame(&mut harness, vec![Event::PointerMoved(start)]);
        frame(&mut harness, vec![middle(start, true), middle(start, false)]);
        let primary = |pressed| Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(&mut harness, vec![primary(true)]);
        assert!(!harness.state().autoscroll.is_active());
        frame(&mut harness, vec![primary(false)]);
        assert!(
            !harness.ctx.input(|input| input.pointer.any_click()),
            "the ending click is swallowed"
        );
    }
}
