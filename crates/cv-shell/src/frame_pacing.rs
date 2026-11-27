//! Frame rate limit (Settings → Appearance): caps continuous repaints such as animations, wheel
//! inertia and loading shimmer, never input.
//!
//! egui is reactive, so an idle app renders no frames and there is nothing to limit. While
//! something moves, every frame asks for the next one at once and vsync alone would pace them at
//! the monitor's refresh rate. The limit holds such a frame back, before its UI is built, until
//! the next slot of a fixed grid (one slot per 1/limit seconds) comes up. The grid keeps the
//! average rate on the limit even though vsync presents each frame on the next refresh after it
//! was built. Vsync stays on: frames never tear, and a limit at or above the refresh rate
//! changes nothing.
//!
//! A frame that carries input (pointer, keys, wheel, a resize, focus, dropped files) is never held
//! back. The hold is the platform's ([`platform::FrameHold`]): on Windows a high-resolution
//! waitable timer waited on together with the thread's input queue, so input that arrives during
//! the hold ends it at once and the next frame handles it without delay. Nothing spins: the thread
//! sleeps in the kernel the whole time.

use std::time::{Duration, Instant};

use egui::RawInput;
use review_store::FrameRateLimit;

use cv_base::platform;

/// Decides how long a frame waits for its slot. Pure: the caller passes the clock in.
#[derive(Debug, Default)]
pub struct FramePacer {
    /// The next slot a continuous frame may start in.
    next_slot: Option<Instant>,
}

impl FramePacer {
    /// How long a frame that would start at `now` waits; also books the slot it starts in.
    pub fn wait(&mut self, now: Instant, limit: FrameRateLimit, has_input: bool) -> Duration {
        let Some(fps) = limit.cap() else {
            self.next_slot = None;
            return Duration::ZERO;
        };
        let period = Duration::from_secs_f64(1.0 / f64::from(fps));
        match self.next_slot {
            // Continuous frame ahead of its slot: wait for it, and the following slot is one
            // period later, wherever vsync ends up presenting this one.
            Some(slot) if !has_input && now < slot => {
                self.next_slot = Some(slot + period);
                slot - now
            }
            // Slightly late for its slot (vsync rounding): start now, keep the grid.
            Some(slot) if !has_input && now < slot + period => {
                self.next_slot = Some(slot + period);
                Duration::ZERO
            }
            // Input, the first frame after idle, or far behind: start now, grid from here.
            _ => {
                self.next_slot = Some(now + period);
                Duration::ZERO
            }
        }
    }
}

/// Paces the app's frames: tracks what counts as input and holds continuous frames back.
#[derive(Default)]
pub struct FrameLimiter {
    pacer: FramePacer,
    /// What the previous frame's input looked like, to tell a resize or focus change.
    screen_rect: Option<egui::Rect>,
    focused: Option<bool>,
    hold: platform::FrameHold,
    /// How long the current and the previous frame were held back. eframe counts the hold in a
    /// frame's CPU time, which a frame learns one frame later.
    held: Duration,
    held_before: Duration,
}

impl FrameLimiter {
    /// Call from `raw_input_hook`: waits for this frame's slot and moves its time to the end of
    /// the wait, so animations step by the real interval.
    pub fn pace(&mut self, raw_input: &mut RawInput, limit: FrameRateLimit) {
        let focused =
            raw_input.viewports.get(&raw_input.viewport_id).and_then(|viewport| viewport.focused);
        let has_input = has_input(raw_input)
            || self.screen_rect != raw_input.screen_rect
            || self.focused != focused;
        self.screen_rect = raw_input.screen_rect;
        self.focused = focused;
        let wait = self.pacer.wait(Instant::now(), limit, has_input);
        if let Some(fps) = limit.cap() {
            // The first frame after idle steps animations by one capped frame, not by 1/60 s.
            raw_input.predicted_dt = raw_input.predicted_dt.max(1.0 / f32::from(fps));
        }
        self.held_before = std::mem::take(&mut self.held);
        if wait.is_zero() {
            return;
        }
        let started = Instant::now();
        self.hold.wait(wait);
        self.held = started.elapsed();
        raw_input.time = raw_input.time.map(|time| time + self.held.as_secs_f64());
    }

    /// How long the previous frame was held back (sleeping, not working): the part of the CPU
    /// time eframe reports this frame that was no work.
    pub fn held_before(&self) -> Duration {
        self.held_before
    }
}

/// Anything in this frame's input that the user did.
fn has_input(raw_input: &RawInput) -> bool {
    !raw_input.events.is_empty()
        || !raw_input.hovered_files.is_empty()
        || !raw_input.dropped_files.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// A fake clock: the pacer only ever sees the instants it is handed.
    struct Clock(Instant);

    impl Clock {
        fn at(&self, milliseconds: f64) -> Instant {
            self.0 + Duration::from_secs_f64(milliseconds / 1000.0)
        }
    }

    fn millis(duration: Duration) -> f64 {
        duration.as_secs_f64() * 1000.0
    }

    #[test]
    fn input_and_the_first_frame_after_idle_never_wait() {
        let clock = Clock(Instant::now());
        let mut pacer = FramePacer::default();
        assert_eq!(
            pacer.wait(clock.at(0.0), FrameRateLimit::Fps(45), false),
            Duration::ZERO,
            "first frame"
        );
        assert_eq!(
            pacer.wait(clock.at(1.0), FrameRateLimit::Fps(45), true),
            Duration::ZERO,
            "a click right after"
        );
        assert_eq!(
            pacer.wait(clock.at(2.0), FrameRateLimit::Fps(30), true),
            Duration::ZERO,
            "a key at 30 fps"
        );
        // After a long idle stretch (a timed repaint, data arriving) the frame starts at once.
        assert_eq!(pacer.wait(clock.at(5_000.0), FrameRateLimit::Fps(45), false), Duration::ZERO);
    }

    #[test]
    fn continuous_frames_wait_for_their_slot() {
        let clock = Clock(Instant::now());
        let mut pacer = FramePacer::default();
        let period = 1000.0 / 45.0;
        pacer.wait(clock.at(0.0), FrameRateLimit::Fps(45), false);
        // The animation asks for the next frame 7 ms later (one 144 Hz refresh).
        let wait = millis(pacer.wait(clock.at(7.0), FrameRateLimit::Fps(45), false));
        assert!((wait - (period - 7.0)).abs() < 0.01, "waits {wait} ms");
        // Input in between starts at once and restarts the grid from there.
        assert_eq!(pacer.wait(clock.at(30.0), FrameRateLimit::Fps(45), true), Duration::ZERO);
        let wait = millis(pacer.wait(clock.at(32.0), FrameRateLimit::Fps(45), false));
        assert!((wait - (period - 2.0)).abs() < 0.01, "waits {wait} ms after the input frame");
    }

    /// A 144 Hz display with vsync: each frame is built (2 ms) once its slot came, then presented
    /// on the next refresh. Over a second the grid keeps the average on the limit.
    #[test]
    fn the_average_rate_stays_on_the_limit_under_vsync() {
        for (fps, refresh_hz) in
            [(45_u16, 144.0), (30, 144.0), (60, 144.0), (45, 60.0), (144, 144.0), (240, 144.0)]
        {
            let clock = Clock(Instant::now());
            let mut pacer = FramePacer::default();
            let refresh = 1000.0 / refresh_hz;
            let mut now = 0.0_f64;
            let mut frames = 0;
            while now < 1000.0 {
                now += millis(pacer.wait(clock.at(now), FrameRateLimit::Fps(fps), false));
                frames += 1;
                // Build for 2 ms, then wait for the next vblank to present.
                now = ((now + 2.0) / refresh).ceil() * refresh;
            }
            let expected = f64::from(fps).min(refresh_hz);
            assert!(
                (f64::from(frames) - expected).abs() <= 2.0,
                "{fps} fps on {refresh_hz} Hz ran {frames} frames"
            );
        }
    }

    #[test]
    fn match_display_never_waits() {
        let clock = Clock(Instant::now());
        let mut pacer = FramePacer::default();
        for frame in 0..100 {
            assert_eq!(
                pacer.wait(clock.at(f64::from(frame)), FrameRateLimit::MatchDisplay, false),
                Duration::ZERO
            );
        }
        // Switching to a limit live starts its grid at the next frame.
        assert_eq!(pacer.wait(clock.at(100.0), FrameRateLimit::Fps(30), false), Duration::ZERO);
        assert!(pacer.wait(clock.at(101.0), FrameRateLimit::Fps(30), false) > 30 * MS);
    }

    #[test]
    fn the_limiter_counts_events_resizes_and_focus_as_input() {
        let mut limiter = FrameLimiter::default();
        let mut input = RawInput { time: Some(1.0), ..RawInput::default() };
        limiter.pace(&mut input, FrameRateLimit::Fps(30));
        // A resize right after would be held back if it did not count as input.
        let mut resized = RawInput {
            time: Some(1.001),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..RawInput::default()
        };
        limiter.pace(&mut resized, FrameRateLimit::Fps(30));
        let mut moved =
            RawInput { time: Some(1.002), screen_rect: resized.screen_rect, ..RawInput::default() };
        moved.events.push(egui::Event::PointerMoved(egui::pos2(10.0, 10.0)));
        limiter.pace(&mut moved, FrameRateLimit::Fps(30));
        let mut focused =
            RawInput { time: Some(1.003), screen_rect: resized.screen_rect, ..RawInput::default() };
        focused.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(true);
        limiter.pace(&mut focused, FrameRateLimit::Fps(30));
        assert_eq!(
            (resized.time, moved.time, focused.time),
            (Some(1.001), Some(1.002), Some(1.003)),
            "input frames keep their time"
        );
        assert!((moved.predicted_dt - 1.0 / 30.0).abs() < 1e-6);
    }

    #[test]
    fn a_held_frame_moves_its_time_by_the_wait() {
        let mut limiter = FrameLimiter::default();
        limiter.pace(
            &mut RawInput { time: Some(1.0), ..RawInput::default() },
            FrameRateLimit::Fps(60),
        );
        let mut next = RawInput { time: Some(1.0), ..RawInput::default() };
        limiter.pace(&mut next, FrameRateLimit::Fps(60));
        let shift = next.time.unwrap() - 1.0;
        assert!(shift > 0.009, "time moved by {shift}");
        // The frame after learns how long its predecessor slept, to keep it out of the CPU time.
        let mut clicked = RawInput { time: Some(1.1), ..RawInput::default() };
        clicked.events.push(egui::Event::PointerGone);
        limiter.pace(&mut clicked, FrameRateLimit::Fps(60));
        assert!(
            (limiter.held_before().as_secs_f64() - shift).abs() < 1e-9,
            "the held time is handed on"
        );
    }
}
