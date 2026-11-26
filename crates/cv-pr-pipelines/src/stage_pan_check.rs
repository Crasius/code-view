//! Panning the stage canvas with the mouse, headless: a drag anywhere moves the graph 1:1, also
//! when it starts on a card or one of its controls, without clicking it; a plain click still
//! clicks; the middle button pans too; a fling coasts and then stops asking for frames.

use cv_base::theme;
use egui::{CursorIcon, Event, Modifiers, PointerButton, Pos2, Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{PipelineJob, ProviderKind};
use review_store::ThemePreference;

use super::run_view::{RunView, RunViewInput};
use super::stage_view_shots::{describe, sample_pipeline};

struct State {
    view: RunView,
    stages: Vec<PipelineJob>,
    last: Option<String>,
}

/// Smaller than the graph, so it overflows sideways; no offscreen rendering, so no GPU turn.
fn harness() -> Harness<'static, State> {
    let state = State { view: RunView::default(), stages: sample_pipeline(), last: None };
    Harness::builder()
        .with_size([900.0, 520.0])
        // Real frame times, so the pointer has a velocity to fling with.
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(240)
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            // Motion on, whatever the machine says: the fling below exists only with motion, and
            // a macOS runner may have Reduce Motion on (`motion::install` reads the OS).
            theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(
            |ui, state: &mut State| {
                theme::activate(ui.ctx(), ThemePreference::Light);
                let input = RunViewInput {
                    run_id: "build:1",
                    stages: &state.stages,
                    platform: ProviderKind::AzureDevOps,
                    has_token: true,
                    is_busy: false,
                    known: super::attempts::Known::default(),
                };
                if let Some(action) = state.view.show(ui, input) {
                    state.last = Some(describe(action));
                }
            },
            state,
        )
}

fn card(harness: &Harness<'_, State>, name: &str) -> Pos2 {
    harness.get_by_label(&format!("Stage {name}")).rect().min
}

fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }
}

/// Presses at `from`, moves to `from + by` in small steps and returns the card's travel while
/// the button is still down; then holds still (no fling) and releases.
fn drag(
    harness: &mut Harness<'_, State>,
    with: PointerButton,
    from: Pos2,
    by: Vec2,
    name: &str,
) -> Vec2 {
    const STEPS: usize = 10;
    let before = card(harness, name);
    harness.event(Event::PointerMoved(from));
    harness.step();
    harness.event(button(from, with, true));
    harness.step();
    for step in 1..=STEPS {
        harness.event(Event::PointerMoved(from + by * (step as f32 / STEPS as f32)));
        harness.step();
    }
    let moved = card(harness, name) - before;
    assert_eq!(cursor(harness), CursorIcon::Grabbing, "panning shows the closed hand");
    for _ in 0..12 {
        harness.step();
    }
    harness.event(button(from + by, with, false));
    harness.step();
    moved
}

fn cursor(harness: &Harness<'_, State>) -> CursorIcon {
    harness.output().platform_output.cursor_icon
}

fn assert_follows(moved: Vec2, by: Vec2) {
    assert!(
        (moved - by).length() < 0.5,
        "the graph follows the pointer 1:1: moved {moved:?}, dragged {by:?}"
    );
}

#[test]
fn a_drag_on_empty_canvas_pans() {
    let mut harness = harness();
    harness.run();
    // Below the only card of the first column there is nothing but canvas.
    let start = card(&harness, "Analyze") + vec2(40.0, 330.0);
    harness.event(Event::PointerMoved(start));
    harness.step();
    assert_eq!(cursor(&harness), CursorIcon::Grab, "empty canvas shows the open hand");
    harness.event(Event::PointerMoved(harness.get_by_label("Stage Build").rect().center()));
    harness.step();
    assert_ne!(cursor(&harness), CursorIcon::Grab, "a card's header is clickable, not a handle");
    let by = vec2(-180.0, -60.0);
    assert_follows(drag(&mut harness, PointerButton::Primary, start, by, "Analyze"), by);
    assert_eq!(harness.state().last, None);
}

#[test]
fn a_drag_starting_on_a_card_pans_and_does_not_click_it() {
    let mut harness = harness();
    harness.run();
    let on_header = harness.get_by_label("Stage Build").rect().center();
    let by = vec2(-150.0, -40.0);
    assert_follows(drag(&mut harness, PointerButton::Primary, on_header, by, "Build"), by);
    harness.run();
    assert_eq!(harness.state().last, None, "the header was dragged, not clicked");

    // From a job row and from a rerun button, too.
    let on_job = harness.get_by_label("Build and Tag Images").rect().center();
    let by = vec2(120.0, 30.0);
    assert_follows(drag(&mut harness, PointerButton::Primary, on_job, by, "Build"), by);
    let build_x = card(&harness, "Build").x + 20.0;
    let on_rerun = harness
        .get_all_by_label("Rerun just this stage")
        .find(|rerun| rerun.rect().x_range().contains(build_x))
        .expect("the build stage offers a rerun")
        .rect()
        .center();
    let by = vec2(-60.0, 25.0);
    assert_follows(drag(&mut harness, PointerButton::Primary, on_rerun, by, "Build"), by);
    harness.run();
    assert_eq!(harness.state().last, None, "neither the job nor the rerun was clicked");
}

#[test]
fn a_plain_click_on_a_card_still_clicks() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Stage Build").click();
    harness.run();
    assert_eq!(harness.state().last.as_deref(), Some("log Build and Tag Images 1:8"));
}

#[test]
fn a_release_mid_drag_flings_then_stops_repainting() {
    let mut harness = harness();
    harness.run();
    let start = card(&harness, "Analyze") + vec2(40.0, 330.0);
    harness.event(Event::PointerMoved(start));
    harness.step();
    harness.event(button(start, PointerButton::Primary, true));
    harness.step();
    let mut pos = start;
    for _ in 0..6 {
        pos += vec2(-25.0, 0.0);
        harness.event(Event::PointerMoved(pos));
        harness.step();
    }
    harness.event(button(pos, PointerButton::Primary, false));
    harness.step();
    let released = card(&harness, "Analyze");
    harness.run_steps(5);
    let coasted = card(&harness, "Analyze");
    assert!(
        coasted.x < released.x - 1.0,
        "the graph coasts on after the release: at {released:?} on release, {coasted:?} 5 \
         frames later, reduced motion {}",
        theme::motion::is_reduced(&harness.ctx)
    );
    harness.run_steps(120);
    let settled = card(&harness, "Analyze");
    harness.run_steps(5);
    assert_eq!(card(&harness, "Analyze"), settled, "and comes to rest");
    let delay = harness.output().viewport_output[&egui::ViewportId::ROOT].repaint_delay;
    assert_eq!(delay, std::time::Duration::MAX, "a resting canvas asks for no frames");
}

#[test]
fn shift_and_the_wheel_scroll_sideways() {
    let mut harness = harness();
    harness.run();
    let before = card(&harness, "Build");
    harness.event(Event::PointerMoved(before + vec2(0.0, 300.0)));
    harness.step();
    let wheel = Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -120.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::SHIFT,
        source: egui::MouseWheelSource::Wheel,
    };
    harness.event_modifiers(wheel, Modifiers::SHIFT);
    harness.run_steps(30);
    let moved = card(&harness, "Build") - before;
    assert!(moved.x < -100.0 && moved.y.abs() < 0.5, "Shift+wheel scrolls sideways: {moved:?}");
}

#[test]
fn keyboard_focus_brings_a_card_into_view() {
    let mut harness = harness();
    harness.run();
    let label = "Stage Deploy (integration)";
    let width = harness.ctx.content_rect().width();
    assert!(harness.get_by_label(label).rect().right() > width, "starts partly off screen");
    harness.get_by_label("Stage Infra Apps (integration environment)").focus();
    harness.run_steps(1);
    for _ in 0..40 {
        if harness.get_by_label(label).accesskit_node().is_focused() {
            break;
        }
        harness.key_press(egui::Key::Tab);
        harness.run_steps(2);
    }
    assert!(harness.get_by_label(label).accesskit_node().is_focused(), "Tab reaches the card");
    harness.run_steps(60);
    assert!(harness.get_by_label(label).rect().right() < width, "the focused card is in view");
}

#[test]
fn a_middle_drag_pans() {
    let mut harness = harness();
    harness.run();
    let on_header = harness.get_by_label("Stage Build").rect().center();
    let by = vec2(-140.0, -30.0);
    assert_follows(drag(&mut harness, PointerButton::Middle, on_header, by, "Build"), by);
    assert_eq!(harness.state().last, None);
}
