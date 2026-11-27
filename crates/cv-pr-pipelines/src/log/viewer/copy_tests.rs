//! "Copy log" on a log with Azure DevOps timestamps and ANSI colors, by click and by
//! Ctrl+Shift+C (⇧⌘C on macOS): the clipboard gets the plain text, the button confirms with "Copied" without
//! moving the toolbar. No network.

use cv_testkit::Clicks;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::NEXT_MATCH_HINT;
use super::test_harness::{PLAIN, State, harness, wait_for_copy};

fn rect_of(harness: &Harness<'_, State>, label: &str) -> egui::Rect {
    harness.get_by_label(label).rect()
}

#[test]
fn copy_log_puts_plain_text_on_the_clipboard() {
    let mut harness = harness();
    harness.run();
    let search = rect_of(&harness, NEXT_MATCH_HINT);

    harness.click_at(harness.get_by_label("Copy log").rect());
    assert_eq!(
        wait_for_copy(&mut harness),
        PLAIN,
        "timestamps and ANSI escapes are stripped, nothing else"
    );
    harness.step();
    assert!(harness.query_by_label("Copied").is_some(), "the button confirms");
    assert_eq!(
        rect_of(&harness, NEXT_MATCH_HINT),
        search,
        "the confirmation does not move the toolbar"
    );
}

#[test]
fn ctrl_shift_c_copies_the_log() {
    let mut harness = harness();
    harness.run();
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::C);
    assert_eq!(wait_for_copy(&mut harness), PLAIN);
}
