//! Selecting log text with the pointer and keyboard, like a user: drag, Shift+click, double- and
//! triple-click, the line numbers, Ctrl+A in the focused log and Ctrl+C. The clipboard gets the
//! text without timestamps or ANSI escapes, as "Copy log" does. No network.

use cv_base::theme;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Role, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::gutter_width;
use super::test_harness::{PLAIN, State, harness, wait_for_copy};

const FONT_SIZE: f32 = 13.0;
const LINES: usize = 7;
/// "2026-10-18T09:00:00.1000000Z " before every line of the test log.
const TIMESTAMP_CHARS: usize = 29;

fn button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers }
}

/// The node screen readers get for the visible lines: its label and where it is.
fn lines_node(harness: &Harness<'_, State>) -> (String, Rect) {
    harness
        .query_all_by_role(Role::Document)
        .find_map(|node| {
            let label =
                node.accesskit_node().label().filter(|label| label.starts_with("Log lines"));
            label.map(|label| (label, node.rect()))
        })
        .expect("the log lines")
}

fn lines_label(harness: &Harness<'_, State>) -> String {
    lines_node(harness).0
}

/// The width of one character of the log's font.
fn char_width(harness: &Harness<'_, State>) -> f32 {
    let font = theme::fonts::code_font(FONT_SIZE);
    harness.ctx.fonts_mut(|fonts| fonts.glyph_width(&font, '0'))
}

/// Where character `column` of line `line` (counted after the timestamp) starts on screen.
fn at(harness: &Harness<'_, State>, line: usize, column: usize) -> Pos2 {
    let area = lines_node(harness).1;
    let pitch = area.height() / LINES as f32;
    let text_left = area.left() + gutter_width(FONT_SIZE, LINES);
    pos2(
        text_left + (TIMESTAMP_CHARS + column) as f32 * char_width(harness) + 1.0,
        area.top() + (line as f32 + 0.5) * pitch,
    )
}

/// The middle of line `line`'s number.
fn number_of(harness: &Harness<'_, State>, line: usize) -> Pos2 {
    let start = at(harness, line, 0);
    pos2(start.x - TIMESTAMP_CHARS as f32 * char_width(harness) - 12.0, start.y)
}

fn click(harness: &mut Harness<'_, State>, pos: Pos2, modifiers: Modifiers) {
    harness.hover_at(pos);
    harness.event(Event::ModifiersChanged(modifiers));
    harness.event(button(pos, true, modifiers));
    harness.event(button(pos, false, modifiers));
    harness.event(Event::ModifiersChanged(Modifiers::NONE));
    harness.step();
}

/// Clicks `count` times at `pos` within one frame: the harness advances further per frame than a
/// double-click may take.
fn multi_click(harness: &mut Harness<'_, State>, pos: Pos2, count: usize) {
    harness.hover_at(pos);
    harness.step();
    for _ in 0..count {
        harness.input_mut().events.push(button(pos, true, Modifiers::NONE));
        harness.input_mut().events.push(button(pos, false, Modifiers::NONE));
    }
    harness.step();
}

/// Presses at `from`, moves to `to` in a few steps and lets go there.
fn drag(harness: &mut Harness<'_, State>, from: Pos2, to: Pos2) {
    harness.hover_at(from);
    harness.step();
    harness.event(button(from, true, Modifiers::NONE));
    harness.step();
    for step in 1..=4 {
        harness.hover_at(from + (to - from) * (step as f32 / 4.0));
        harness.step();
    }
    harness.event(button(to, false, Modifiers::NONE));
    harness.step();
}

/// Ctrl+C as egui-winit reports it, and what reached the clipboard.
fn copy(harness: &mut Harness<'_, State>) -> String {
    harness.event(Event::Copy);
    wait_for_copy(harness)
}

#[test]
fn dragging_selects_text_and_ctrl_c_copies_it_plain() {
    let mut harness = harness();
    harness.run();
    assert_eq!(lines_label(&harness), "Log lines");

    let (from, to) = (at(&harness, 1, 2), at(&harness, 3, 11));
    drag(&mut harness, from, to);
    assert_eq!(lines_label(&harness), "Log lines, 3 lines selected");
    assert_eq!(
        copy(&mut harness),
        "cargo test --workspace\n   Compiling review-app v0.1.0\ntest result",
        "no timestamps, no ANSI escapes, nothing else"
    );

    // A triple-click takes the line, Shift+click on a line number extends by whole lines.
    let line = at(&harness, 4, 3);
    multi_click(&mut harness, line, 3);
    let number = number_of(&harness, 5);
    click(&mut harness, number, Modifiers::SHIFT);
    assert_eq!(lines_label(&harness), "Log lines, 2 lines selected");
    assert_eq!(
        copy(&mut harness),
        "##[warning]2 tests ignored\n##[error]error[E0308]: mismatched types\n"
    );
}

#[test]
fn shift_click_and_double_click_pick_text() {
    let mut harness = harness();
    harness.run();
    let start = at(&harness, 2, 3);
    click(&mut harness, start, Modifiers::NONE);
    let end = at(&harness, 4, 5);
    click(&mut harness, end, Modifiers::SHIFT);
    assert_eq!(
        copy(&mut harness),
        "Compiling review-app v0.1.0\ntest result: ok. 162 passed; 0 failed\n##[wa"
    );

    let word = at(&harness, 3, 7);
    multi_click(&mut harness, word, 2);
    assert_eq!(copy(&mut harness), "result", "a double-click takes the word");

    let number = number_of(&harness, 6);
    click(&mut harness, number, Modifiers::NONE);
    assert_eq!(copy(&mut harness), "##[section]Finishing: Build\n", "a line number takes its line");

    harness.key_press(Key::Escape);
    harness.step();
    assert_eq!(lines_label(&harness), "Log lines", "Esc lets go of the selection");
}

#[test]
fn ctrl_a_selects_the_whole_log_once_it_has_focus() {
    let mut harness = harness();
    harness.run();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.step();
    assert_eq!(
        lines_label(&harness),
        "Log lines",
        "Ctrl+A belongs to the log only while it has focus"
    );

    let inside = at(&harness, 0, 0);
    click(&mut harness, inside, Modifiers::NONE);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.step();
    assert_eq!(lines_label(&harness), "Log lines, 7 lines selected");
    // A bare Ctrl+C key press (no copy event) copies too.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::C);
    assert_eq!(wait_for_copy(&mut harness), PLAIN, "the whole log, as Copy log copies it");
}
