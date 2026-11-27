//! The one place every automatic refresh goes through.
//!
//! A screen asks [`RefreshScheduler::due`] each frame it is shown; the scheduler answers true when
//! the item's interval has passed. Screens that are not shown do not ask, so hidden tabs and pages
//! never refresh. While the window is unfocused, minimized or covered nothing fires and no repaint
//! is requested for it: due items are only marked pending. When the window is active again, every
//! pending item fires once, [`CATCH_UP_STAGGER`] apart, and the normal schedule resumes from there.
//! eframe runs no frames at all while the window is minimized; an item whose interval passed
//! meanwhile fires on the first frame after, however long the gap was (entries age only while
//! frames run, see [`FORGET_AFTER`]). The user coming back after a pause or to the window
//! ([`RefreshScheduler::wake`], `cv_shell::attention`) catches up the same way, before the interval.
//! User actions (clicks, Retry, posting) never go through here and are never paused.
//!
//! The clock is egui's input time, so tests drive it with simulated frames.
//!
//! Running pipelines are polled ([`pipeline_poll_due`]): GitHub, GitLab and Azure DevOps offer no
//! push channel a desktop client can subscribe to without a webhook on a server of its own, so
//! polling is the only way to follow a run live.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use cv_base::platform::is_window_active;
use review_core::PipelineRun;

use super::load::Live;

/// Gap between catch-up refreshes after the window became active again.
pub const CATCH_UP_STAGGER: Duration = Duration::from_millis(200);
/// Entries no screen asked about for this long while the app ran frames are dropped. Our number.
/// Measured up to the last pass that asked anything, not up to now: eframe runs no frame while the
/// window is minimized, and the entries of the page shown must survive that gap to catch up.
const FORGET_AFTER: f64 = 15.0 * 60.0;

/// Running pipelines reload this often with a token: runs and the stages of the unfinished runs
/// shown. Our own number. Fixed while anything runs: the runs' answer often stays the same while
/// jobs inside them progress, so a backoff on it would let the jobs lag.
pub const PIPELINE_INTERVAL: Duration = Duration::from_secs(15);
/// Running pipelines reload this often on an anonymous connection. Our own number: GitHub allows
/// 60 anonymous requests an hour, the live probe takes 30 of them at its 120 s, and every poll
/// costs the runs plus the stages of each running run shown, so 12 polls an hour leave room for
/// both.
pub const ANONYMOUS_PIPELINE_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhenInactive {
    /// Paused while the window is inactive; caught up once when it is active again.
    Pause,
    /// Keeps its timer while inactive. Only for local work that must not lapse (CLI token renewal).
    KeepRunning,
}

#[derive(Default)]
pub struct RefreshScheduler {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Entry>,
    was_active: bool,
    /// Earliest time the next catch-up refresh may fire.
    next_slot: f64,
    /// Pass in which a holder (a token renewal in flight) asked paused items to wait.
    held_in_pass: Option<u64>,
    /// The pass and input time of the latest ask: stale entries are judged against it.
    last_pass: u64,
    last_time: f64,
}

struct Entry {
    last_fired: f64,
    last_asked: f64,
    pending: bool,
    slot: Option<f64>,
    /// Asked with [`WhenInactive::Pause`] last time.
    is_paused: bool,
}

impl RefreshScheduler {
    /// True when the item `key` is due now. The first call only starts its timer: whatever was just
    /// loaded on opening counts as the first refresh.
    pub fn due(
        &self,
        ctx: &egui::Context,
        key: &str,
        interval: Duration,
        policy: WhenInactive,
    ) -> bool {
        let (now, is_active) = ctx.input(|input| (input.time, is_window_active(input.viewport())));
        let pass = ctx.cumulative_pass_nr();
        let mut inner = self.lock();
        inner.observe_activity(is_active, now);
        let is_held = inner.held_in_pass.is_some_and(|held| held + 1 >= pass);
        inner.forget_stale(pass, now);
        let Inner { entries, next_slot, .. } = &mut *inner;
        if !entries.contains_key(key) {
            let entry = Entry {
                last_fired: now,
                last_asked: now,
                pending: false,
                slot: None,
                is_paused: true,
            };
            entries.insert(key.to_owned(), entry);
        }
        let Some(entry) = entries.get_mut(key) else {
            return false;
        };
        let is_paused = policy == WhenInactive::Pause;
        entry.last_asked = now;
        entry.is_paused = is_paused;
        let due_at = entry.last_fired + interval.as_secs_f64();
        if now < due_at {
            if is_active || !is_paused {
                ctx.request_repaint_after(Duration::from_secs_f64(due_at - now));
            }
            return false;
        }
        if is_paused && !is_active {
            entry.pending = true;
            return false;
        }
        if is_paused && is_held {
            ctx.request_repaint_after(CATCH_UP_STAGGER);
            return false;
        }
        if is_paused && entry.pending {
            let slot = *entry.slot.get_or_insert(next_slot.max(now));
            if now < slot {
                ctx.request_repaint_after(Duration::from_secs_f64(slot - now));
                return false;
            }
            *next_slot = slot.max(now) + CATCH_UP_STAGGER.as_secs_f64();
        }
        entry.last_fired = now;
        entry.pending = false;
        entry.slot = None;
        true
    }

    /// The user refreshed `key` by hand: the next automatic refresh is a full interval later.
    pub fn restart(&self, ctx: &egui::Context, key: &str) {
        let now = ctx.input(|input| input.time);
        if let Some(entry) = self.lock().entries.get_mut(key) {
            entry.last_fired = now;
            entry.pending = false;
            entry.slot = None;
        }
    }

    /// The user came back after a pause: every paused item of the screens shown fires on its next
    /// ask, [`CATCH_UP_STAGGER`] apart, as after the window became active again. Items that keep
    /// running while inactive keep their timer.
    pub fn wake(&self, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        let mut inner = self.lock();
        inner.next_slot = inner.next_slot.max(now);
        for entry in inner.entries.values_mut().filter(|entry| entry.is_paused) {
            entry.last_fired = f64::NEG_INFINITY;
            entry.pending = true;
        }
    }

    /// Stops tracking `key` (the run finished, the page closed).
    pub fn forget(&self, key: &str) {
        self.lock().entries.remove(key);
    }

    /// Paused items wait while this is called each frame (a token renewal they would need is running).
    pub fn hold(&self, ctx: &egui::Context) {
        self.lock().held_in_pass = Some(ctx.cumulative_pass_nr());
    }

    /// Items waiting for the window to become active again.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pending_count(&self) -> usize {
        self.lock().entries.values().filter(|entry| entry.pending).count()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Inner {
    fn observe_activity(&mut self, is_active: bool, now: f64) {
        if is_active && !self.was_active {
            self.next_slot = now;
        }
        self.was_active = is_active;
    }

    /// Once per pass, before anything is asked in it: drops the entries no screen asked about for
    /// [`FORGET_AFTER`] up to the previous pass. A stretch without frames (a minimized window)
    /// ages nothing, so the first frame after it finds the entries of the screens shown, with
    /// their pending catch-ups and wake marks, and fires them.
    fn forget_stale(&mut self, pass: u64, now: f64) {
        if pass != self.last_pass {
            let previous = self.last_time;
            self.entries.retain(|_, entry| previous - entry.last_asked < FORGET_AFTER);
            self.last_pass = pass;
        }
        self.last_time = now;
    }
}

/// The live refresh of a pull request's pipelines, asked by whichever screen shows them (the
/// Overview, the Pipelines tab) under one `key` per pull request, so switching between them keeps
/// the schedule. Due only while a run is unfinished and no reload of the runs is on its way,
/// every [`PIPELINE_INTERVAL`] ([`ANONYMOUS_PIPELINE_INTERVAL`] without a token); once every run
/// finished, the key is dropped and nothing is polled.
pub fn pipeline_poll_due(
    ctx: &egui::Context,
    scheduler: &RefreshScheduler,
    key: &str,
    runs: &Live<Vec<PipelineRun>>,
    has_token: bool,
) -> bool {
    let is_running =
        runs.ready().is_some_and(|runs| runs.iter().any(|run| !run.status.is_finished()));
    if !is_running {
        scheduler.forget(key);
        return false;
    }
    let interval = if has_token { PIPELINE_INTERVAL } else { ANONYMOUS_PIPELINE_INTERVAL };
    !runs.is_loading() && scheduler.due(ctx, key, interval, WhenInactive::Pause)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Clock {
        ctx: egui::Context,
        time: f64,
        focused: bool,
    }

    impl Clock {
        fn new() -> Self {
            Self { ctx: egui::Context::default(), time: 0.0, focused: true }
        }

        /// Runs one frame at `self.time`; the keys that fired.
        fn frame(
            &mut self,
            scheduler: &RefreshScheduler,
            keys: &[&str],
            interval: Duration,
        ) -> Vec<String> {
            let mut input = egui::RawInput { time: Some(self.time), ..Default::default() };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(self.focused);
            let mut fired = Vec::new();
            self.ctx
                .run_ui(input, |ui| {
                    for key in keys {
                        if scheduler.due(ui.ctx(), key, interval, WhenInactive::Pause) {
                            fired.push((*key).to_owned());
                        }
                    }
                })
                .drop_without_applying_deltas();
            fired
        }

        fn run(
            &mut self,
            scheduler: &RefreshScheduler,
            keys: &[&str],
            interval: Duration,
            seconds: f64,
        ) -> Vec<String> {
            let mut fired = Vec::new();
            let end = self.time + seconds;
            while self.time < end {
                self.time += 0.05;
                fired.extend(self.frame(scheduler, keys, interval));
            }
            fired
        }
    }

    #[test]
    fn fires_every_interval_while_focused() {
        let scheduler = RefreshScheduler::default();
        let mut clock = Clock::new();
        assert!(
            clock.frame(&scheduler, &["a"], Duration::from_secs(15)).is_empty(),
            "first ask only starts the timer"
        );
        let fired = clock.run(&scheduler, &["a"], Duration::from_secs(15), 61.0);
        assert_eq!(fired.len(), 4);
    }

    #[test]
    fn unfocused_fires_nothing_and_catches_up_once_staggered() {
        let scheduler = RefreshScheduler::default();
        let mut clock = Clock::new();
        let keys = ["a", "b", "c"];
        clock.frame(&scheduler, &keys, Duration::from_secs(15));
        clock.focused = false;
        assert_eq!(
            clock.run(&scheduler, &keys, Duration::from_secs(15), 300.0),
            Vec::<String>::new()
        );
        assert_eq!(scheduler.pending_count(), 3);

        clock.focused = true;
        let first = clock.frame(&scheduler, &keys, Duration::from_secs(15));
        assert_eq!(first.len(), 1, "one at a time, not a burst");
        let rest = clock.run(&scheduler, &keys, Duration::from_secs(15), 1.0);
        assert_eq!(rest.len(), 2, "each pending item fired exactly once");
        let mut all = [first, rest].concat();
        all.sort();
        assert_eq!(all, ["a", "b", "c"]);
        assert_eq!(scheduler.pending_count(), 0);
        // The first catch-up fired at focus; 15 s later the regular schedule fires again.
        let quiet = clock.run(&scheduler, &keys, Duration::from_secs(15), 13.0);
        assert!(quiet.is_empty(), "the schedule resumes from the catch-up");
        let next = clock.run(&scheduler, &keys, Duration::from_secs(15), 2.0);
        assert_eq!(next.len(), 3, "each item fires again one interval after its catch-up");
    }

    /// eframe runs no frame while the window is minimized: the first frame after a gap longer than
    /// the forget time still finds the page's item and fires it at once.
    #[test]
    fn a_window_minimized_for_any_length_of_time_catches_up_at_once() {
        let interval = Duration::from_secs(30);
        for minimized in [40.0, FORGET_AFTER + 60.0, 6.0 * 3600.0] {
            let scheduler = RefreshScheduler::default();
            let mut clock = Clock::new();
            clock.frame(&scheduler, &["live"], interval);
            // The frame of the minimize, then none until the window is restored.
            clock.time += 0.05;
            clock.focused = false;
            assert_eq!(clock.frame(&scheduler, &["live"], interval), Vec::<String>::new());
            clock.time += minimized;
            clock.focused = true;
            assert_eq!(
                clock.frame(&scheduler, &["live"], interval),
                ["live"],
                "minimized for {minimized} s"
            );
        }
    }

    /// The return after a short minimize wakes the items, which fire on the first frame although
    /// their interval has not passed, also when no frame ran in between.
    #[test]
    fn a_wake_after_a_gap_without_frames_fires_every_item() {
        let scheduler = RefreshScheduler::default();
        let mut clock = Clock::new();
        let keys = ["live", "pipelines"];
        clock.frame(&scheduler, &keys, Duration::from_secs(30));
        clock.time += FORGET_AFTER + 1.0;
        scheduler.wake(&clock.ctx);
        let first = clock.frame(&scheduler, &keys, Duration::from_secs(30));
        let rest = clock.run(&scheduler, &keys, Duration::from_secs(30), 1.0);
        assert_eq!([first, rest].concat().len(), 2, "each item fired once");
    }

    /// An item no screen asked about for [`FORGET_AFTER`] while frames ran is dropped: asked again,
    /// it only starts its timer.
    #[test]
    fn items_of_screens_no_longer_shown_are_forgotten() {
        let scheduler = RefreshScheduler::default();
        let mut clock = Clock::new();
        let interval = Duration::from_secs(30);
        clock.frame(&scheduler, &["closed", "shown"], interval);
        let end = clock.time + FORGET_AFTER + 10.0;
        while clock.time < end {
            clock.time += 10.0;
            clock.frame(&scheduler, &["shown"], interval);
        }
        clock.time += 0.05;
        assert_eq!(
            clock.frame(&scheduler, &["closed"], interval),
            Vec::<String>::new(),
            "a forgotten item starts over"
        );
    }

    #[test]
    fn a_wake_fires_every_item_once_staggered_before_its_interval() {
        let scheduler = RefreshScheduler::default();
        let mut clock = Clock::new();
        let keys = ["a", "b"];
        clock.frame(&scheduler, &keys, Duration::from_secs(30));
        let quiet = clock.run(&scheduler, &keys, Duration::from_secs(30), 10.0);
        assert_eq!(quiet, Vec::<String>::new());

        scheduler.wake(&clock.ctx);
        let first = clock.frame(&scheduler, &keys, Duration::from_secs(30));
        assert_eq!(first.len(), 1, "one at a time, not a burst");
        let rest = clock.run(&scheduler, &keys, Duration::from_secs(30), 1.0);
        assert_eq!(rest.len(), 1, "each item fired exactly once");
        let resumed = clock.run(&scheduler, &keys, Duration::from_secs(30), 28.0);
        assert_eq!(resumed, Vec::<String>::new(), "the schedule resumes from the wake");
    }
}
