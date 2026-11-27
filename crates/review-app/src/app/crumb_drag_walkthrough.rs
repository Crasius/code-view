//! A long press on a breadcrumb moves the window: a short click still navigates, a press held
//! still past [`platform::LONG_PRESS`] sends one `StartDrag`, shows the grabbing hand and leads
//! nowhere on release, a press that moves first does neither, and the current page's crumb moves
//! the window too. Asserts the viewport commands and the cursor the app sends; the window itself
//! does not move in a test. No network, no tokens.

use egui::{CursorIcon, Event, Modifiers, PointerButton, Pos2, Role, ViewportCommand, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::walkthrough::{app_with, wait_until};
use cv_base::platform;
use cv_runtime::route::Route;
use cv_testkit::ScratchDir;

const STEP: f32 = 1.0 / 60.0;
/// The page the walkthrough starts on: its trail is the app's name (a link) and itself.
const PAGE: &str = "Repositories";
/// The frames after the release that a gesture still counts: a click navigates in them, and a
/// second window drag a late release sent would show in them.
const RELEASE_FRAMES: usize = 3;

/// What the frames of one gesture sent the window, and the cursors they asked for.
#[derive(Default)]
struct Gesture {
    drags: usize,
    grabbing_frames: usize,
}

fn harness(scratch: &ScratchDir) -> Harness<'static, ReviewApp> {
    let builder = Harness::builder().with_size([1200.0, 480.0]).with_step_dt(STEP);
    let mut harness = app_with(builder, scratch.app_paths(), cv_providers::create_provider);
    go_to_page(&mut harness);
    harness
}

/// Goes to [`PAGE`] and waits until its crumb is the current page.
fn go_to_page(harness: &mut Harness<'_, ReviewApp>) {
    harness.state_mut().navigator.go(Route::Repositories);
    wait_until(harness, "the current page's crumb", |harness| current_crumb(harness).is_some());
}

/// One frame, counting the window drags it sent and whether it showed the grabbing hand.
fn step(harness: &mut Harness<'_, ReviewApp>, gesture: &mut Gesture) {
    harness.step();
    let output = harness.output();
    gesture.drags += output.viewport_output.get(&ViewportId::ROOT).map_or(0, |viewport| {
        viewport
            .commands
            .iter()
            .filter(|command| matches!(command, ViewportCommand::StartDrag))
            .count()
    });
    if output.platform_output.cursor_icon == CursorIcon::Grabbing {
        gesture.grabbing_frames += 1;
    }
}

fn button(harness: &mut Harness<'_, ReviewApp>, pos: Pos2, pressed: bool) {
    harness.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

/// Presses at `from`, moves to `to` on the next frame, holds for `seconds` and releases there.
fn press(harness: &mut Harness<'_, ReviewApp>, from: Pos2, to: Pos2, seconds: f32) -> Gesture {
    let mut gesture = Gesture::default();
    harness.hover_at(from);
    step(harness, &mut gesture);
    button(harness, from, true);
    step(harness, &mut gesture);
    if to != from {
        harness.hover_at(to);
        step(harness, &mut gesture);
    }
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a few frames")]
    let frames = (seconds / STEP).ceil() as usize;
    for _ in 0..frames {
        step(harness, &mut gesture);
    }
    button(harness, to, false);
    for _ in 0..RELEASE_FRAMES {
        step(harness, &mut gesture);
    }
    gesture
}

fn crumb(harness: &Harness<'_, ReviewApp>, role: Role, label: &str) -> Pos2 {
    harness.get_by_role_and_label(role, label).rect().center()
}

fn current_crumb(harness: &Harness<'_, ReviewApp>) -> Option<Pos2> {
    let value = format!("{PAGE} (current page)");
    harness
        .query_by(|node| node.value().as_deref() == Some(value.as_str()))
        .map(|node| node.rect().center())
}

#[test]
fn long_press_on_a_breadcrumb_moves_the_window_instead_of_navigating() {
    let scratch = ScratchDir::new("crumb-drag");
    let mut harness = harness(&scratch);
    let long = platform::LONG_PRESS.as_secs_f32();

    // A short click navigates and moves nothing.
    let app = crumb(&harness, Role::Link, review_core::APP_NAME);
    let gesture = press(&mut harness, app, app, 0.05);
    assert_eq!(harness.state().route, Route::MyPullRequests, "a click on the app's name goes home");
    assert_eq!(gesture.drags, 0, "a click moves no window");
    assert_eq!(gesture.grabbing_frames, 0, "a click shows no grabbing hand");

    // Held still past the long press: one window drag, the grabbing hand, no navigation.
    go_to_page(&mut harness);
    let app = crumb(&harness, Role::Link, review_core::APP_NAME);
    let gesture = press(&mut harness, app, app, long + 0.2);
    assert_eq!(gesture.drags, 1, "a long press starts exactly one window drag");
    assert!(gesture.grabbing_frames > 0, "the cursor is the grabbing hand while armed");
    assert_eq!(harness.state().route, Route::Repositories, "the release after it leads nowhere");

    // Moved past the click distance before the time is up: neither a drag nor a navigation.
    let moved = app + egui::vec2(24.0, 0.0);
    let gesture = press(&mut harness, app, moved, long + 0.2);
    assert_eq!(gesture.drags, 0, "a press that moves first never moves the window");
    assert_eq!(gesture.grabbing_frames, 0);
    assert_eq!(harness.state().route, Route::Repositories, "nor is it a click");

    // The current page's crumb is no link, yet held it moves the window too.
    let page = current_crumb(&harness).expect("the current page's crumb");
    let gesture = press(&mut harness, page, page, long + 0.2);
    assert_eq!(gesture.drags, 1, "the current page's crumb moves the window when held");
    assert_eq!(harness.state().route, Route::Repositories);

    // Afterwards a short click navigates again.
    let app = crumb(&harness, Role::Link, review_core::APP_NAME);
    let gesture = press(&mut harness, app, app, 0.05);
    assert_eq!(gesture.drags, 0);
    assert_eq!(harness.state().route, Route::MyPullRequests, "the next click navigates again");
}
