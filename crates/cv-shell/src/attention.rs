//! Moments that tell where the user goes next, so the app can refresh before they get there.
//!
//! - [`Cue::Returned`]: the first input after [`PAUSE`] without any, or the window becoming active
//!   again after it was inactive (minimized, covered, unfocused) for at least `AWAY`, or after
//!   such a pause. What is on screen may be old by now; the page shown refreshes at once and the
//!   pages likely next warm in the background. The time away is measured from the frame the window
//!   went inactive in, so it holds however long eframe ran no frames meanwhile.
//! - [`Cue::Wrote`]: one of the user's own writes succeeded (an approval, a merge). After one the
//!   user often goes back to the list, so it warms right away.
//!
//! Input counts only while the window is active: a pointer passing over a window in the background
//! is no return.

use std::time::Duration;

use egui::Event;

use cv_base::platform;

/// How long without input counts as a pause: our number. Shorter than the time a diff is read
/// for would refresh during reading; longer ones would leave the page showing minutes-old state.
pub const PAUSE: Duration = Duration::from_secs(60);
/// How long the window has to be inactive for coming back to it to count as a return: our number.
/// A glance at another window and back costs nothing; a minimize or anything longer finds the page
/// refreshed, since the host may have changed it meanwhile (an approval, a merge, a draft made
/// ready).
pub(crate) const AWAY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    Returned,
    Wrote,
}

pub struct Attention {
    /// [`PAUSE`], or longer in tests that measure one screen without the refresh a return brings.
    pause: Duration,
    /// [`AWAY`], or longer in those tests.
    away: Duration,
    /// Input time of the user's last input while the window was active.
    last_input: Option<f64>,
    was_active: bool,
    /// Input time of the frame the window went inactive in.
    inactive_since: Option<f64>,
    /// The last of the user's writes seen ([`cv_services::writes::Writes::last`]).
    writes_seen: u64,
}

impl Default for Attention {
    fn default() -> Self {
        Self {
            pause: PAUSE,
            away: AWAY,
            last_input: None,
            was_active: false,
            inactive_since: None,
            writes_seen: 0,
        }
    }
}

impl Attention {
    /// Never takes a stretch without input or away from the window for a pause: for request
    /// budgets that let simulated minutes pass without a pointer and measure focus on their own.
    #[cfg(any(test, feature = "test-support"))]
    pub fn without_pauses() -> Self {
        Self { pause: Duration::MAX, away: Duration::MAX, ..Self::default() }
    }

    /// Each frame, with the sequence number of the user's latest write: the cue of the frame, if
    /// any. A return outweighs a write in the same frame.
    pub fn update(&mut self, ctx: &egui::Context, last_write: u64) -> Option<Cue> {
        let has_written = last_write != self.writes_seen;
        self.writes_seen = last_write;
        if self.has_returned(ctx) { Some(Cue::Returned) } else { has_written.then_some(Cue::Wrote) }
    }

    fn has_returned(&mut self, ctx: &egui::Context) -> bool {
        let (now, is_active, has_input) = ctx.input(|input| {
            let has_input = input.events.iter().any(is_user_input);
            (input.time, platform::is_window_active(input.viewport()), has_input)
        });
        let became_active = is_active && !self.was_active;
        if self.was_active && !is_active {
            self.inactive_since = Some(now);
        }
        self.was_active = is_active;
        if !is_active || !(has_input || became_active) {
            return false;
        }
        let was_away = became_active
            && self.inactive_since.is_some_and(|since| now - since >= self.away.as_secs_f64());
        let paused = self.last_input.is_some_and(|last| now - last >= self.pause.as_secs_f64());
        self.last_input = Some(now);
        was_away || paused
    }
}

/// Input from the user's hands; window and clipboard notices are not.
fn is_user_input(event: &Event) -> bool {
    matches!(
        event,
        Event::PointerMoved(_)
            | Event::PointerButton { .. }
            | Event::MouseWheel { .. }
            | Event::Key { .. }
            | Event::Text(_)
            | Event::Touch { .. }
            | Event::Zoom(_)
            | Event::Paste(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Clock {
        ctx: egui::Context,
        attention: Attention,
        time: f64,
    }

    impl Clock {
        fn new() -> Self {
            Self { ctx: egui::Context::default(), attention: Attention::default(), time: 0.0 }
        }

        /// One frame `seconds` after the last, with `events` and the window `focused` or not.
        fn frame(
            &mut self,
            seconds: f64,
            events: Vec<Event>,
            focused: bool,
            last_write: u64,
        ) -> Option<Cue> {
            self.time += seconds;
            let mut input = egui::RawInput { time: Some(self.time), events, ..Default::default() };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(focused);
            let mut cue = None;
            let output = self.ctx.run_ui(input, |ui| {
                cue = self.attention.update(ui.ctx(), last_write);
            });
            output.drop_without_applying_deltas();
            cue
        }
    }

    fn moved() -> Vec<Event> {
        vec![Event::PointerMoved(egui::pos2(10.0, 10.0))]
    }

    #[test]
    fn the_first_input_after_a_pause_is_a_return() {
        let mut clock = Clock::new();
        assert_eq!(clock.frame(0.0, moved(), true, 0), None, "the start is no return");
        assert_eq!(clock.frame(30.0, moved(), true, 0), None, "a short pause");
        assert_eq!(clock.frame(59.0, Vec::new(), true, 0), None, "no input, no return");
        assert_eq!(clock.frame(2.0, moved(), true, 0), Some(Cue::Returned));
        assert_eq!(clock.frame(1.0, moved(), true, 0), None, "once per return");
    }

    #[test]
    fn coming_back_to_the_window_after_a_pause_is_a_return() {
        let mut clock = Clock::new();
        clock.frame(0.0, moved(), true, 0);
        assert_eq!(clock.frame(1.0, Vec::new(), false, 0), None);
        assert_eq!(clock.frame(120.0, moved(), false, 0), None, "input in the background");
        assert_eq!(clock.frame(1.0, Vec::new(), true, 0), Some(Cue::Returned));
    }

    /// A minimize of a few seconds is a return, whatever the input before; a glance at another
    /// window is none. No frame runs while minimized: the time counts from the frame it went
    /// inactive in.
    #[test]
    fn coming_back_to_the_window_after_being_away_is_a_return() {
        let mut clock = Clock::new();
        clock.frame(0.0, moved(), true, 0);
        clock.frame(1.0, moved(), true, 0);
        assert_eq!(clock.frame(0.1, Vec::new(), false, 0), None, "minimized");
        assert_eq!(clock.frame(10.0, Vec::new(), true, 0), Some(Cue::Returned), "restored");
        assert_eq!(clock.frame(0.5, moved(), true, 0), None, "once per return");

        assert_eq!(clock.frame(0.1, Vec::new(), false, 0), None);
        assert_eq!(clock.frame(1.0, Vec::new(), true, 0), None, "a glance away");
    }

    #[test]
    fn a_write_of_the_users_is_a_cue() {
        let mut clock = Clock::new();
        clock.frame(0.0, moved(), true, 0);
        assert_eq!(clock.frame(1.0, Vec::new(), true, 1), Some(Cue::Wrote));
        assert_eq!(clock.frame(1.0, Vec::new(), true, 1), None, "once per write");
    }
}
