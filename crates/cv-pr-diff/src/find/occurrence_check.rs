//! The other occurrences of the selected text, driven with the pointer and keyboard like a user:
//! a double-clicked word marks every whole-word occurrence in the file, on both sides and inside
//! collapsed unchanged lines; blanks, several lines and the setting turned off mark none. A query
//! in the find widget leaves them be: its hits show beside them in another colour.

use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use review_store::ThemePreference;

use crate::find::bar::last_bar;
use crate::find::text::OccurrenceRows;
use crate::testing::{Fixture, fixture_of, harness, settle};
use cv_base::theme;
use cv_code::diff_rows::Layout;
use cv_code::find_paint::{self as paint, MarkLook, last_marks};
use cv_code::line_source::Pane;

/// The line that changes; "count" is on it in both versions.
const CHANGED: u32 = 30;
/// Unchanged lines with "count", hidden in the collapsed gaps above and below the change.
const HIDDEN: [u32; 2] = [5, 55];
/// Every whole-word "count" in the file: each hidden line once, the changed line on each side.
const OCCURRENCES: usize = 4;

/// Sixty lines; `CHANGED` differs between the versions, "counter" is never a whole-word match.
fn text(changed: &str) -> String {
    let line = |number: u32| match number {
        5 => "    let count = 5;".to_owned(),
        CHANGED => changed.to_owned(),
        31 => "    let counter = 0;".to_owned(),
        55 => "    let shown = count;".to_owned(),
        number => format!("    let filler_{number} = {number};"),
    };
    (1..=60).map(|number| line(number) + "\n").collect()
}

fn occurrence_fixture() -> Fixture {
    let (old, new) = (text("    let total = count + 1;"), text("    let total = count * 2;"));
    fixture_of(Layout::SideBySide, Some(&old), Some(&new))
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// Clicks `count` times at `pos` within one frame, as a double-click comes in.
fn multi_click(harness: &mut Harness<'_, Fixture>, pos: Pos2, count: usize) {
    harness.hover_at(pos);
    harness.step();
    for _ in 0..count {
        harness.input_mut().events.push(button(pos, true));
        harness.input_mut().events.push(button(pos, false));
    }
    harness.step();
}

fn drag(harness: &mut Harness<'_, Fixture>, from: Pos2, to: Pos2) {
    harness.hover_at(from);
    harness.step();
    harness.event(button(from, true));
    harness.step();
    harness.hover_at(to);
    harness.step();
    harness.event(button(to, false));
    harness.step();
}

/// Steps until the occurrences are found; their rows and overview ticks.
fn wait_for_occurrences(harness: &mut Harness<'_, Fixture>) -> OccurrenceRows {
    cv_testkit::poll_for("the occurrences", || {
        harness.step();
        harness.state().view.occurrences()
    })
}

fn has_occurrences(harness: &Harness<'_, Fixture>) -> bool {
    harness.state().view.occurrences().is_some()
}

#[test]
fn a_selected_word_marks_its_other_occurrences_in_the_whole_file() {
    let mut harness = harness(occurrence_fixture());
    settle(&mut harness);
    let state = harness.state();
    let changed = state.view.row_of(&state.diff, None, Some(CHANGED));
    let word = state.view.text_point(Pane::Right, changed, 18);

    multi_click(&mut harness, word, 2);
    let OccurrenceRows { rows, ticks, gap_rows: gaps } = wait_for_occurrences(&mut harness);
    assert_eq!(gaps.len(), HIDDEN.len(), "the unchanged lines around the change are collapsed");
    assert_eq!(rows.len(), OCCURRENCES, "every whole-word occurrence on both sides, once each");
    for gap in &gaps {
        assert!(rows.contains(&Some(*gap)), "an occurrence in a collapsed gap is at its row");
        assert!(ticks.contains(gap), "and ticks on the overview strip there");
    }

    // A click elsewhere clears the selection, and the marks with it on the next frame (the one
    // egui runs after input).
    let other = harness.state().view.text_point(Pane::Right, changed + 2, 2);
    multi_click(&mut harness, other, 1);
    harness.step();
    assert!(!has_occurrences(&harness), "no selection, no occurrences");

    // Blanks mark nothing, nor does a selection over two lines.
    let blanks = (0..2).map(|column| harness.state().view.text_point(Pane::Right, changed, column));
    let [from, to] = <[Pos2; 2]>::try_from(blanks.collect::<Vec<_>>()).expect("two points");
    drag(&mut harness, from, to);
    assert!(!has_occurrences(&harness), "a selection of blanks marks nothing");
    let below = harness.state().view.text_point(Pane::Right, changed + 1, 8);
    drag(&mut harness, word, below);
    assert!(!has_occurrences(&harness), "a selection over two lines marks nothing");

    // A query in the find widget leaves them be, and so does closing it.
    multi_click(&mut harness, word, 2);
    wait_for_occurrences(&mut harness);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::F);
    harness.step();
    assert!(has_occurrences(&harness), "the find widget's query keeps the occurrences");
    harness.key_press(Key::Escape);
    let rows = wait_for_occurrences(&mut harness).rows;
    assert_eq!(rows.len(), OCCURRENCES, "and so does closing it");

    // The setting turned off marks nothing.
    harness.state_mut().find.highlight_occurrences = false;
    harness.step();
    assert!(!has_occurrences(&harness), "the setting turned off marks nothing");
}

/// Steps until the find widget's count ends in `suffix`.
fn wait_for_count(harness: &mut Harness<'_, Fixture>, suffix: &str) {
    cv_testkit::poll_until(&format!("the find count to end in {suffix:?}"), || {
        harness.step();
        last_bar(&harness.ctx).is_some_and(|shown| shown.count.ends_with(suffix))
    });
}

/// C#-like lines (highlighted as Rust): `endsTheContract` declared on line 28 and tested on
/// line 29, both unchanged and shown as context around the change on line 31.
fn contract_fixture() -> Fixture {
    let text = |result: &str| -> String {
        let line = |number: u32| match number {
            28 => "        var endsTheContract = act is Ending;".to_owned(),
            29 => "        if (!endsTheContract)".to_owned(),
            31 => format!("            return {result};"),
            number => format!("        int filler{number} = {number};"),
        };
        (1..=60).map(|number| line(number) + "\n").collect()
    };
    let (old, new) = (text("1"), text("2"));
    fixture_of(Layout::SideBySide, Some(&old), Some(&new))
}

/// The owner's gesture: with a find query showing its hits, drag over an identifier in a context
/// row and let go. Its occurrences show on the line above, in that pane and in the other one, in
/// the occurrence fill, beside the find hits in theirs, in both themes. (They showed nothing
/// while the find widget had a query.)
#[test]
fn a_dragged_identifier_marks_its_occurrences_beside_the_find_hits() {
    for preference in [ThemePreference::Light, ThemePreference::Dark] {
        let mut harness = harness(Fixture { theme: preference, ..contract_fixture() });
        settle(&mut harness);
        let state = harness.state();
        let (declared, tested) = (
            state.view.row_of(&state.diff, Some(28), Some(28)),
            state.view.row_of(&state.diff, Some(29), Some(29)),
        );
        let left = state.view.text_point(Pane::Left, tested, 2);
        let from = state.view.text_point(Pane::Right, tested, 13);
        let to = state.view.text_point(Pane::Right, tested, 28);
        let split = state.view.text_point(Pane::Right, tested, 0).x;
        let rows_y = {
            let top = state.view.text_point(Pane::Left, declared, 0).y;
            top - 12.0..state.view.text_point(Pane::Left, tested, 0).y + 12.0
        };
        // A find query over the left pane: "return" on the changed line.
        harness.hover_at(left);
        harness.step();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::F);
        harness.step();
        harness.event(Event::Text("return".to_owned()));
        wait_for_count(&mut harness, " of 1");

        drag(&mut harness, from, to);
        wait_for_occurrences(&mut harness);
        harness.run();
        let palette = theme::palette(&harness.ctx);
        let marks = last_marks(&harness.ctx);
        let occurrences: Vec<_> =
            marks.iter().filter(|(look, _, _)| *look == MarkLook::Occurrence).collect();
        let on_rows = |rect: &egui::Rect| rows_y.contains(&rect.center().y);
        let in_left =
            occurrences.iter().filter(|(_, rect, _)| on_rows(rect) && rect.right() < split);
        let in_right =
            occurrences.iter().filter(|(_, rect, _)| on_rows(rect) && rect.left() > split);
        assert!(in_left.count() >= 2, "{preference:?}: both lines in the left pane: {marks:?}");
        assert!(in_right.count() >= 1, "{preference:?}: the line above in its own pane: {marks:?}");
        assert!(occurrences.iter().all(|(_, _, color)| *color == palette.occurrence));
        assert_ne!(palette.occurrence, palette.surface, "not the row's background");
        let hit = marks.iter().find(|(look, _, _)| *look != MarkLook::Occurrence);
        let (_, _, hit_color) = hit.expect("the find hit is painted too");
        assert_ne!(*hit_color, palette.occurrence, "hits and occurrences differ");
        assert!(
            *hit_color == paint::fill(&palette, true) || *hit_color == paint::fill(&palette, false)
        );
    }
}
