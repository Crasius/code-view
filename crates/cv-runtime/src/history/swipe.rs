//! A two-finger swipe across the trackpad steps back or forward, as in Safari and the Finder.
//!
//! winit hands macOS trackpad scrolling to egui as wheel events with a phase: `Start` when the
//! fingers land, `Move` while they travel, `End` when they lift, then the system's momentum as
//! events of their own source. A swipe is one such gesture whose fingers travel mostly sideways
//! and whose sideways travel no content took: egui's scroll areas remove what they scroll from the
//! frame's scroll delta (content that scrolls itself must do the same, as the pipeline graph does),
//! so what is left after the page is free. Once
//! any content took part of a gesture, the whole gesture belongs to it, the way Safari leaves a
//! swipe to a page that can still scroll that way. The step fires once, as soon as the fingers
//! have travelled [`TRAVEL`]; momentum never counts.
//!
//! Fingers to the right go back, to the left forward, whichever way the system scrolls content
//! (its natural scrolling setting turns the deltas round). Only where the system's "Swipe between
//! pages" is on ([`cv_base::platform::page_swipe`]), read as the fingers land.

use cv_base::platform::PageSwipe;
use egui::{Event, InputState, MouseWheelSource, TouchPhase, Vec2};

use super::Step;

/// How far the fingers travel sideways before the swipe steps (points; our own choice, about a
/// third of a trackpad's width at the default tracking speed).
const TRAVEL: f32 = 120.0;
/// A swipe travels at least this many times as far sideways as up or down.
const SIDEWAYS_RATIO: f32 = 2.0;
/// Sideways delta the content may round away without having scrolled (points).
const TAKEN_EPSILON: f32 = 0.5;

/// One frame's trackpad input, read after the page took what it scrolls.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SwipeFrame {
    /// Fingers landed on the trackpad this frame.
    pub started: bool,
    /// The fingers' travel this frame, in egui's content direction.
    pub moved: Vec2,
    /// A modifier was held, so the travel zooms or scrolls along one axis instead.
    pub has_modifiers: bool,
    /// What of `moved.x` no content scrolled.
    pub left_over_x: f32,
    /// The fingers lifted this frame.
    pub ended: bool,
}

impl SwipeFrame {
    /// The frame's finger events and the sideways delta the page left over.
    pub fn read(input: &InputState) -> Self {
        let mut frame = Self { left_over_x: input.smooth_scroll_delta.x, ..Self::default() };
        for event in &input.events {
            let Event::MouseWheel { delta, phase, source, modifiers, .. } = event else {
                continue;
            };
            if *source != MouseWheelSource::Trackpad {
                continue;
            }
            match phase {
                TouchPhase::Start => frame.started = true,
                TouchPhase::Move => {
                    frame.moved += *delta;
                    frame.has_modifiers |= !modifiers.is_none();
                }
                TouchPhase::End | TouchPhase::Cancel => frame.ended = true,
            }
        }
        frame
    }

    /// Some content scrolled part of this frame's sideways travel.
    fn is_taken(&self) -> bool {
        self.moved.x != 0.0 && (self.moved.x - self.left_over_x).abs() > TAKEN_EPSILON
    }
}

/// The gesture under way, between the fingers landing and lifting.
#[derive(Debug, Default)]
pub struct Swipe {
    gesture: Option<Gesture>,
}

#[derive(Debug)]
struct Gesture {
    setting: PageSwipe,
    travel: Vec2,
    /// Content scrolled with it, or a modifier turned it into something else.
    is_content: bool,
    has_stepped: bool,
}

impl Swipe {
    /// Follows the gesture through `frame`; the step once its fingers have swiped far enough
    /// sideways. `setting` reads the system's swipe setting when fingers land.
    pub fn observe(
        &mut self,
        frame: SwipeFrame,
        setting: impl FnOnce() -> Option<PageSwipe>,
    ) -> Option<Step> {
        if frame.started {
            self.gesture = setting().map(Gesture::new);
        }
        let step = self.gesture.as_mut().and_then(|gesture| gesture.travel(&frame));
        if frame.ended {
            self.gesture = None;
        }
        step
    }
}

impl Gesture {
    fn new(setting: PageSwipe) -> Self {
        Self { setting, travel: Vec2::ZERO, is_content: false, has_stepped: false }
    }

    fn travel(&mut self, frame: &SwipeFrame) -> Option<Step> {
        if self.has_stepped || self.is_content {
            return None;
        }
        if frame.is_taken() || frame.has_modifiers {
            self.is_content = true;
            return None;
        }
        self.travel += frame.moved;
        let sideways = self.travel.x.abs();
        if sideways < TRAVEL || sideways < SIDEWAYS_RATIO * self.travel.y.abs() {
            return None;
        }
        self.has_stepped = true;
        // Natural scrolling moves content with the fingers; otherwise the deltas are turned round.
        let is_fingers_right = (self.travel.x > 0.0) == self.setting.is_natural;
        Some(if is_fingers_right { Step::Back } else { Step::Forward })
    }
}

#[cfg(test)]
mod tests {
    use cv_base::platform::PageSwipe;
    use egui::{Event, Modifiers, MouseWheelSource, MouseWheelUnit, RawInput, TouchPhase, vec2};

    use super::{Swipe, SwipeFrame, TRAVEL};
    use crate::history::Step;

    const NATURAL: Option<PageSwipe> = Some(PageSwipe { is_natural: true });
    const NO_STEP: [Step; 0] = [];

    fn start() -> SwipeFrame {
        SwipeFrame { started: true, ..SwipeFrame::default() }
    }

    /// The fingers travel `x`, `y` and the page leaves all of it.
    fn free(x: f32, y: f32) -> SwipeFrame {
        SwipeFrame { moved: vec2(x, y), left_over_x: x, ..SwipeFrame::default() }
    }

    fn end() -> SwipeFrame {
        SwipeFrame { ended: true, ..SwipeFrame::default() }
    }

    /// Every step the frames produce with `setting`.
    fn steps_with(setting: Option<PageSwipe>, frames: &[SwipeFrame]) -> Vec<Step> {
        let mut swipe = Swipe::default();
        frames.iter().filter_map(|frame| swipe.observe(*frame, || setting)).collect()
    }

    fn steps(frames: &[SwipeFrame]) -> Vec<Step> {
        steps_with(NATURAL, frames)
    }

    #[test]
    fn fingers_to_the_right_go_back_once() {
        let quarter = TRAVEL / 4.0;
        let frames = [start(), free(quarter, 0.0), free(quarter, 2.0), free(quarter, 0.0)];
        assert_eq!(steps(&frames), NO_STEP, "three quarters of the way do nothing yet");
        let frames = [&frames[..], &[free(quarter, 0.0), free(quarter, 0.0), end()]].concat();
        assert_eq!(steps(&frames), [Step::Back], "one step however far the fingers go on");
    }

    #[test]
    fn fingers_to_the_left_go_forward() {
        assert_eq!(steps(&[start(), free(-TRAVEL, 0.0), end()]), [Step::Forward]);
    }

    #[test]
    fn without_natural_scrolling_the_fingers_still_decide() {
        let unnatural = Some(PageSwipe { is_natural: false });
        // Fingers to the right scroll content to the left there.
        assert_eq!(steps_with(unnatural, &[start(), free(-TRAVEL, 0.0)]), [Step::Back]);
    }

    #[test]
    fn with_swiping_off_in_the_system_nothing_steps() {
        assert_eq!(steps_with(None, &[start(), free(TRAVEL, 0.0), end()]), NO_STEP);
    }

    /// The setting is read once, as the fingers land.
    #[test]
    fn the_setting_is_read_when_fingers_land() {
        let mut swipe = Swipe::default();
        let mut reads = 0;
        for frame in [start(), free(TRAVEL / 2.0, 0.0), free(TRAVEL / 2.0, 0.0), end()] {
            swipe.observe(frame, || {
                reads += 1;
                NATURAL
            });
        }
        assert_eq!(reads, 1);
    }

    #[test]
    fn a_gesture_content_scrolled_never_steps() {
        let taken =
            SwipeFrame { moved: vec2(10.0, 0.0), left_over_x: 0.0, ..SwipeFrame::default() };
        // The pane reached its edge after a while; the rest of the gesture is still the pane's.
        assert_eq!(steps(&[start(), taken, free(TRAVEL, 0.0), free(TRAVEL, 0.0), end()]), NO_STEP);
        // The next gesture starts free again.
        assert_eq!(steps(&[start(), taken, end(), start(), free(TRAVEL, 0.0)]), [Step::Back]);
    }

    #[test]
    fn mostly_vertical_travel_scrolls_instead() {
        assert_eq!(steps(&[start(), free(TRAVEL, TRAVEL), end()]), NO_STEP);
    }

    #[test]
    fn a_held_modifier_makes_it_no_swipe() {
        let zoom = SwipeFrame { has_modifiers: true, ..free(TRAVEL, 0.0) };
        assert_eq!(steps(&[start(), zoom, free(TRAVEL, 0.0), end()]), NO_STEP);
    }

    #[test]
    fn travel_without_landing_fingers_is_no_swipe() {
        // Momentum after the lift, or a wheel: no gesture is under way.
        assert_eq!(steps(&[free(TRAVEL, 0.0), free(TRAVEL, 0.0)]), NO_STEP);
        assert_eq!(steps(&[start(), end(), free(TRAVEL, 0.0)]), NO_STEP);
    }

    /// The frame reads trackpad fingers only; momentum and wheels pass by.
    #[test]
    fn a_frame_reads_only_trackpad_fingers() {
        let wheel = |phase, delta, source| Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta,
            phase,
            source,
            modifiers: Modifiers::NONE,
        };
        let ctx = egui::Context::default();
        let events = vec![
            wheel(TouchPhase::Start, vec2(0.0, 0.0), MouseWheelSource::Trackpad),
            wheel(TouchPhase::Move, vec2(30.0, 1.0), MouseWheelSource::Trackpad),
            wheel(TouchPhase::Move, vec2(500.0, 0.0), MouseWheelSource::Momentum),
        ];
        let mut frame = SwipeFrame::default();
        ctx.run_ui(RawInput { events, ..RawInput::default() }, |ui| {
            frame = ui.input(SwipeFrame::read);
        })
        .drop_without_applying_deltas();
        assert!(frame.started && !frame.ended && !frame.has_modifiers);
        assert_eq!(frame.moved, vec2(30.0, 1.0));
    }
}
