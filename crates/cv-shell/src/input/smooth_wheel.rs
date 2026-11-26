//! Browser-like mouse wheel scrolling. A wheel notch scrolls a fixed distance, eased out over a
//! short glide instead of a hard step; notches that arrive mid-glide extend it, as in Edge or
//! Chrome. Trackpads already deliver smooth pixel deltas and pass through unchanged, as do
//! Ctrl+wheel zoom and everything under reduced motion.
//!
//! The glide is handed to egui as one continuous scroll gesture (start, moves, end), which egui
//! applies as-is, so every scroll area in the app gets it without knowing about it.

use egui::{Event, Modifiers, MouseWheelSource, MouseWheelUnit, RawInput, TouchPhase, Vec2};

/// Distance of one wheel notch, like a browser at 100 % zoom.
pub const NOTCH: f32 = 100.0;
/// Time constant of the exponential ease-out: about 95 % of a glide is covered in 3 × this.
const GLIDE_SECONDS: f32 = 0.06;
/// Below this many points left, the glide finishes in one step.
const SETTLE: f32 = 0.5;
/// Frame time assumed when the input carries no usable timing.
const FALLBACK_DT: f32 = 1.0 / 60.0;

#[derive(Default)]
pub struct SmoothWheel {
    /// Points still to scroll, in egui's wheel direction.
    remaining: Vec2,
    modifiers: Modifiers,
    source: Option<MouseWheelSource>,
    last_time: Option<f64>,
    is_gliding: bool,
}

impl SmoothWheel {
    /// Rewrites this frame's discrete wheel events into the next step of the glide.
    /// `is_enabled` is false under reduced motion.
    pub fn process(&mut self, raw_input: &mut RawInput, is_enabled: bool) {
        let dt = self.frame_time(raw_input);
        if !is_enabled {
            self.finish(&mut raw_input.events);
            return;
        }
        self.end_before_zoom(&mut raw_input.events);
        let mut wheel_started = false;
        raw_input.events.retain(|event| {
            let Event::MouseWheel { unit: MouseWheelUnit::Line, delta, modifiers, source, .. } =
                event
            else {
                return true;
            };
            if modifiers.command {
                return true;
            }
            self.remaining += *delta * NOTCH;
            self.modifiers = *modifiers;
            self.source = Some(*source);
            wheel_started = true;
            false
        });
        if !wheel_started && !self.is_gliding {
            return;
        }
        let step = self.step(dt);
        let source = self.source.unwrap_or(MouseWheelSource::Wheel);
        let modifiers = self.modifiers;
        let wheel = |delta: Vec2, phase: TouchPhase| Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta,
            phase,
            modifiers,
            source,
        };
        if !self.is_gliding {
            raw_input.events.push(wheel(Vec2::ZERO, TouchPhase::Start));
            self.is_gliding = true;
        }
        if step == Vec2::ZERO {
            raw_input.events.push(wheel(Vec2::ZERO, TouchPhase::End));
            self.is_gliding = false;
            self.source = None;
        } else {
            raw_input.events.push(wheel(step, TouchPhase::Move));
        }
    }

    /// True while a glide still has distance left; the app keeps repainting until it ends.
    pub fn is_active(&self) -> bool {
        self.is_gliding
    }

    fn step(&mut self, dt: f32) -> Vec2 {
        if self.remaining.length() <= SETTLE {
            return std::mem::take(&mut self.remaining);
        }
        let fraction = 1.0 - (-dt / GLIDE_SECONDS).exp();
        let step = self.remaining * fraction;
        self.remaining -= step;
        step
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

    /// Ctrl+wheel during a glide: the glide stops right before the zoom notch. egui merges the
    /// modifiers of one gesture, so a glide still running would otherwise turn into zoom.
    fn end_before_zoom(&mut self, events: &mut Vec<Event>) {
        if !self.is_gliding {
            return;
        }
        let zoom = events.iter().position(
            |event| matches!(event, Event::MouseWheel { modifiers, .. } if modifiers.command),
        );
        let Some(index) = zoom else {
            return;
        };
        let source = self.source.take().unwrap_or(MouseWheelSource::Wheel);
        events.insert(
            index,
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::ZERO,
                phase: TouchPhase::End,
                modifiers: self.modifiers,
                source,
            },
        );
        self.remaining = Vec2::ZERO;
        self.is_gliding = false;
    }

    /// Ends a running glide at once (reduced motion switched on mid-glide).
    fn finish(&mut self, events: &mut Vec<Event>) {
        if !self.is_gliding {
            return;
        }
        let modifiers = self.modifiers;
        let source = self.source.take().unwrap_or(MouseWheelSource::Wheel);
        let rest = std::mem::take(&mut self.remaining);
        for (delta, phase) in [(rest, TouchPhase::Move), (Vec2::ZERO, TouchPhase::End)] {
            events.push(Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta,
                phase,
                modifiers,
                source,
            });
        }
        self.is_gliding = false;
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Modifiers, MouseWheelSource, MouseWheelUnit, RawInput, TouchPhase, vec2};

    use super::{NOTCH, SmoothWheel};

    fn frame(time: f64, events: Vec<Event>) -> RawInput {
        RawInput { time: Some(time), events, ..Default::default() }
    }

    fn notch(modifiers: Modifiers) -> Event {
        Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, -1.0),
            phase: TouchPhase::Move,
            modifiers,
            source: MouseWheelSource::Wheel,
        }
    }

    fn scrolled(input: &RawInput) -> f32 {
        input
            .events
            .iter()
            .map(|event| match event {
                Event::MouseWheel { delta, .. } => delta.y,
                _ => 0.0,
            })
            .sum()
    }

    #[test]
    fn a_notch_glides_the_full_distance_and_then_ends() {
        let mut wheel = SmoothWheel::default();
        let mut input = frame(0.0, vec![notch(Modifiers::NONE)]);
        wheel.process(&mut input, true);
        let first = scrolled(&input);
        assert!(
            first < 0.0 && first > -NOTCH / 2.0,
            "the first frame moves only part of the way: {first}"
        );
        let mut total = first;
        let mut time = 0.0;
        let mut ended = false;
        for _ in 0..120 {
            time += 1.0 / 60.0;
            let mut input = frame(time, Vec::new());
            wheel.process(&mut input, true);
            total += scrolled(&input);
            ended |= input
                .events
                .iter()
                .any(|event| matches!(event, Event::MouseWheel { phase: TouchPhase::End, .. }));
            if !wheel.is_active() {
                break;
            }
        }
        assert!(ended && !wheel.is_active());
        assert!((total + NOTCH).abs() < 0.01, "scrolled {total}");
    }

    #[test]
    fn zoom_and_reduced_motion_pass_through() {
        let mut wheel = SmoothWheel::default();
        let mut zoom = frame(0.0, vec![notch(Modifiers::COMMAND)]);
        wheel.process(&mut zoom, true);
        assert!(matches!(
            zoom.events.as_slice(),
            [Event::MouseWheel { unit: MouseWheelUnit::Line, .. }]
        ));

        let mut reduced = frame(0.1, vec![notch(Modifiers::NONE)]);
        wheel.process(&mut reduced, false);
        assert!(matches!(
            reduced.events.as_slice(),
            [Event::MouseWheel { unit: MouseWheelUnit::Line, .. }]
        ));
        assert!(!wheel.is_active());
    }

    #[test]
    fn a_zoom_notch_ends_a_running_glide_first() {
        let mut wheel = SmoothWheel::default();
        wheel.process(&mut frame(0.0, vec![notch(Modifiers::NONE)]), true);
        assert!(wheel.is_active());
        let mut zoom = frame(1.0 / 60.0, vec![notch(Modifiers::COMMAND)]);
        wheel.process(&mut zoom, true);
        assert!(!wheel.is_active());
        assert!(matches!(
            zoom.events.as_slice(),
            [
                Event::MouseWheel { phase: TouchPhase::End, .. },
                Event::MouseWheel {
                    unit: MouseWheelUnit::Line,
                    modifiers: Modifiers { command: true, .. },
                    ..
                },
            ]
        ));
    }
}
