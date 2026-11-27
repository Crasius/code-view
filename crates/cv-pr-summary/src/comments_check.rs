//! Commenting in the folder summary's cards as in the file view: a comment from a card's "+" in
//! both drag directions opens in its final place from its first frame, a thread shows at its line
//! and takes a reply and a resolve, and a draft survives its card scrolling out of view and back.

use cv_testkit::Clicks;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::ThreadAnchor;
use review_git::{ChangeKind, ChangedFile};
use review_store::ThemePreference;

use super::bodies::{CardBody, MAX_BODIES};
use super::shots::{Scene, card_of, file, harness, settle};
use super::{MARGIN, SummaryEvent};
use cv_code::line_source::Pane;
use cv_pr_diff::diff_view::DiffView;
use cv_pr_diff::testing::thread_on_old_line;
use cv_pr_threads::ThreadEvent;

/// The card commented on, between others so the view can scroll both ways around it. Its first
/// hunk shows head lines 1–3 unchanged, 4–8 added and 9–11 unchanged.
const PATH: &str = "src/app/cart.rs";
const SIZE: [f32; 2] = [1180.0, 860.0];

fn files(others: usize) -> Vec<ChangedFile> {
    let other = |index: usize| {
        file(&format!("src/app/more_{index:03}.rs"), ChangeKind::Modified, Some((10, 4)))
    };
    let mut files: Vec<ChangedFile> = (0..others.min(4)).map(other).collect();
    files.push(file(PATH, ChangeKind::Modified, Some((14, 6))));
    files.extend((others.min(4)..others).map(other));
    files
}

fn press(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn card_view(scene: &Scene) -> &DiffView {
    &scene.summary.bodies.get(PATH).expect("the card's diff is loaded").view
}

/// The card's row showing head line `new_line`.
fn row_of(scene: &Scene, new_line: u32) -> usize {
    let body = scene.summary.bodies.get(PATH).expect("the card's diff is loaded");
    body.view.row_of(&body.diff, None, Some(new_line))
}

fn line_rect(scene: &Scene, new_line: u32) -> Rect {
    card_view(scene).text_area(Pane::Inline, row_of(scene, new_line)).rect
}

/// Scrolls the summary by `delta` points and lets it settle.
fn scroll_by(harness: &mut Harness<'_, Scene>, delta: f32) {
    let summary = &mut harness.state_mut().summary;
    summary.scroll_to = Some(summary.viewport.top() + MARGIN + delta);
    settle(harness);
}

/// The summary with the card's lines on screen: `place` puts head line `new_line` at the top
/// edge, the middle or the bottom edge of the view.
fn summary_at(new_line: u32, place: Place) -> Harness<'static, Scene> {
    let mut harness = harness(Scene::new(files(8), ThemePreference::Light), SIZE);
    settle(&mut harness);
    let card = card_of(harness.state(), PATH);
    let top = harness.state().summary.index.top(card);
    harness.state_mut().summary.scroll_to = Some(top + MARGIN);
    settle(&mut harness);
    let (line, view) = (line_rect(harness.state(), new_line), harness.state().summary.screen);
    let delta = match place {
        Place::Top => line.top() - view.top() - 4.0,
        Place::Middle => line.center().y - view.center().y,
        Place::Bottom => line.bottom() - view.bottom() + 4.0,
    };
    scroll_by(&mut harness, delta);
    harness
}

#[derive(Clone, Copy)]
enum Place {
    Top,
    Middle,
    Bottom,
}

/// Dragging a card's "+" from head line 4 to 6 (or 6 to 4) and letting go opens the comment card
/// once, in its final place, as in the file view: below the last line dragging down, above the
/// first dragging up, inside the view, the lines marked in every frame. In the middle of the view
/// the line at the pointer keeps its place; at the edge the view scrolls just enough. Posting it
/// takes the file view's path.
#[test]
fn a_comment_from_a_card_opens_in_place_in_both_directions() {
    for is_up in [false, true] {
        for at_edge in [false, true] {
            let case = format!(
                "{}-{}",
                if is_up { "up" } else { "down" },
                if at_edge { "edge" } else { "middle" }
            );
            let place = match (at_edge, is_up) {
                (false, _) => Place::Middle,
                (true, true) => Place::Top,
                (true, false) => Place::Bottom,
            };
            let mut harness = summary_at(if is_up { 4 } else { 6 }, place);
            let to = drag_plus_over_lines_4_to_6(&mut harness, is_up);
            let shown = assert_card_kept_its_place(&harness, &case);
            assert_card_placed_by_the_drag(&harness, &case, shown, is_up, at_edge, to);
            assert_posting_gives_a_new_thread(&mut harness, &case);
        }
    }
}

/// Drags the card's "+" over head lines 4 to 6, `is_up` from 6 to 4, records the frames from
/// letting go on and returns where it let go.
fn drag_plus_over_lines_4_to_6(harness: &mut Harness<'_, Scene>, is_up: bool) -> Pos2 {
    let marker = |harness: &Harness<'_, Scene>, line: u32| {
        card_view(harness.state()).marker_point(Pane::Inline, row_of(harness.state(), line))
    };
    let (from, to) = if is_up {
        (marker(harness, 6), marker(harness, 4))
    } else {
        (marker(harness, 4), marker(harness, 6))
    };
    harness.hover_at(from);
    harness.step();
    harness.event(press(from, true));
    harness.step();
    harness.hover_at(from.lerp(to, 0.5));
    harness.step();
    harness.hover_at(to);
    harness.step();
    harness.state_mut().recording = Some(PATH.to_owned());
    harness.event(press(to, false));
    harness.run_steps(12);
    to
}

/// The comment card showed in one place from its first frame on, inside the view, with the
/// picked lines marked in every frame. Returns where it showed.
fn assert_card_kept_its_place(harness: &Harness<'_, Scene>, case: &str) -> Rect {
    let frames = &harness.state().frames;
    let first_card = frames
        .iter()
        .position(|(card, _)| card.is_some())
        .unwrap_or_else(|| panic!("{case}: the card never showed: {frames:?}"));
    let shown = frames[first_card].0.expect("a card");
    for (index, (card, _)) in frames.iter().enumerate().skip(first_card) {
        let card =
            card.unwrap_or_else(|| panic!("{case}: frame {index} lost the card: {frames:?}"));
        assert!(
            (card.min - shown.min).length() < 0.5 && (card.size() - shown.size()).length() < 0.5,
            "{case}: the card moved from {shown:?} (frame {first_card}) to {card:?} (frame {index})"
        );
    }
    assert!(
        frames.iter().all(|(_, marked)| *marked),
        "{case}: the picked lines blink unmarked in a frame: {frames:?}"
    );
    let view = harness.state().summary.screen;
    assert!(
        shown.top() >= view.top() - 0.5 && shown.bottom() <= view.bottom() + 0.5,
        "{case}: the card {shown:?} is outside the view {view:?}"
    );
    shown
}

/// Dragged down the card is below the last line, dragged up above the first; in the middle of
/// the view the line at the pointer (`to`) stayed, at the edge the view scrolled just enough.
fn assert_card_placed_by_the_drag(
    harness: &Harness<'_, Scene>,
    case: &str,
    shown: Rect,
    is_up: bool,
    at_edge: bool,
    to: Pos2,
) {
    let view = harness.state().summary.screen;
    let (first_line, last_line) = (line_rect(harness.state(), 4), line_rect(harness.state(), 6));
    let focus_after = if is_up {
        assert!(
            shown.bottom() <= first_line.top() + 0.5,
            "{case}: dragged up, the card ends at {}, below the first line at {}",
            shown.bottom(),
            first_line.top()
        );
        first_line.center().y
    } else {
        assert!(
            shown.top() >= last_line.bottom() - 0.5,
            "{case}: dragged down, the card starts at {}, above the last line's end {}",
            shown.top(),
            last_line.bottom()
        );
        last_line.center().y
    };
    if at_edge {
        let left_over =
            if is_up { shown.top() - view.top() } else { view.bottom() - shown.bottom() };
        assert!(
            left_over < 40.0,
            "{case}: the view scrolled more than the card needed ({left_over} left over)"
        );
    } else {
        assert!(
            (focus_after - to.y).abs() < 1.0,
            "{case}: the line at the pointer moved from {} to {focus_after}",
            to.y
        );
    }
}

/// Posting the comment takes the file view's path: a new thread on lines 4 to 6.
fn assert_posting_gives_a_new_thread(harness: &mut Harness<'_, Scene>, case: &str) {
    harness.event(Event::Text("Round before the tax.".to_owned()));
    harness.step();
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Comment").rect());
    settle(harness);
    let created = harness.state().events.iter().find_map(|event| match event {
        SummaryEvent::Create { thread, .. } => Some(thread.clone()),
        _ => None,
    });
    let thread =
        created.unwrap_or_else(|| panic!("{case}: posting gives the file view's new thread"));
    let anchor = thread.anchor.expect("on lines");
    assert_eq!(thread.body, "Round before the tax.", "{case}");
    assert_eq!((anchor.path.as_str(), anchor.new_line), (PATH, Some(6)), "{case}");
    assert_eq!(
        anchor.range.map(|range| range.start_new_line),
        Some(Some(4)),
        "{case}: on lines 4–6"
    );
}

/// A thread on a card's line shows right below it, inside the card's height, and is answered
/// and resolved there.
#[test]
fn a_thread_shows_at_its_line_in_its_card() {
    let mut scene = Scene::new(files(8), ThemePreference::Light);
    let mut thread = thread_on_old_line(0, "Please keep the old rounding here.");
    thread.id = "card-thread".into();
    thread.anchor = Some(ThreadAnchor {
        path: PATH.into(),
        old_path: None,
        old_line: None,
        new_line: Some(6),
        range: None,
    });
    scene.threads.push(thread);
    let mut harness = harness(scene, SIZE);
    settle(&mut harness);
    let card = card_of(harness.state(), PATH);
    let top = harness.state().summary.index.top(card);
    harness.state_mut().summary.scroll_to = Some(top + MARGIN);
    settle(&mut harness);

    assert!(
        harness.query_by_label_contains("Please keep the old rounding here.").is_some(),
        "the thread shows in the card"
    );
    let scene = harness.state();
    let summary = &scene.summary;
    let (geometry, slot) = (summary.geometry.expect("laid out"), summary.slots[card]);
    let height = summary.index.bottom(card) - summary.index.top(card);
    assert!(
        height > geometry.slot(slot.shape, false) + 60.0,
        "the card counts the thread's card: {height}"
    );
    let gap = line_rect(scene, 7).top() - line_rect(scene, 6).bottom();
    assert!(gap > 60.0, "the thread sits between its line and the next one ({gap} apart)");

    harness.click_at(harness.get_by_role_and_label(Role::Button, "Resolve").rect());
    harness.run_steps(2);
    let resolved = harness.state().events.iter().any(|event| matches!(event, SummaryEvent::Thread(ThreadEvent::SetStatus { thread_id, .. }) if thread_id == "card-thread"));
    assert!(resolved, "Resolve in the card goes to the thread");

    harness.click_at(harness.get_by_role_and_label(Role::Button, "Reply").rect());
    settle(&mut harness);
    harness.event(Event::Text("Done.".to_owned()));
    harness.step();
    let before = harness.state().events.len();
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Reply").rect());
    settle(&mut harness);
    let replied = harness.state().events[before..]
        .iter()
        .any(|event| matches!(event, SummaryEvent::Thread(_)));
    assert!(replied, "a reply in the card goes to the thread");
}

/// A comment being written stays when its card scrolls far out of view, even while more bodies
/// than the cache keeps come and go, and is there to finish when the card comes back.
#[test]
fn a_draft_survives_scrolling_its_card_out_of_view_and_back() {
    let mut harness = harness(Scene::new(files(300), ThemePreference::Light), SIZE);
    settle(&mut harness);
    let card = card_of(harness.state(), PATH);
    let top = harness.state().summary.index.top(card);
    harness.state_mut().summary.scroll_to = Some(top + MARGIN);
    settle(&mut harness);
    let plus = card_view(harness.state()).marker_point(Pane::Inline, row_of(harness.state(), 5));
    harness.hover_at(plus);
    harness.step();
    harness.event(press(plus, true));
    harness.step();
    harness.event(press(plus, false));
    settle(&mut harness);
    assert!(card_view(harness.state()).composer().is_some(), "the + opens a comment on line 5");
    harness.event(Event::Text("Keep this draft".to_owned()));
    settle(&mut harness);

    let end = harness.state().summary.index.total();
    harness.state_mut().summary.scroll_to = Some(end);
    settle(&mut harness);
    let summary = &harness.state().summary;
    assert!(summary.index.bottom(card) < summary.viewport.top(), "the card is out of view");
    // More bodies than the cache keeps arrive: the oldest go, the one with the draft stays.
    let earlier = harness.state().files[0].path.clone();
    let diff = harness.state().summary.bodies.get(PATH).expect("kept").diff.clone();
    let summary = &mut harness.state_mut().summary;
    let frame = summary.frame;
    for index in 0..MAX_BODIES + 20 {
        summary.bodies.insert(
            format!("filler/{index}.rs"),
            CardBody::new(diff.clone(), 3, false),
            frame,
            &|_| false,
        );
    }
    assert!(!summary.bodies.contains(&earlier), "older bodies were dropped");
    assert!(summary.bodies.contains(PATH), "the body with the comment being written stays");

    harness.state_mut().summary.scroll_to = Some(top + MARGIN);
    settle(&mut harness);
    assert!(card_view(harness.state()).composer().is_some(), "the comment is still open");
    assert!(harness.query_all_by_value("Keep this draft").next().is_some(), "with its text");
}
