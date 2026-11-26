//! "Scroll past the end" in the text diff, driven like a user: the wheel takes the view on until
//! the last line is at its top, while the overview strip measures the rows alone; dragging the
//! strip's view box to the bottom stops at the end, a short file does not scroll, and with the
//! setting off the view stops where it always did. The strip's context menu switches it. New only
//! (its last removed lines no rows of their own) and a whole added file scroll as far.

use std::fmt::Write as _;
use std::sync::Arc;

use egui::{
    Event, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, Rect, pos2, vec2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use review_core::DiffSide;
use review_git::FileDiff;

use super::{PAST_END_LABEL, last_view_box, past_end};
use crate::diff_view::OVERVIEW_WIDTH;
use crate::testing::{Fixture, fixture, harness, settle, side};
use cv_code::contents::{Coloring, FileContents};
use cv_code::diff_rows::Layout;

/// The harness's size (see [`harness`]).
const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;
/// Over the code, away from the strip.
const OVER_CODE: Pos2 = pos2(500.0, 400.0);
/// Lines of the long file.
const LINES: u32 = 240;

/// A file of `lines` lines with one line changed in the middle, every line shown.
fn file_fixture(lines: u32) -> Fixture {
    let mut old = String::new();
    let mut new = String::new();
    for line in 1..=lines {
        writeln!(old, "    let value_{line} = {line};").expect("a string takes text");
        let changed = if line == lines / 2 { line * 10 } else { line };
        writeln!(new, "    let value_{line} = {changed};").expect("a string takes text");
    }
    let unified =
        similar::TextDiff::from_lines(&old, &new).unified_diff().context_radius(10_000).to_string();
    let mut fixture = fixture(Layout::SideBySide);
    fixture.diff = Arc::new(FileDiff::parse_unified(unified.as_bytes()));
    fixture.contents = FileContents {
        old: Some(side(&old)),
        new: Some(side(&new)),
        coloring: Coloring::Done,
        error: None,
    };
    fixture
}

/// The harness for a file of `lines` lines, with "Scroll past the end" `on` or off, settled.
fn shown(lines: u32, on: bool) -> Harness<'static, Fixture> {
    let mut harness = harness(file_fixture(lines));
    past_end::set_for_test(&harness.ctx, on);
    settle(&mut harness);
    harness
}

fn wheel(harness: &mut Harness<'_, Fixture>, delta: f32) {
    harness.event(Event::PointerMoved(OVER_CODE));
    harness.event(Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, delta),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
        source: MouseWheelSource::Wheel,
    });
    // The wheel's smooth scroll eases over a few frames.
    harness.run_steps(20);
}

/// Nothing repaints once the view rests: the frames settle within the harness's few steps.
fn assert_idle(harness: &mut Harness<'_, Fixture>, when: &str) {
    if let Err(error) = harness.try_run() {
        panic!("{when}: the view keeps repainting: {error:?}");
    }
}

/// The top of the last line's row, from the top of the first row.
fn last_row_top(harness: &Harness<'_, Fixture>) -> f32 {
    let state = harness.state();
    state.view.row_top(state.view.row_of(&state.diff, Some(LINES), Some(LINES)))
}

/// The strip and its view box; the strip spans the rows' view.
fn strip(harness: &Harness<'_, Fixture>) -> (Rect, Rect) {
    last_view_box(&harness.ctx).expect("the rows scroll")
}

/// Where the view stops without room past the end: the last line at the bottom.
fn end(harness: &Harness<'_, Fixture>) -> f32 {
    harness.state().view.height() - strip(harness).0.height()
}

/// The strip's view box rests at the bottom of the strip, as tall as the view's share of the rows.
fn assert_box_at_bottom(harness: &Harness<'_, Fixture>, when: &str) {
    let (strip, view_box) = strip(harness);
    let expected = strip.height() / harness.state().view.height() * strip.height();
    assert!((view_box.bottom() - strip.bottom()).abs() < 0.5, "{when}: {view_box:?} in {strip:?}");
    assert!((view_box.height() - expected).abs() < 0.5, "{when}: box {view_box:?}, {expected}");
}

#[test]
fn the_wheel_scrolls_past_the_end_until_the_last_line_is_at_the_top() {
    let mut harness = shown(LINES, true);
    let end = end(&harness);
    assert!(end > HEIGHT, "the file is several screens long: end at {end}");

    let to_end = end - harness.state().view.view_top() - 1.0;
    wheel(&mut harness, -to_end.max(0.0));
    wheel(&mut harness, -1.0);
    let top = harness.state().view.view_top();
    assert!((top - end).abs() < 1.0, "the wheel reaches the end of the rows: {top} vs {end}");
    assert_box_at_bottom(&harness, "at the end");

    for _ in 0..4 {
        wheel(&mut harness, -5_000.0);
    }
    let top = harness.state().view.view_top();
    let last = last_row_top(&harness);
    assert!((top - last).abs() < 1.0, "the last line stops at the top: {top} vs {last}");
    assert_box_at_bottom(&harness, "in the room past the end");
    assert_idle(&mut harness, "in the room past the end");
}

#[test]
fn dragging_the_view_box_to_the_bottom_stops_at_the_end() {
    let mut harness = shown(LINES, true);
    for _ in 0..4 {
        wheel(&mut harness, -5_000.0);
    }
    let (strip, view_box) = strip(&harness);
    let x = strip.center().x;
    harness.event(Event::PointerMoved(pos2(x, view_box.center().y)));
    harness.drag_at(pos2(x, view_box.center().y));
    harness.step();
    harness.event(Event::PointerMoved(pos2(x, strip.bottom() - 1.0)));
    harness.step();
    harness.drop_at(pos2(x, strip.bottom() - 1.0));
    harness.run_steps(4);
    let (top, end) = (harness.state().view.view_top(), end(&harness));
    assert!((top - end).abs() < 1.0, "the drag goes to the end of the rows: {top} vs {end}");
    assert_box_at_bottom(&harness, "dragged to the bottom");
}

#[test]
fn a_file_shorter_than_the_view_does_not_scroll() {
    let mut harness = shown(12, true);
    assert!(harness.state().view.height() < HEIGHT / 2.0, "the file fits the view");
    wheel(&mut harness, -5_000.0);
    assert!(harness.state().view.view_top().abs() < 0.01, "nothing to scroll");
    assert_eq!(last_view_box(&harness.ctx), None, "no view box on rows that do not scroll");
}

#[test]
fn off_the_view_stops_where_the_last_line_meets_the_bottom() {
    let mut harness = shown(LINES, false);
    for _ in 0..4 {
        wheel(&mut harness, -5_000.0);
    }
    let (top, end) = (harness.state().view.view_top(), end(&harness));
    assert!((top - end).abs() < 1.0, "off: the view stops at the end: {top} vs {end}");
    assert_box_at_bottom(&harness, "off, at the end");
}

#[test]
fn the_strips_menu_switches_scrolling_past_the_end() {
    let mut harness = shown(LINES, false);
    let strip = pos2(WIDTH - OVERVIEW_WIDTH / 2.0, 300.0);
    harness.event(Event::PointerMoved(strip));
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos: strip,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
    settle(&mut harness);
    assert!(harness.state().view.view_top().abs() < 0.01, "the right click leaves the view");
    harness.get_by_label(PAST_END_LABEL).click();
    settle(&mut harness);
    assert_eq!(past_end::pending_switch(&harness.ctx), Some(true), "switched on, to be saved");
    assert!(past_end::is_on(&harness.ctx), "the diff follows at once");
    for _ in 0..4 {
        wheel(&mut harness, -5_000.0);
    }
    let (top, last) = (harness.state().view.view_top(), last_row_top(&harness));
    assert!((top - last).abs() < 1.0, "the room is there: {top} vs {last}");
}

/// `lines` lines in the base; in the head every seventh changed and the last `cut` gone, in hunks
/// with three lines of context, shown New only.
fn new_only_fixture(lines: u32, cut: u32) -> Fixture {
    let mut old = String::new();
    let mut new = String::new();
    for line in 1..=lines {
        writeln!(old, "    let value_{line} = {line};").expect("a string takes text");
        let changed = if line % 7 == 0 { line * 10 } else { line };
        if line <= lines - cut {
            writeln!(new, "    let value_{line} = {changed};").expect("a string takes text");
        }
    }
    let unified =
        similar::TextDiff::from_lines(&old, &new).unified_diff().context_radius(3).to_string();
    let mut fixture = fixture(Layout::NewOnly);
    fixture.diff = Arc::new(FileDiff::parse_unified(unified.as_bytes()));
    fixture.contents.old = Some(side(&old));
    fixture.contents.new = Some(side(&new));
    fixture
}

/// An added file of `lines` lines: all of them new, shown whole.
fn added_fixture(lines: u32) -> Fixture {
    let mut new = String::new();
    for line in 1..=lines {
        writeln!(new, "    let value_{line} = {line};").expect("a string takes text");
    }
    let unified = similar::TextDiff::from_lines("", &new).unified_diff().to_string();
    let mut fixture = fixture(Layout::WholeFile(DiffSide::New));
    fixture.diff = Arc::new(FileDiff::parse_unified(unified.as_bytes()));
    fixture.contents.old = None;
    fixture.contents.new = Some(side(&new));
    fixture
}

/// Owner report: "scroll past the end" did not work in New only. Its rows end in removals of no
/// height, an added file has one pane: the wheel takes either on until the last head line is at
/// the top, and a New only file shorter than the view still does not scroll.
#[test]
fn new_only_and_an_added_file_scroll_past_the_end() {
    let cases = [
        ("new only", new_only_fixture(LINES, 5), LINES - 5),
        ("added file", added_fixture(LINES), LINES),
    ];
    for (name, fixture, last_line) in cases {
        let mut harness = harness(fixture);
        past_end::set_for_test(&harness.ctx, true);
        settle(&mut harness);
        assert!(end(&harness) > HEIGHT, "{name}: several screens long");
        for _ in 0..4 {
            wheel(&mut harness, -5_000.0);
        }
        let state = harness.state();
        let last = state.view.row_top(state.view.row_of(&state.diff, None, Some(last_line)));
        let top = state.view.view_top();
        assert!(
            (top - last).abs() < 1.0,
            "{name}: the last line stops at the top: {top} vs {last}"
        );
        assert_box_at_bottom(&harness, name);
    }

    let mut harness = harness(new_only_fixture(14, 2));
    past_end::set_for_test(&harness.ctx, true);
    settle(&mut harness);
    assert!(harness.state().view.height() < HEIGHT / 2.0, "the file fits the view");
    wheel(&mut harness, -5_000.0);
    assert!(harness.state().view.view_top().abs() < 0.01, "new only: nothing to scroll");
}
