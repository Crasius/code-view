//! Where a test thread is, for the [`Watchdog`](crate::Watchdog)'s message when the test runs past
//! its limit: the wait or step it is in (the helpers' labels, innermost last), the frames it ran
//! and whether one is running now, the background work in flight after the last frame, and a
//! panic it has not ended since.
//!
//! libtest shows a test's captured output only when the test ends, so a test that hangs, or that
//! panicked and then hangs in a drop (a runtime waiting for a blocked worker), would leave nothing
//! but its name. The watchdog writes this trace to the process's stderr instead.

use std::cell::OnceCell;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

/// What one test thread did last.
#[derive(Default)]
struct State {
    /// The waits and steps under way, outermost first.
    steps: Vec<(String, Instant)>,
    frames: u64,
    /// When the frame now running started.
    frame_started: Option<Instant>,
    last_frame_ended: Option<Instant>,
    /// Background tasks in flight when the last frame ended.
    in_flight: Option<usize>,
    /// What asked egui for the last frame: what keeps a run of frames going.
    repaint_causes: Vec<egui::RepaintCause>,
    /// When the test panicked, and the panic's message and place.
    panic: Option<(Instant, String)>,
}

/// One test thread's trace, shared with its watchdog thread.
#[derive(Clone, Default)]
pub(crate) struct Trace(Arc<Mutex<State>>);

thread_local! {
    static TRACE: OnceCell<Trace> = const { OnceCell::new() };
}

/// This test thread's trace; made on first use, so every helper and the watchdog see the same.
pub(crate) fn current() -> Trace {
    TRACE.with(|trace| trace.get_or_init(Trace::default).clone())
}

impl Trace {
    fn update(&self, change: impl FnOnce(&mut State)) {
        change(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// The trace as the watchdog prints it, as of `now`.
    pub(crate) fn describe(&self, now: Instant) -> String {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let ago = |at: Instant| seconds(now.saturating_duration_since(at));
        let mut text = String::new();
        if state.steps.is_empty() {
            text.push_str("\nIt was in no labelled wait or step.");
        } else {
            text.push_str("\nIt was in:");
            for (what, started) in &state.steps {
                let _infallible = write!(text, "\n  - {what} (for {})", ago(*started));
            }
        }
        match (state.frame_started, state.last_frame_ended) {
            (Some(started), _) => {
                let _infallible = write!(
                    text,
                    "\nFrame {} has been running for {} (seconds here mean the app's frame never \
                     returned; a fraction means frames follow one another).",
                    state.frames,
                    ago(started)
                );
            }
            (None, Some(ended)) => {
                let _infallible = write!(
                    text,
                    "\n{} frames ran; the last ended {} ago.",
                    state.frames,
                    ago(ended)
                );
            }
            (None, None) => text.push_str("\nNo frame of the app ran."),
        }
        if let Some(in_flight) = state.in_flight {
            let _infallible =
                write!(text, "\n{in_flight} background tasks were in flight after the last frame.");
        }
        if !state.repaint_causes.is_empty() {
            text.push_str("\nThe last frame was asked for by:");
            for cause in &state.repaint_causes {
                let _infallible = write!(text, "\n  - {cause}");
            }
        }
        if let Some((at, message)) = &state.panic {
            let _infallible = write!(
                text,
                "\nIt panicked {} ago (a test that hangs after its panic hangs in a drop): {message}",
                ago(*at)
            );
        }
        text
    }
}

fn seconds(duration: Duration) -> String {
    format!("{:.1} s", duration.as_secs_f64())
}

/// A wait or step of this test thread under way, in the watchdog's message until it is dropped.
#[must_use = "the step lasts as long as the guard"]
pub struct Step {
    trace: Trace,
    depth: usize,
}

/// Records that this test thread now does `what` (a wait's label, a stretch of simulated time),
/// until the returned guard is dropped.
pub fn step(what: impl Into<String>) -> Step {
    let trace = current();
    let mut depth = 0;
    trace.update(|state| {
        depth = state.steps.len();
        state.steps.push((what.into(), Instant::now()));
    });
    Step { trace, depth }
}

impl Drop for Step {
    fn drop(&mut self) {
        // Also the steps above it, which a panic unwound past.
        self.trace.update(|state| state.steps.truncate(self.depth));
    }
}

/// The app's frame starts on this test thread (called by the app under test, first thing).
pub fn frame_started(ctx: &egui::Context) {
    let causes = ctx.repaint_causes();
    current().update(|state| {
        state.frames += 1;
        state.frame_started = Some(Instant::now());
        state.repaint_causes = causes;
    });
}

/// The app's frame ended with `in_flight` background tasks still running.
pub fn frame_ended(in_flight: usize) {
    current().update(|state| {
        state.frame_started = None;
        state.last_frame_ended = Some(Instant::now());
        state.in_flight = Some(in_flight);
    });
}

/// Chains a panic hook that notes a panic on a test thread with a trace (one whose test took a
/// step or started a watchdog), so the watchdog can tell a test that panicked and then hung in a
/// drop.
pub(crate) fn note_panics() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let trace = TRACE.try_with(|trace| trace.get().cloned()).ok().flatten();
            if let Some(trace) = trace {
                let message = info
                    .payload()
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| info.payload().downcast_ref::<&str>().copied())
                    .unwrap_or("(no message)");
                let place = info
                    .location()
                    .map(|location| format!(" at {}:{}", location.file(), location.line()))
                    .unwrap_or_default();
                let noted = format!("{message}{place}");
                trace.update(|state| state.panic = Some((Instant::now(), noted)));
            }
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trace_names_the_steps_the_frame_and_a_panic() {
        let outer = step("waiting for the list");
        {
            let _inner = step("pump 2 s");
            frame_started(&egui::Context::default());
            frame_ended(3);
            frame_started(&egui::Context::default());
            let text = current().describe(Instant::now());
            assert!(text.contains("- waiting for the list (for"), "{text}");
            assert!(text.contains("- pump 2 s (for"), "{text}");
            assert!(text.contains("Frame 2 has been running for"), "{text}");
            assert!(text.contains("3 background tasks were in flight"), "{text}");
        }
        frame_ended(0);
        let text = current().describe(Instant::now());
        assert!(!text.contains("pump 2 s"), "the inner step ended: {text}");
        assert!(text.contains("2 frames ran; the last ended"), "{text}");
        drop(outer);

        note_panics();
        let panicked = std::panic::catch_unwind(|| {
            let _wait = step("a wait that panics");
            panic!("timed out waiting for the list");
        });
        assert!(panicked.is_err());
        let text = current().describe(Instant::now());
        assert!(text.contains("It was in no labelled wait or step."), "unwound: {text}");
        assert!(text.contains("hangs in a drop): timed out waiting for the list at"), "{text}");
    }
}
