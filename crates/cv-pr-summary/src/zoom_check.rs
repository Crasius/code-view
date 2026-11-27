//! The code zoom over the folder summary, with the app's zoom handler: Ctrl+wheel and
//! Ctrl+Plus/Minus/0 over a card's rows change the code size (not the app's zoom) and keep the line
//! under the pointer where it is; over the summary's header they zoom the app.

use egui::{Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, Pos2, TouchPhase, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_git::{ChangeKind, ChangedFile};
use review_store::{AppSettings, ThemePreference};

use super::MARGIN;
use super::testing::{Scene, card_of, file, harness, settle};
use cv_code::line_source::Pane;

const PATH: &str = "src/app/cart.rs";
const SIZE: [f32; 2] = [1180.0, 860.0];
/// Head line 125 of [`PATH`] sits this far below the top of the view before zooming.
const BELOW_TOP: f32 = 320.0;

fn files() -> Vec<ChangedFile> {
    vec![
        file("src/app/billing.rs", ChangeKind::Modified, Some((10, 6))),
        file(PATH, ChangeKind::Modified, Some((30, 18))),
        file("src/app/tax.rs", ChangeKind::Modified, Some((10, 6))),
    ]
}

/// Where head line 125 of [`PATH`] shows.
fn line(scene: &Scene) -> egui::Rect {
    let body = scene.summary.bodies.get(PATH).expect("the card's diff is loaded");
    body.view.text_area(Pane::Inline, body.view.row_of(&body.diff, None, Some(125))).rect
}

/// The summary with head line 125 [`BELOW_TOP`] below the top of the view and the pointer on it.
fn summary_on_the_line() -> (Harness<'static, Scene>, Pos2) {
    let mut harness = harness(Scene::new(files(), ThemePreference::Light), SIZE);
    settle(&mut harness);
    let card = card_of(harness.state(), PATH);
    let top = harness.state().summary.index.top(card);
    harness.state_mut().summary.scroll_to = Some(top + MARGIN);
    settle(&mut harness);
    let delta = line(harness.state()).top() - harness.state().summary.screen.top() - BELOW_TOP;
    let summary = &mut harness.state_mut().summary;
    summary.scroll_to = Some(summary.viewport.top() + MARGIN + delta);
    settle(&mut harness);
    let pointer = line(harness.state()).center();
    harness.hover_at(pointer);
    harness.run_steps(2);
    (harness, pointer)
}

fn ctrl_wheel(harness: &mut Harness<'_, Scene>, notches: f32) {
    harness.event(Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: vec2(0.0, notches),
        phase: TouchPhase::Move,
        modifiers: Modifiers::COMMAND,
        source: MouseWheelSource::Wheel,
    });
    harness.step();
}

fn levels(harness: &Harness<'_, Scene>) -> (f32, f32) {
    let settings = &harness.state().settings;
    (settings.code_font_size, settings.ui_scale)
}

/// Ctrl+wheel over a card's code makes the code larger, not the app, and the line under the
/// pointer stays under it, in the frame the size changes and once the cards settle; back down,
/// it is still there.
#[test]
fn ctrl_wheel_over_a_card_zooms_its_code_around_the_pointer() {
    let (mut harness, pointer) = summary_on_the_line();
    let before = line(harness.state());
    ctrl_wheel(&mut harness, 3.0);
    assert_eq!(
        levels(&harness),
        (AppSettings::DEFAULT_CODE_FONT_SIZE + 3.0, 1.0),
        "the code zooms, the app does not"
    );
    let zoomed = line(harness.state());
    assert!(
        zoomed.height() > before.height() + 2.0,
        "the line got taller: {before:?} to {zoomed:?}"
    );
    assert!(
        zoomed.y_range().contains(pointer.y),
        "the line under the pointer stays under it in the same frame: {zoomed:?}, pointer {pointer:?}"
    );
    settle(&mut harness);
    let settled = line(harness.state());
    assert!(
        (settled.center().y - pointer.y).abs() < 2.0,
        "and once the cards settle: {settled:?}, pointer {pointer:?}"
    );
    assert!(harness.query_by_label("Code 16 px").is_some(), "the overlay names the code size");

    ctrl_wheel(&mut harness, -3.0);
    settle(&mut harness);
    let back = line(harness.state());
    assert_eq!(levels(&harness), (AppSettings::DEFAULT_CODE_FONT_SIZE, 1.0));
    assert!(
        (back.center().y - pointer.y).abs() < 2.0,
        "back to the old size, the line is under the pointer: {back:?}, pointer {pointer:?}"
    );
}

/// Ctrl+Plus and Ctrl+0 with the pointer on a card's code and no widget focused go to the code as
/// well, around the pointer.
#[test]
fn ctrl_plus_with_the_pointer_on_a_card_zooms_its_code() {
    let (mut harness, pointer) = summary_on_the_line();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Plus);
    settle(&mut harness);
    assert_eq!(levels(&harness), (AppSettings::DEFAULT_CODE_FONT_SIZE + 1.0, 1.0));
    let zoomed = line(harness.state());
    assert!(
        (zoomed.center().y - pointer.y).abs() < 2.0,
        "the line stays under the pointer: {zoomed:?}, pointer {pointer:?}"
    );
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num0);
    settle(&mut harness);
    assert_eq!(
        levels(&harness),
        (AppSettings::DEFAULT_CODE_FONT_SIZE, 1.0),
        "Ctrl+0 resets the code size"
    );
}

/// Over the summary's header, which is no code, Ctrl+wheel zooms the whole app and leaves the code
/// size alone.
#[test]
fn ctrl_wheel_over_the_header_zooms_the_app() {
    let (mut harness, _) = summary_on_the_line();
    let header = harness.get_by_label("3 changed files").rect().center();
    harness.hover_at(header);
    harness.run_steps(2);
    ctrl_wheel(&mut harness, 1.0);
    assert_eq!(levels(&harness), (AppSettings::DEFAULT_CODE_FONT_SIZE, 1.1));
}
