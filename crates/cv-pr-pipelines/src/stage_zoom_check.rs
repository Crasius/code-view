//! Zooming the stage canvas, headless: Ctrl+wheel zooms around the pointer and stops at 25 % and
//! 200 %, the corner control and Ctrl+Plus/Minus/0 step and reset, Fit shows every stage, a job
//! still opens on click at 50 % and 200 %, as does an attempt picked from the popup of a job's
//! attempt badge, and nothing drifts between frames.

use cv_base::theme;
use cv_testkit::Clicks;
use egui::{Event, Key, Modifiers, Pos2, Rect, Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{AttemptSource, PipelineAttempt, PipelineJob, ProviderKind};
use review_store::ThemePreference;

use super::run_view::{RunView, RunViewInput};
use super::stage_view_shots::{describe, sample_pipeline};

struct State {
    view: RunView,
    stages: Vec<PipelineJob>,
    last: Option<String>,
}

const STAGES: [&str; 7] = [
    "Analyze",
    "Build",
    "Infra Shared",
    "Infra Apps (integration environment)",
    "Deploy (integration)",
    "PR Comment (integration)",
    "Cleanup",
];

/// Jobs of the sample run and the log each opens.
const JOBS: [(&str, &str); 6] = [
    ("Analyze code", "1:4"),
    ("Build and Tag Images", "1:8"),
    ("Pulumi Up", "1:9"),
    ("Apply infrastructure in Azure", "1:12"),
    ("Read outputs and check the deployment health", "1:13"),
    ("Remove preview resources", "1:20"),
];

/// The failed infrastructure job ran twice; its first attempt loads on demand.
const RETRIED_JOB: &str = "Apply infrastructure in Azure";

fn with_retried_job(mut stages: Vec<PipelineJob>) -> Vec<PipelineJob> {
    let job = stages
        .iter_mut()
        .flat_map(|stage| &mut stage.children)
        .find(|job| job.name == RETRIED_JOB)
        .expect("the sample has the job");
    job.attempt = 2;
    job.previous_attempts =
        vec![PipelineAttempt { number: 1, source: AttemptSource::Lazy("attempt-1".to_owned()) }];
    stages
}

/// Smaller than the graph, so it overflows.
fn harness() -> Harness<'static, State> {
    let state =
        State { view: RunView::default(), stages: with_retried_job(sample_pipeline()), last: None };
    Harness::builder()
        .with_size([1100.0, 620.0])
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(240)
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            // Motion on, whatever the machine says: the zoom steps glide.
            theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(
            |ui, state: &mut State| {
                let palette = theme::activate(ui.ctx(), ThemePreference::Light);
                ui.painter().rect_filled(ui.max_rect(), 0, palette.ground);
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

fn header(harness: &Harness<'_, State>, stage: &str) -> Rect {
    harness.get_by_label(&format!("Stage {stage}")).rect()
}

fn level(harness: &Harness<'_, State>, percent: u32) -> bool {
    harness.query_by_label(&format!("Zoom {percent} %, reset to 100 %")).is_some()
}

fn ctrl_wheel(harness: &mut Harness<'_, State>, at: Pos2, notches: f32) {
    harness.event(Event::PointerMoved(at));
    harness.step();
    let wheel = Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: vec2(0.0, notches),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::COMMAND,
        source: egui::MouseWheelSource::Wheel,
    };
    harness.event_modifiers(wheel, Modifiers::COMMAND);
    harness.run();
}

/// Where `point` sits inside `rect`, from 0 to 1 per axis.
fn relative(point: Pos2, rect: Rect) -> Vec2 {
    (point - rect.min) / rect.size()
}

/// The canvas on screen: below the pivot row, left of nothing, minus the zoom control.
fn canvas(harness: &Harness<'_, State>) -> Rect {
    let content = harness.ctx.content_rect();
    let pivot = harness.get_by_label("Stages").rect();
    Rect::from_min_max(egui::pos2(content.left(), pivot.bottom() + 2.0), content.max)
}

/// Clicks the control labelled `label` (not its tooltip, which comes later in the tree).
fn click(harness: &mut Harness<'_, State>, label: &str, times: usize) {
    for _ in 0..times {
        harness.click_at(
            harness.get_all_by_label_contains(label).next().expect("the control is shown").rect(),
        );
        harness.run();
    }
}

/// Moves the pointer onto empty canvas, so no control is hovered and no tooltip waits.
fn rest_pointer(harness: &mut Harness<'_, State>) {
    harness.event(Event::PointerMoved(canvas(harness).left_top() + vec2(4.0, 4.0)));
    harness.run();
}

#[test]
fn ctrl_wheel_zooms_around_the_pointer() {
    let mut harness = harness();
    harness.run();
    let before = header(&harness, "Build");
    let at = before.min + vec2(170.0, 20.0);
    ctrl_wheel(&mut harness, at, 2.0);
    let after = header(&harness, "Build");
    assert!(after.width() > before.width() * 1.2, "zoomed in: {before:?} -> {after:?}");
    let drift = (relative(at, after) - relative(at, before)) * after.size();
    assert!(drift.length() < 1.0, "the point under the pointer stays put, drifted {drift:?}");

    ctrl_wheel(&mut harness, at, -4.0);
    let out = header(&harness, "Build");
    assert!(out.width() < before.width() * 0.8, "zoomed out: {out:?}");
    let drift = (relative(at, out) - relative(at, before)) * out.size();
    assert!(drift.length() < 1.0, "and back out around the same point, drifted {drift:?}");
    assert_eq!(harness.state().last, None, "zooming clicks nothing");
}

#[test]
fn zoom_stops_at_25_and_200_percent() {
    let mut harness = harness();
    harness.run();
    let at = header(&harness, "Build").center();
    for _ in 0..6 {
        ctrl_wheel(&mut harness, at, 4.0);
    }
    assert!(level(&harness, 200), "the level reads 200 %");
    assert!((header(&harness, "Build").width() - 480.0).abs() < 0.5, "cards are twice as wide");
    for _ in 0..12 {
        ctrl_wheel(&mut harness, at, -4.0);
    }
    assert!(level(&harness, 25), "the level reads 25 %");
    assert!((header(&harness, "Build").width() - 60.0).abs() < 0.5, "cards are a quarter as wide");
    // Panned and zoomed out, some of the graph is still on screen.
    let view = canvas(&harness);
    assert!(STAGES.iter().any(|stage| view.intersects(header(&harness, stage))));
}

#[test]
fn the_control_and_shortcuts_step_and_reset() {
    let mut harness = harness();
    harness.run();
    assert!(level(&harness, 100));
    click(&mut harness, "Zoom in", 1);
    assert!(level(&harness, 110));
    click(&mut harness, "Zoom out", 2);
    assert!(level(&harness, 90));
    click(&mut harness, "reset to 100 %", 1);
    assert!(level(&harness, 100));
    assert!((header(&harness, "Build").width() - 240.0).abs() < 0.01, "back to the exact size");

    harness.event(Event::PointerMoved(header(&harness, "Build").center()));
    harness.step();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Equals);
    harness.run();
    assert!(level(&harness, 110), "Ctrl+= zooms in over the canvas");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Minus);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Minus);
    harness.run();
    assert!(level(&harness, 90), "Ctrl+- zooms out");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num0);
    harness.run();
    assert!(level(&harness, 100), "Ctrl+0 resets");
}

#[test]
fn fit_shows_every_stage() {
    let mut harness = harness();
    harness.run();
    let view = canvas(&harness);
    assert!(
        !view.contains_rect(header(&harness, "Deploy (integration)")),
        "at 100 % the graph overflows"
    );
    click(&mut harness, "Fit", 1);
    for stage in STAGES {
        let rect = header(&harness, stage);
        assert!(view.contains_rect(rect), "{stage} is in view after Fit: {rect:?} in {view:?}");
    }
    assert!(!level(&harness, 100), "Fit zoomed out");
}

#[test]
fn a_job_opens_on_click_at_50_and_200_percent() {
    let mut harness = harness();
    harness.run();
    for (steps, label, percent) in [(4, "Zoom out", 50), (5, "Zoom in", 200)] {
        click(&mut harness, "reset to 100 %", 1);
        click(&mut harness, label, steps);
        assert!(level(&harness, percent), "at {percent} %");
        let control = harness.get_by_label_contains("reset to 100 %").rect().expand(48.0);
        let view = canvas(&harness).shrink(8.0);
        // Rows far off screen are not built at all.
        let (job, log) = JOBS
            .iter()
            .find(|(job, _)| {
                harness.query_by_label(job).is_some_and(|row| {
                    let center = row.rect().center();
                    view.contains(center) && !control.contains(center)
                })
            })
            .unwrap_or_else(|| panic!("a job row is on screen at {percent} %"));
        harness.state_mut().last = None;
        harness.click_at(harness.get_by_label(job).rect());
        harness.run();
        assert_eq!(
            harness.state().last.as_deref(),
            Some(format!("log {job} {log}").as_str()),
            "at {percent} %"
        );
    }
}

/// The attempt list is a popup on its own, untransformed layer, opened from a badge on the
/// zoomed canvas: it opens next to the badge and its options take clicks at any zoom.
#[test]
fn an_attempt_opens_from_the_badge_popup_at_50_and_200_percent() {
    let mut harness = harness();
    harness.run();
    let badge_label = format!("{RETRIED_JOB}, 2 attempts. Switch attempt");
    for (key, steps, percent) in [(Key::Minus, 4, 50), (Key::Equals, 5, 200)] {
        // The zoom keys zoom around the pointer, so the badge stays where it is.
        harness.event(Event::PointerMoved(harness.get_by_label(&badge_label).rect().center()));
        harness.step();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::Num0);
        harness.run();
        for _ in 0..steps {
            harness.key_press_modifiers(Modifiers::COMMAND, key);
            harness.run();
        }
        assert!(level(&harness, percent), "at {percent} %");
        let badge = harness.get_by_label(&badge_label).rect();
        assert!(
            canvas(&harness).contains_rect(badge),
            "the badge is on screen at {percent} %: {badge:?}"
        );
        harness.state_mut().last = None;
        harness.click_at(harness.get_by_label(&badge_label).rect());
        harness.run();
        let option = harness.get_by_label_contains("Attempt 1, opens on demand").rect();
        assert!(
            option.top() >= badge.top() - 1.0 && (option.top() - badge.bottom()).abs() < 120.0,
            "the list opens at the badge at {percent} %: {option:?} below {badge:?}"
        );
        harness.click_at(harness.get_by_label_contains("Attempt 1, opens on demand").rect());
        harness.run();
        assert_eq!(
            harness.state().last.as_deref(),
            Some(format!("attempt 1 of job-{RETRIED_JOB}").as_str()),
            "at {percent} %"
        );
    }
}

#[test]
fn rects_hold_still_across_frames() {
    let mut harness = harness();
    harness.run();
    for (steps, label) in [(4, "Zoom out"), (9, "Zoom in")] {
        click(&mut harness, label, steps);
        rest_pointer(&mut harness);
        let rects = |harness: &Harness<'_, State>| {
            STAGES.map(|stage| {
                harness.query_by_label(&format!("Stage {stage}")).map(|node| node.rect())
            })
        };
        let settled = rects(&harness);
        harness.step();
        harness.step();
        assert_eq!(rects(&harness), settled, "after {label}");
        let delay = harness.output().viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        assert_eq!(delay, std::time::Duration::MAX, "a resting zoom asks for no frames");
    }
}
