//! Shows the diff view with a real Rust file and its syntax colors and drives it through the
//! accessibility tree like a user.
//!
//! The shared fixture lives in [`crate::testing`].

use cv_code::diff_rows::Layout;
use cv_code::line_source::Pane;
use cv_pr_threads::samples;
use cv_testkit::Clicks;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{DiffSide, ReviewThread, ThreadStatus};

use crate::diff_view::DiffEvent;
use crate::testing::{
    Fixture, NEW, OLD, PATH, fixture, fixture_of, harness, harness_sized, line_of, settle,
    thread_on_old_line, whole_file_fixture,
};

/// A spot in the code area away from every wedge and popover.
const AWAY: egui::Pos2 = egui::pos2(1000.0, 740.0);

fn wedge_labels(harness: &Harness<'_, Fixture>) -> Vec<String> {
    harness
        .query_all_by_role(Role::Button)
        .filter_map(|node| node.accesskit_node().label())
        .filter(|label| label.contains("removed line"))
        .collect()
}

#[test]
fn new_only_marks_removals_and_shows_them_on_demand() {
    let mut fixture = fixture(Layout::NewOnly);
    let logged = line_of(OLD, "println!");
    fixture.threads.push(thread_on_old_line(logged, "Do we still want this trace somewhere?"));
    let mut harness = harness(fixture);
    settle(&mut harness);

    let add_start = line_of(OLD, "pub fn add(");
    let loop_start = line_of(OLD, "let mut total = 0;");
    let comment_line = line_of(OLD, "// Codes are case-insensitive.");
    assert_eq!(
        wedge_labels(&harness),
        [
            // Replacements (the new `add` signature, the iterator in `total`) get a wedge for their old lines too.
            format!("Show 3 removed lines (base lines {add_start}–{})", add_start + 2),
            format!("Show 5 removed lines (base lines {loop_start}–{})", loop_start + 4),
            format!("Show 1 removed line (base line {comment_line})"),
        ],
        "one wedge per run of removed lines"
    );
    assert!(
        harness.query_by_label_contains("Do we still want this trace").is_some(),
        "a comment on a removed line stays visible"
    );

    // Resting on a wedge opens the popover with the removed lines.
    let loop_label = format!("Show 5 removed lines (base lines {loop_start}–{})", loop_start + 4);
    harness.get_by_label(&loop_label).hover();
    settle(&mut harness);
    let popover = harness.get_by_label_contains("Removed lines:");
    let text = popover.accesskit_node().label().unwrap_or_default();
    assert!(
        text.contains("let mut total = 0;") && text.contains("for item in &self.items {"),
        "popover shows the removed code: {text}"
    );
    assert!(harness.query_by_label("5 lines removed").is_some());

    // Moving away closes a hover popover.
    harness.hover_at(AWAY);
    settle(&mut harness);
    assert!(
        harness.query_by_label_contains("Removed lines:").is_none(),
        "hover popover closes when the pointer leaves"
    );

    check_pinned_popover(&mut harness, &loop_label, loop_start);
    check_popover_keyboard(&mut harness, &loop_label);

    // N walks the changes; removals count as changes.
    assert_eq!(
        harness.state().view.change_position().total,
        4,
        "add, total, discount comment, is_empty"
    );
    harness.key_press(egui::Key::N);
    settle(&mut harness);
    assert_eq!(harness.state().view.change_position().current, Some(0));

    // The other layouts show removed lines as rows, so they have no wedges.
    harness.state_mut().layout = Layout::Inline;
    harness.state_mut().view.relayout();
    settle(&mut harness);
    assert!(
        wedge_labels(&harness).is_empty(),
        "other layouts show removals as rows, without wedges"
    );
}

/// A click pins the popover of the wedge `loop_label` (base lines from `loop_start`): it stays
/// open without the pointer, Copy puts the plain removed text on the clipboard, Esc closes it.
fn check_pinned_popover(harness: &mut Harness<'_, Fixture>, loop_label: &str, loop_start: u32) {
    harness.click_at(harness.get_by_label(loop_label).rect());
    settle(harness);
    harness.hover_at(AWAY);
    settle(harness);
    assert!(
        harness.query_by_label_contains("Removed lines:").is_some(),
        "a pinned popover stays open"
    );
    harness.click_at(harness.get_by_label("Copy the removed lines").rect());
    harness.step();
    let copied =
        harness.output().platform_output.commands.iter().find_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        });
    let expected: Vec<&str> = OLD.lines().skip(loop_start as usize - 1).take(5).collect();
    assert_eq!(
        copied.as_deref(),
        Some(expected.join("\n").as_str()),
        "copies the removed lines without numbers or markers"
    );
    settle(harness);
    harness.key_press(egui::Key::Escape);
    settle(harness);
    assert!(
        harness.query_by_label_contains("Removed lines:").is_none(),
        "Esc closes a pinned popover"
    );
}

/// Enter on the focused wedge `loop_label` opens its popover with focus on its first button; Esc
/// closes it and gives focus back to the wedge.
fn check_popover_keyboard(harness: &mut Harness<'_, Fixture>, loop_label: &str) {
    harness.get_by_label(loop_label).focus();
    harness.step();
    harness.key_press(egui::Key::Enter);
    settle(harness);
    assert!(
        harness.get_by_label("Copy the removed lines").accesskit_node().is_focused(),
        "focus moves into the popover"
    );
    harness.key_press(egui::Key::Escape);
    settle(harness);
    assert!(harness.query_by_label_contains("Removed lines:").is_none());
    assert!(
        harness.get_by_label(loop_label).accesskit_node().is_focused(),
        "focus returns to the wedge"
    );
}

#[test]
fn new_only_offers_another_layout_for_an_emptied_file() {
    let mut harness = harness(fixture_of(Layout::NewOnly, Some(OLD), Some("")));
    settle(&mut harness);
    assert!(harness.query_by_label("The new version is empty").is_some());
    harness.click_at(harness.get_by_label_contains("Show inline").rect());
    settle(&mut harness);
    let events = &harness.state().events;
    assert!(matches!(events[..], [DiffEvent::SwitchLayout(Layout::Inline)]), "{events:?}");
}

/// An added or a deleted file shows its one version whole: a row for each of its lines, in order.
#[test]
fn a_file_with_one_version_shows_each_of_its_lines() {
    for (side, text) in [(DiffSide::New, NEW), (DiffSide::Old, OLD)] {
        let mut harness = harness(whole_file_fixture(side));
        settle(&mut harness);
        let state = harness.state();
        assert_eq!(state.layout, Layout::WholeFile(side));
        let absent = match side {
            DiffSide::Old => &state.contents.new,
            DiffSide::New => &state.contents.old,
        };
        assert!(absent.is_none(), "{side:?}: the other version is absent");
        let row = |line: u32| match side {
            DiffSide::Old => state.view.row_of(&state.diff, Some(line), None),
            DiffSide::New => state.view.row_of(&state.diff, None, Some(line)),
        };
        let last = text.lines().count() as u32;
        assert_eq!(row(last) - row(1), last as usize - 1, "{side:?}: one row per line");
    }
}

/// The file's contents and a thread arriving after the user stepped to a change rebuild the
/// rows; the change stays in focus, so the counter keeps naming it ("2 of 4", not "4 changes").
/// Under load the contents came late and focus mode's counter went missing.
#[test]
fn the_change_in_focus_survives_rows_rebuilt_for_late_contents_and_threads() {
    let mut late = fixture(Layout::SideBySide);
    late.has_contents = false;
    let mut harness = harness(late);
    settle(&mut harness);
    for _ in 0..2 {
        harness.key_press(egui::Key::N);
        settle(&mut harness);
    }
    let position = harness.state().view.change_position();
    assert_eq!(position.current, Some(1), "N twice: {position:?}");

    harness.state_mut().has_contents = true;
    settle(&mut harness);
    assert_eq!(harness.state().view.change_position(), position, "the contents arrived");
    let line = line_of(OLD, "println!");
    harness.state_mut().threads.push(thread_on_old_line(line, "Keep the trace?"));
    settle(&mut harness);
    assert_eq!(harness.state().view.change_position(), position, "a thread arrived");
    harness.key_press(egui::Key::N);
    settle(&mut harness);
    let current = harness.state().view.change_position().current;
    assert_eq!(current, Some(2), "N goes on from the change in focus");

    // A layout switch builds other changes: the count starts over.
    harness.state_mut().layout = Layout::Inline;
    harness.state_mut().view.relayout();
    settle(&mut harness);
    assert_eq!(harness.state().view.change_position().current, None);
}

/// A new code size keeps the line under the pointer under it, as a browser zooms around the
/// pointer: in the frame the size changes, and back again.
#[test]
fn a_new_code_size_keeps_the_line_under_the_pointer() {
    let mut harness = harness(every_fourth_changed());
    settle(&mut harness);
    harness.hover_at(egui::pos2(600.0, 420.0));
    wheel_down(&mut harness, 4);
    // A changed line in the middle of the view, the pointer on it.
    let under = (5..=400u32)
        .step_by(4)
        .find(|line| text_rect(harness.state(), *line).top() > 380.0)
        .expect("a changed line below the middle");
    assert!(under > 60, "the view scrolled down first: line {under} in the middle");
    let pointer = text_rect(harness.state(), under).center();
    harness.hover_at(pointer);
    harness.run_steps(2);

    for size in [18.0, 13.0] {
        harness.state_mut().font_size = size;
        harness.step();
        let moved = text_rect(harness.state(), under);
        assert!(
            moved.y_range().contains(pointer.y),
            "at {size} px line {under} stays under the pointer in the same frame: {moved:?}, pointer {pointer:?}"
        );
        settle(&mut harness);
        let settled = text_rect(harness.state(), under);
        assert!(settled.y_range().contains(pointer.y), "and stays there at {size} px: {settled:?}");
    }
}

/// 400 lines, every fourth one changed, in one pane.
fn every_fourth_changed() -> Fixture {
    let line = |index: usize, changed: bool| {
        format!(
            "    let value_{index} = compute({index}, {});\n",
            if changed { index * 7 } else { index }
        )
    };
    let old: String = (0..400).map(|index| line(index, false)).collect();
    let new: String = (0..400).map(|index| line(index, index % 4 == 0)).collect();
    fixture_of(Layout::Inline, Some(&old), Some(&new))
}

/// Turns the mouse wheel `notches` lines down where the pointer is, then lets the scroll ease out.
fn wheel_down(harness: &mut Harness<'_, Fixture>, notches: usize) {
    for _ in 0..notches {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -12.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Wheel,
        });
        harness.step();
    }
    harness.run_steps(30);
}

/// Where the text of `new_line` shows in the one pane.
fn text_rect(fixture: &Fixture, new_line: u32) -> egui::Rect {
    let row = fixture.view.row_of(&fixture.diff, None, Some(new_line));
    fixture.view.text_area(Pane::Inline, row).rect
}

fn thread_on_new_line(id: &str, new_line: u32, body: &str) -> ReviewThread {
    samples::thread(
        id,
        ThreadStatus::Active,
        Some(samples::lines(PATH, new_line, new_line)),
        vec![samples::comment(&format!("{id}-c"), &samples::mara(), 5, body)],
    )
}

const NEW_LINE_BODY: &str = "Should this live on the trait instead?";
const OLD_LINE_BODY: &str = "Do we still want this trace somewhere?";

fn shows(harness: &Harness<'_, Fixture>, body: &str) -> bool {
    harness.query_by_label_contains(body).is_some()
}

fn counts<'h>(harness: &'h Harness<'_, Fixture>, label: &'h str) -> Vec<egui_kittest::Node<'h>> {
    harness.query_all_by_role_and_label(Role::Button, label).collect()
}

/// While comments are hidden in the diff, a count in the gutter stands for a line's threads: a
/// click shows them there and hides them again. Switching comments on shows every card, and off
/// again hides the ones shown one by one.
#[test]
fn hidden_comments_show_one_by_one_from_their_count() {
    let mut fixture = fixture(Layout::SideBySide);
    // Next to each other, so both cards fit the view.
    fixture.threads.push(thread_on_new_line(
        "new-line-thread",
        line_of(NEW, "pub fn add("),
        NEW_LINE_BODY,
    ));
    fixture.threads.push(thread_on_old_line(line_of(OLD, "println!"), OLD_LINE_BODY));
    fixture.show_threads = false;
    let mut harness = harness(fixture);
    settle(&mut harness);
    assert!(
        !shows(&harness, NEW_LINE_BODY) && !shows(&harness, OLD_LINE_BODY),
        "both cards are hidden"
    );
    let hidden = counts(&harness, "1 comment hidden here, click to show");
    assert_eq!(hidden.len(), 2, "a count on each commented line");
    let head_count = hidden
        .into_iter()
        .max_by(|a, b| a.rect().left().total_cmp(&b.rect().left()))
        .expect("the head pane's count")
        .rect();
    harness.click_at(head_count);
    settle(&mut harness);
    assert!(shows(&harness, NEW_LINE_BODY), "the clicked line's card shows");
    assert!(!shows(&harness, OLD_LINE_BODY), "the other stays hidden");
    let shown_count = counts(&harness, "1 comment shown here, click to hide")
        .first()
        .expect("the shown count")
        .rect();
    harness.click_at(shown_count);
    settle(&mut harness);
    assert!(!shows(&harness, NEW_LINE_BODY), "a second click hides it again");

    harness.state_mut().show_threads = true;
    settle(&mut harness);
    assert!(
        shows(&harness, NEW_LINE_BODY) && shows(&harness, OLD_LINE_BODY),
        "switched on, every card shows"
    );
    assert!(
        harness.query_by_label_contains("comment hidden here").is_none(),
        "and the avatars mark the lines again"
    );
    harness.state_mut().show_threads = false;
    settle(&mut harness);
    assert!(
        !shows(&harness, NEW_LINE_BODY) && !shows(&harness, OLD_LINE_BODY),
        "switched off again, all are hidden"
    );
}

/// A jump to a thread (the files tab, the thread list) shows it while comments are hidden.
#[test]
fn jumping_to_a_hidden_comment_shows_it() {
    let mut fixture = fixture(Layout::SideBySide);
    fixture.threads.push(thread_on_new_line(
        "new-line-thread",
        line_of(NEW, "pub fn is_empty(&self)"),
        NEW_LINE_BODY,
    ));
    fixture.threads.push(thread_on_old_line(line_of(OLD, "println!"), OLD_LINE_BODY));
    fixture.show_threads = false;
    let mut harness = harness(fixture);
    settle(&mut harness);
    harness.state_mut().view.reveal_thread("new-line-thread");
    settle(&mut harness);
    assert!(shows(&harness, NEW_LINE_BODY), "the thread jumped to shows");
    assert!(!shows(&harness, OLD_LINE_BODY), "the others stay hidden");
}

/// Hiding and showing the comments keeps the code in view where it is, with cards above it.
#[test]
fn switching_comments_keeps_the_view_in_place() {
    // Threads on the first two changes.
    let mut fixture = every_fourth_changed();
    fixture.threads.push(thread_on_new_line("first", 5, "A comment on the first change."));
    fixture.threads.push(thread_on_new_line("second", 9, "A comment on the second change."));
    let mut harness = harness_sized(fixture, [1180.0, 520.0]);
    settle(&mut harness);
    harness.hover_at(egui::pos2(600.0, 300.0));
    wheel_down(&mut harness, 3);
    assert!(!shows(&harness, "A comment on the first change."), "the cards are above the view");
    let watched = (5..=400u32)
        .step_by(4)
        .find(|line| text_rect(harness.state(), *line).top() > 200.0)
        .expect("a line in the view");
    let before = text_rect(harness.state(), watched);
    for show in [false, true] {
        harness.state_mut().show_threads = show;
        settle(&mut harness);
        let after = text_rect(harness.state(), watched);
        assert!(
            (after.top() - before.top()).abs() < 0.5,
            "comments {}: line {watched} moved from {before:?} to {after:?}",
            if show { "shown" } else { "hidden" }
        );
    }
}
