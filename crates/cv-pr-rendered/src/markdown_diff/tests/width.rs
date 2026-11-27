//! The rendered view's width: a centered reading column by default that its edges resize on both
//! sides, Full width that fills the pane, and tables and code that never clip in either.

use cv_testkit::poll_until;
use egui::{Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use review_core::DiffSide;
use review_git::FULL_FILE_CONTEXT;
use review_store::{DiffLayout, RenderedWidth};

use super::super::{GUTTER, RIGHT_PADDING, RenderedEvent};
use super::{Scene, harness, press, wheel};

const PROSE: &str = "The reading column keeps lines to a length the eye can follow, and this paragraph runs long \
enough to wrap in it many times over, so its laid-out text shows how wide the column is: every row but the last one \
reaches the right edge of the column, give or take a word, and the first row starts at its left edge. A pane wider \
than the column leaves the same room on both sides of it, and dragging either edge moves both of them. Full width \
fills the pane instead, minus the gutter on the left and the padding on the right.";

const LONG_CELL: &str = "holds the legal-form suffix of Sec. 5a (1) GmbHG. In a German invoice the supplier's legal \
name must carry it in full, so the export never shortens it and the check rejects an invoice whose name lacks it";

/// An added Markdown file: shown whole, in one column.
fn added(text: &str, width: RenderedWidth) -> Scene {
    let mut scene = Scene::new("", text, DiffLayout::SideBySide, FULL_FILE_CONTEXT);
    scene.doc.old = None;
    scene.width = width;
    scene
}

fn full() -> RenderedWidth {
    RenderedWidth { full: true, ..RenderedWidth::default() }
}

/// The lane of the first block of `side` on screen, as painted.
fn lane(harness: &Harness<'_, Scene>, side: DiffSide) -> Rect {
    harness
        .state()
        .view
        .probe
        .cells
        .iter()
        .find(|cell| cell.side == side && cell.unit.is_some())
        .expect("a block on screen")
        .rect
}

fn label(harness: &Harness<'_, Scene>, text: &str) -> Rect {
    harness.get_by_label(text).rect()
}

/// The paragraph `PROSE` ending in "Before." and in "After.", for both sides.
fn before_after() -> (String, String) {
    (format!("{PROSE} Before.\n"), format!("{PROSE} After.\n"))
}

/// Presses at `from`, moves to `to` in a few steps and lets go there.
fn drag(harness: &mut Harness<'_, Scene>, from: Pos2, to: Pos2) {
    harness.hover_at(from);
    harness.step();
    harness.input_mut().events.push(press(from, true));
    harness.step();
    for step in 1..=4 {
        harness.hover_at(from + (to - from) * (step as f32 / 4.0));
        harness.step();
    }
    harness.input_mut().events.push(press(to, false));
    harness.run();
}

fn double_click(harness: &mut Harness<'_, Scene>, pos: Pos2) {
    harness.hover_at(pos);
    harness.step();
    for _ in 0..2 {
        harness.input_mut().events.push(press(pos, true));
        harness.input_mut().events.push(press(pos, false));
    }
    harness.run();
}

#[test]
fn the_reading_column_is_centered_by_default() {
    let mut harness = harness(added(PROSE, RenderedWidth::default()), [1650.0, 800.0]);
    harness.run();
    let lane = lane(&harness, DiffSide::New);
    let text = label(&harness, PROSE);
    let (left, width) = (text.left(), text.width());
    let column = RenderedWidth::DEFAULT_READING;
    assert!(
        (left - (lane.center().x - column / 2.0)).abs() < 1.0,
        "the column is centered in {lane:?}: starts at {left}"
    );
    assert!(
        width <= column + 0.5 && width > column - 60.0,
        "the text wraps at the column's width: {width}"
    );
    assert!(!harness.state().view.fills_width(), "the switch shows the reading column");
}

#[test]
fn full_width_fills_the_pane() {
    let mut harness = harness(added(PROSE, full()), [1650.0, 800.0]);
    harness.run();
    let lane = lane(&harness, DiffSide::New);
    let text = label(&harness, PROSE);
    let (left, width) = (text.left(), text.width());
    assert!(
        (left - (lane.left() + GUTTER)).abs() < 1.0,
        "the text starts after the gutter: {left} in {lane:?}"
    );
    let room = lane.width() - GUTTER - RIGHT_PADDING;
    assert!(
        width > room - 80.0 && width <= room + 0.5,
        "the text takes the pane's width: {width} of {room}"
    );
    assert!(harness.state().view.fills_width());
}

/// Side by side each half takes its whole width, whether the setting is the reading column or
/// Full width.
#[test]
fn side_by_side_halves_each_fill_their_width() {
    let (old, new) = before_after();
    for (width, what) in [(full(), "full width"), (RenderedWidth::default(), "reading")] {
        let mut scene = Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT);
        scene.width = width;
        let mut harness = harness(scene, [1650.0, 800.0]);
        harness.run();
        for (side, text) in [(DiffSide::Old, old.trim_end()), (DiffSide::New, new.trim_end())] {
            let lane = lane(&harness, side);
            let shown = label(&harness, text);
            let (left, text_width) = (shown.left(), shown.width());
            let room = lane.width() - GUTTER - RIGHT_PADDING;
            assert!(
                (left - (lane.left() + GUTTER)).abs() < 1.0,
                "{what}: {side:?} starts after the gutter: {left} in {lane:?}"
            );
            assert!(
                text_width > room - 80.0,
                "{what}: {side:?} fills its half: {text_width} of {room}"
            );
        }
        assert!(harness.state().view.fills_width(), "{what}: the switch shows Full width");
    }
}

#[test]
fn dragging_an_edge_widens_both_sides_and_is_saved() {
    let mut harness = harness(added(PROSE, RenderedWidth::default()), [1650.0, 800.0]);
    harness.run();
    let lane = lane(&harness, DiffSide::New);
    let column = RenderedWidth::DEFAULT_READING;
    let edge = pos2(lane.center().x + column / 2.0 + GUTTER, 300.0);
    drag(&mut harness, edge, edge + vec2(100.0, 0.0));
    let reading = harness.state().width.reading;
    assert!((reading - (column + 200.0)).abs() <= 4.0, "both sides moved out by 100: {reading}");
    let left = label(&harness, PROSE).left();
    assert!(
        (left - (lane.center().x - reading / 2.0)).abs() < 1.0,
        "still centered: starts at {left}"
    );
    assert!(
        harness.state().events.contains(&RenderedEvent::WidthResized),
        "the new width is saved: {:?}",
        harness.state().events
    );

    // The left edge narrows it the same way.
    let edge = pos2(lane.center().x - reading / 2.0 - GUTTER, 300.0);
    drag(&mut harness, edge, edge + vec2(150.0, 0.0));
    let narrower = harness.state().width.reading;
    assert!((narrower - (reading - 300.0)).abs() <= 4.0, "both sides moved in by 150: {narrower}");
}

#[test]
fn double_clicking_an_edge_resets_the_width() {
    let width = RenderedWidth { reading: 400.0, ..RenderedWidth::default() };
    let mut harness = harness(added(PROSE, width), [1650.0, 800.0]);
    harness.run();
    let lane = lane(&harness, DiffSide::New);
    double_click(&mut harness, pos2(lane.center().x + 200.0 + GUTTER, 300.0));
    assert_eq!(harness.state().width, RenderedWidth::default());
    assert!(harness.state().events.contains(&RenderedEvent::WidthResized), "the reset is saved");
}

#[test]
fn a_column_dragged_to_the_pane_edge_switches_to_full_width_and_back() {
    let mut harness = harness(added(PROSE, RenderedWidth::default()), [1650.0, 800.0]);
    harness.run();
    let lane = lane(&harness, DiffSide::New);
    let edge = pos2(lane.center().x + RenderedWidth::DEFAULT_READING / 2.0 + GUTTER, 300.0);
    drag(&mut harness, edge, pos2(lane.right() - 4.0, 300.0));
    assert!(harness.state().width.full, "near the pane's edge the column snaps to full width");
    assert!(harness.state().view.fills_width(), "the switch shows Full width");
    let left = label(&harness, PROSE).left();
    assert!(
        (left - (lane.left() + GUTTER)).abs() < 1.0,
        "the text fills the pane: starts at {left}"
    );

    // A full lane's right edge, dragged inward, makes a column again.
    let edge = pos2(lane.right() - RIGHT_PADDING / 2.0 - 4.0, 300.0);
    drag(&mut harness, edge, edge - vec2(300.0, 0.0));
    let width = harness.state().width;
    assert!(
        !width.full && !harness.state().view.fills_width(),
        "back to the reading column: {width:?}"
    );
    let expected =
        2.0 * (lane.right() - RIGHT_PADDING / 2.0 - 4.0 - 300.0 - lane.center().x - GUTTER);
    assert!(
        (width.reading - expected).abs() <= 4.0,
        "as wide as dragged: {} vs {expected}",
        width.reading
    );
}

/// Side by side has no reading column to resize: no grips, and dragging where one would be in
/// one column changes nothing.
#[test]
fn side_by_side_has_no_column_grips() {
    let (old, new) = before_after();
    let mut harness =
        harness(Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT), [1650.0, 800.0]);
    harness.run();
    assert!(harness.query_by_label("Reading width").is_none(), "no column edge to grab");
    let right = lane(&harness, DiffSide::New);
    for x in [
        right.center().x + RenderedWidth::DEFAULT_READING / 2.0 + GUTTER,
        right.right() - RIGHT_PADDING / 2.0 - 4.0,
    ] {
        drag(&mut harness, pos2(x, 300.0), pos2(x - 150.0, 300.0));
    }
    assert_eq!(harness.state().width, RenderedWidth::default(), "the width setting is untouched");
    assert!(!harness.state().events.contains(&RenderedEvent::WidthResized));
    let left = label(&harness, new.trim_end()).left();
    assert!((left - (right.left() + GUTTER)).abs() < 1.0, "the half still fills its width");
}

fn wide_table() -> String {
    format!(
        "| Field | Rule | Example |\n| --- | --- | --- |\n| `Invoice.SupplierLegalName` | {LONG_CELL} | ACME GmbH |\n\
         | `Invoice.Number` | Unique per supplier and year, at most 32 characters | 2026-0042 |\n"
    )
}

/// Every cell's text inside the room the view gives the table, the columns side by side without
/// overlapping, and the long cell wrapped.
fn check_table(harness: &Harness<'_, Scene>, room: (f32, f32), what: &str) {
    let rows = [
        ["Field", "Rule", "Example"],
        ["Invoice.SupplierLegalName", LONG_CELL, "ACME GmbH"],
        ["Invoice.Number", "Unique per supplier and year, at most 32 characters", "2026-0042"],
    ];
    for row in rows {
        let rects: Vec<Rect> = row.iter().map(|text| label(harness, text)).collect();
        for (text, rect) in row.iter().zip(&rects) {
            assert!(
                rect.left() >= room.0 - 0.5 && rect.right() <= room.1 + 0.5,
                "{what}: {text:?} at {rect:?} is outside {room:?}"
            );
        }
        for pair in rects.windows(2) {
            assert!(
                pair[0].right() <= pair[1].left(),
                "{what}: cells overlap in {row:?}: {rects:?}"
            );
        }
    }
    let (long, short) = (label(harness, LONG_CELL), label(harness, "ACME GmbH"));
    assert!(long.height() > 1.5 * short.height(), "{what}: the long cell wraps: {long:?}");
}

#[test]
fn a_wide_table_wraps_its_cells_and_nothing_is_clipped() {
    let doc = format!("{PROSE}\n\n{}", wide_table());
    // Full width in a narrow pane, and a reading column the table grows out of in a wide one.
    for (width, size, what) in [
        (full(), [900.0, 800.0], "full width"),
        (RenderedWidth::default(), [1100.0, 800.0], "reading"),
    ] {
        let mut harness = harness(added(&doc, width), size);
        harness.run();
        let lane = lane(&harness, DiffSide::New);
        let room = if width.full {
            (lane.left() + GUTTER, lane.right() - RIGHT_PADDING)
        } else {
            (lane.left() + GUTTER, lane.right() - GUTTER)
        };
        check_table(&harness, room, what);
        if !width.full {
            let text = label(&harness, PROSE);
            let (left, column) = (text.left(), text.width());
            let table = label(&harness, "Field").union(label(&harness, "ACME GmbH"));
            assert!(
                table.width() > column,
                "{what}: the table grows past the {column} column: {table:?}"
            );
            assert!(
                table.left() < left - 50.0,
                "{what}: the table grows to both sides of the column: {table:?} from {left}"
            );
        }
    }
}

#[test]
fn a_table_too_wide_for_its_columns_scrolls_inside_its_box() {
    let header = (1..=8).map(|column| format!("Column {column} heading")).collect::<Vec<_>>();
    let cells = (1..=8)
        .map(|column| format!("value {column} that is long enough to want room"))
        .collect::<Vec<_>>();
    let doc =
        format!("| {} |\n|{}\n| {} |\n", header.join(" | "), " --- |".repeat(8), cells.join(" | "));
    let mut harness = harness(added(&doc, full()), [600.0, 600.0]);
    harness.run();
    let lane = lane(&harness, DiffSide::New);
    let last = label(&harness, "Column 8 heading");
    assert!(last.left() > lane.right(), "the last column is past the view, not squeezed: {last:?}");
    assert!(
        label(&harness, "Column 1 heading").left() >= lane.left() + GUTTER - 0.5,
        "the first column shows"
    );
    let first = label(&harness, "Column 1 heading").center();
    wheel(&mut harness, first, vec2(-4000.0, 0.0));
    let last = label(&harness, "Column 8 heading");
    assert!(
        last.right() <= lane.right() - RIGHT_PADDING + 0.5,
        "scrolled to its end, the last column shows: {last:?} in {lane:?}"
    );
}

#[test]
fn a_long_code_line_scrolls_inside_its_block() {
    let line = format!(
        "let summary = [{}];",
        (1..=40).map(|value| format!("\"entry {value}\"")).collect::<Vec<_>>().join(", ")
    );
    let doc = format!("Intro.\n\n```text\n{line}\n```\n");
    let mut harness = harness(added(&doc, full()), [600.0, 600.0]);
    harness.run();
    let lane = lane(&harness, DiffSide::New);
    let code = label(&harness, &line);
    assert!(code.right() > lane.right(), "the line runs on past the view: {code:?}");
    wheel(&mut harness, pos2(lane.center().x, code.center().y), vec2(-8000.0, 0.0));
    let scrolled = label(&harness, &line);
    assert!(
        scrolled.left() < code.left() - 200.0,
        "the block scrolls sideways: {code:?} to {scrolled:?}"
    );
    assert!(
        scrolled.right() <= lane.right() - RIGHT_PADDING + 0.5,
        "its end comes into view: {scrolled:?}"
    );
}

/// Dragging the column's edge over the 5,000-line README lays out only the rows in view at each
/// new width (in steps of a few points), and the block at the top of the view stays there. How
/// fast that is, is the frame budget benchmark's question (release build).
#[test]
fn resizing_the_column_of_a_long_readme_keeps_the_place() {
    let (old, new) = super::long_readme();
    let mut harness =
        harness(Scene::new(&old, &new, DiffLayout::Inline, FULL_FILE_CONTEXT), [1280.0, 900.0]);
    poll_until("every row of the README measured", || {
        harness.step();
        harness.state().view.heights.is_all_measured()
    });
    wheel(&mut harness, pos2(640.0, 450.0), vec2(0.0, -6000.0));
    let anchor = |harness: &Harness<'_, Scene>| {
        harness.state().view.top_unit().map(|(_, side, unit, _)| (side, unit))
    };
    let before = anchor(&harness);
    assert!(before.is_some(), "scrolled into the README");
    let lane = lane(&harness, DiffSide::New);
    let mut x = lane.center().x + RenderedWidth::DEFAULT_READING / 2.0 + GUTTER;
    harness.hover_at(pos2(x, 450.0));
    harness.step();
    harness.input_mut().events.push(press(pos2(x, 450.0), true));
    harness.step();
    let mut most_cells = 0;
    for _ in 0..60 {
        x += 3.0;
        harness.hover_at(pos2(x, 450.0));
        harness.step();
        most_cells = most_cells.max(harness.state().view.probe.cells.len());
    }
    harness.input_mut().events.push(press(pos2(x, 450.0), false));
    harness.run();
    assert!(most_cells < 80, "a frame of the drag lays out the rows in view, not {most_cells}");
    assert_eq!(anchor(&harness), before, "the block at the top of the view stays there");
    let reading = harness.state().width.reading;
    assert!(
        reading > RenderedWidth::DEFAULT_READING + 300.0,
        "the drag widened the column: {reading}"
    );
}
