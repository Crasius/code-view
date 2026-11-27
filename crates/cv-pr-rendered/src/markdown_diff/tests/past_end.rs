//! "Scroll past the end" in the rendered diff: Page Down and the wheel go on until the last block
//! is at the top of the view, End goes to where the last block meets the bottom, and off the view
//! stops there whatever scrolls it. The strip's context menu switches it. New only and an added
//! file go as far.

use cv_testkit::Clicks;
use egui::{Event, Key, Modifiers, PointerButton, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use review_git::FULL_FILE_CONTEXT;
use review_store::DiffLayout;

use super::{RenderedEvent, Scene, harness, sample, wheel};
use cv_pr_diff::diff_overview::PAST_END_LABEL;
use cv_pr_diff::diff_view::OVERVIEW_WIDTH;

const SIZE: [f32; 2] = [900.0, 600.0];

/// The release notes, with "Scroll past the end" `on` or off, laid out.
fn shown(on: bool) -> Harness<'static, Scene> {
    let (old, new) = sample();
    shown_scene(Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT), on)
}

fn shown_scene(mut scene: Scene, on: bool) -> Harness<'static, Scene> {
    scene.doc.past_end = on;
    let mut harness = harness(scene, SIZE);
    harness.run();
    harness
}

/// Page Down until the view can go no further.
fn page_to_the_end(harness: &mut Harness<'_, Scene>) {
    for _ in 0..40 {
        harness.key_press(Key::PageDown);
        harness.run();
    }
}

/// (the view's top, where the rows end, the last row's top) after the last frame.
fn place(scene: &Scene) -> (f32, f32, f32) {
    let view = scene.view.viewport.0;
    let heights = &scene.view.heights;
    let last = heights.top(scene.view.rows.len() - 1);
    (view.top(), heights.total() - view.height(), last)
}

#[test]
fn page_down_reaches_the_last_block_at_the_top_and_end_goes_to_the_end() {
    let mut harness = shown(true);
    page_to_the_end(&mut harness);
    let (top, end, last) = place(harness.state());
    assert!(end > 0.0, "the notes are longer than the view");
    assert!((top - last).abs() < 1.0, "the last block stops at the top: {top} vs {last}");

    harness.key_press(Key::End);
    harness.run();
    let (top, end, _) = place(harness.state());
    assert!((top - end).abs() < 1.0, "End: the last block meets the bottom: {top} vs {end}");

    wheel(&mut harness, pos2(450.0, 300.0), vec2(0.0, -20_000.0));
    let (top, _, last) = place(harness.state());
    assert!((top - last).abs() < 1.0, "the wheel goes on to the last block: {top} vs {last}");
}

#[test]
fn off_the_view_stops_where_the_last_block_meets_the_bottom() {
    let mut harness = shown(false);
    page_to_the_end(&mut harness);
    wheel(&mut harness, pos2(450.0, 300.0), vec2(0.0, -20_000.0));
    let (top, end, _) = place(harness.state());
    assert!((top - end).abs() < 1.0, "off: the view stops at the end: {top} vs {end}");
}

#[test]
fn the_strips_menu_switches_scrolling_past_the_end() {
    let mut harness = shown(false);
    let strip = pos2(SIZE[0] - OVERVIEW_WIDTH / 2.0, 300.0);
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
    harness.run();
    harness.click_at(harness.get_by_label(PAST_END_LABEL).rect());
    harness.run();
    let events = &harness.state().events;
    assert_eq!(events[..], [RenderedEvent::SwitchPastEnd(true)], "switched on, to be saved");
    assert!(harness.state().doc.past_end, "the saved setting is on");
    page_to_the_end(&mut harness);
    let (top, _, last) = place(harness.state());
    assert!((top - last).abs() < 1.0, "the room is there: {top} vs {last}");
}

#[test]
fn new_only_and_an_added_file_reach_the_last_block_at_the_top() {
    let (old, new) = sample();
    let new_only = Scene::new(&old, &new, DiffLayout::NewOnly, FULL_FILE_CONTEXT);
    let mut added = Scene::new("", &new, DiffLayout::NewOnly, FULL_FILE_CONTEXT);
    added.doc.old = None;
    for (name, scene) in [("new only", new_only), ("added file", added)] {
        let mut harness = shown_scene(scene, true);
        page_to_the_end(&mut harness);
        let (top, end, last) = place(harness.state());
        assert!(end > 0.0, "{name}: the notes are longer than the view");
        assert!((top - last).abs() < 1.0, "{name}: Page Down to the last block: {top} vs {last}");
        harness.key_press(Key::End);
        harness.run();
        wheel(&mut harness, pos2(450.0, 300.0), vec2(0.0, -20_000.0));
        let (top, _, last) = place(harness.state());
        assert!((top - last).abs() < 1.0, "{name}: the wheel to the last block: {top} vs {last}");
    }
}
