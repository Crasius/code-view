//! An offset the view places past the end its scroll area knew from the last frame, in the frame
//! in which the rows grew to reach it: the view goes there and stays, it does not spring back
//! towards the old end and coast on.

use cv_code::diff_rows::Layout;
use cv_code::line_source::Pane;
use cv_pr_threads::samples;
use egui_kittest::Harness;
use review_core::ThreadStatus;

use crate::testing::{Fixture, PATH, fixture_of, harness_sized};

/// 200 lines, every fourth one changed, with a comment on each of the first three changes.
fn commented_at_the_top() -> Fixture {
    let line = |index: usize, changed: bool| {
        format!(
            "    let value_{index} = compute({index}, {});\n",
            if changed { index * 7 } else { index }
        )
    };
    let old: String = (0..200).map(|index| line(index, false)).collect();
    let new: String = (0..200).map(|index| line(index, index % 4 == 0)).collect();
    let mut fixture = fixture_of(Layout::Inline, Some(&old), Some(&new));
    fixture.threads = [1, 5, 9]
        .into_iter()
        .map(|line| {
            let id = format!("thread-{line}");
            let body = format!("A comment on line {line}.");
            let comment = samples::comment(&format!("{id}-c"), &samples::mara(), 5, &body);
            let lines = samples::lines(PATH, line, line);
            samples::thread(&id, ThreadStatus::Active, Some(lines), vec![comment])
        })
        .collect();
    fixture.show_threads = false;
    fixture.past_end = false;
    fixture
}

/// The top of new line `line`'s text on screen.
fn top_of(harness: &Harness<'_, Fixture>, line: u32) -> f32 {
    let state = harness.state();
    let row = state.view.row_of(&state.diff, None, Some(line));
    state.view.text_area(Pane::Inline, row).rect.top()
}

/// At the end of the rows, comments switched on put their cards above the view: the view moves
/// down by their height in the frame they appear, past the end of the frame before, and the last
/// line stays where it was on screen in every frame after.
#[test]
fn comments_switched_on_at_the_end_keep_the_last_line_in_place() {
    let mut harness = harness_sized(commented_at_the_top(), [1180.0, 420.0]);
    harness.run();
    harness.hover_at(egui::pos2(600.0, 200.0));
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -100_000.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Wheel,
    });
    harness.run();
    let before = top_of(&harness, 200);
    assert!(before > 0.0 && before < 420.0, "the last line is in view: {before}");

    harness.state_mut().show_threads = true;
    for step in 0..12 {
        harness.step();
        let after = top_of(&harness, 200);
        assert!(
            (after - before).abs() < 0.5,
            "step {step} after comments showed: the last line moved from {before} to {after}"
        );
    }
}
