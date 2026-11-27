//! Starting the whole app in a walkthrough's harness, bounded like every other wait.
//!
//! `egui_kittest`'s `build_eframe` runs frames back to back until none asks for the next at once,
//! up to the builder's `max_steps`, and goes on whether or not the app settled. The walkthroughs
//! gave it up to a million steps so the start could finish loading. A start that never settles
//! then ran for some twenty minutes, unlabelled, until the test watchdog stopped the binary: in
//! GitLab job 25128 the overview's rows waited out a rate-limit pause an earlier test had left on
//! the pooled mock server's origin (`cv_testkit::fresh_origin`), and their updating edge asked for
//! every frame.
//!
//! [`start`] lets kittest run its first [`KITTEST_FRAMES`], then steps on the same way itself:
//! labelled for the watchdog, and failing after [`LIMIT`] with what kept asking for frames.

use std::time::{Duration, Instant};

use egui_kittest::{Harness, HarnessBuilder};

use super::ReviewApp;
use super::walkthrough::LIMIT;

/// The frames kittest itself runs when it builds the harness; its default.
const KITTEST_FRAMES: u64 = 4;

/// Builds the app with `build` and runs frames until none asks for the next at once.
pub(super) fn start(
    builder: HarnessBuilder<ReviewApp>,
    build: impl FnOnce(&mut eframe::CreationContext<'static>) -> ReviewApp,
) -> Harness<'static, ReviewApp> {
    let mut harness = first_frames(builder, build);
    settle(&mut harness);
    harness
}

/// Builds the app with `build` and runs only its first [`KITTEST_FRAMES`]: for a test that looks
/// at the app while it still loads.
pub(super) fn first_frames(
    builder: HarnessBuilder<ReviewApp>,
    build: impl FnOnce(&mut eframe::CreationContext<'static>) -> ReviewApp,
) -> Harness<'static, ReviewApp> {
    let _step = cv_testkit::step("building the app's harness");
    builder.with_max_steps(KITTEST_FRAMES).build_eframe(build)
}

/// Runs frames back to back, as kittest's own run does, until one asks for no next frame at once
/// (animations done, no answer just landed) and no image is still loading; while only images load
/// the image loaders get a moment of real time between frames.
pub(super) fn settle(harness: &mut Harness<'_, ReviewApp>) {
    let _step = cv_testkit::step("starting the app: frames until none asks for the next at once");
    let started = Instant::now();
    let mut frames: u64 = 0;
    loop {
        let has_pending_images = harness.ctx.has_pending_images();
        if !asks_for_the_next_frame(harness) {
            if !has_pending_images {
                return;
            }
            cv_testkit::let_threads_run(Duration::from_millis(10));
        }
        if started.elapsed() > LIMIT {
            let causes: Vec<String> =
                harness.ctx.repaint_causes().iter().map(ToString::to_string).collect();
            panic!(
                "the app's start did not settle in {frames} frames ({:?}): the last frame was asked \
                 for by {causes:?}, images still loading: {has_pending_images}",
                started.elapsed()
            );
        }
        harness.step();
        frames += 1;
    }
}

fn asks_for_the_next_frame(harness: &Harness<'_, ReviewApp>) -> bool {
    harness
        .output()
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .is_some_and(|viewport| viewport.repaint_delay == Duration::ZERO)
}
