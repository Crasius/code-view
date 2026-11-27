//! The file tree's side end to end: the real app against the mocked GitHub host. The tree starts
//! at the left edge; the switch in its header moves it to the right and back in one frame, with
//! one save, at the same width, with the open file, its scroll position and the keyboard focus
//! kept. In focus mode the strip's tree switch sits at the end of the tree's side, and its context
//! menu moves the tree, also while the tree is hidden.

use egui::{Key, Modifiers, Rect, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_store::TreeSide;

use super::ReviewApp;
use super::github_mock::{ExtraFile, GitHubHost, open_pull_request};
use super::walkthrough::{app, files_tab, open_in_tree, saved_settings, wait_for_diff};
use cv_base::platform::FOCUS_MODE_KEY;
use cv_shell::focus_mode::{self, Surface};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const LONG_FILE: &str = "src/long.rs";
const TO_RIGHT: &str = "Move the file tree to the right";
const TO_LEFT: &str = "Move the file tree to the left";

/// 600 lines, every eighth one changed.
pub(super) fn long_file() -> ExtraFile {
    let line = |index: usize, changed: bool| {
        if changed {
            format!("    let value_{index} = compute({index}) + 1; // changed\n")
        } else {
            format!("    let value_{index} = compute({index});\n")
        }
    };
    let base: String = (0..600).map(|index| line(index, false)).collect();
    let head: String = (0..600).map(|index| line(index, index % 8 == 4)).collect();
    ExtraFile { path: LONG_FILE, base: base.into_bytes(), head: head.into_bytes() }
}

fn side(harness: &Harness<'_, ReviewApp>) -> TreeSide {
    harness.state().services.settings.file_tree_side
}

fn tree_row(harness: &Harness<'_, ReviewApp>) -> Rect {
    harness.get_by_role_and_label(Role::TreeItem, LONG_FILE).rect()
}

fn diff_rect(harness: &Harness<'_, ReviewApp>) -> Rect {
    harness.get_by_label(&format!("Diff of {LONG_FILE}")).rect()
}

/// The tree's width on screen, as its full-width search field takes it.
fn tree_width_on_screen(harness: &Harness<'_, ReviewApp>) -> f32 {
    harness.get_by_role(Role::TextInput).rect().width()
}

/// The tree on `expected`'s side of the diff, the diff still showing.
pub(super) fn assert_tree_at(harness: &Harness<'_, ReviewApp>, expected: TreeSide) {
    assert_eq!(side(harness), expected);
    let (row, diff) = (tree_row(harness), diff_rect(harness));
    match expected {
        TreeSide::Left => assert!(row.right() <= diff.left() + 1.0, "{row:?} left of {diff:?}"),
        TreeSide::Right => assert!(row.left() >= diff.right() - 1.0, "{row:?} right of {diff:?}"),
    }
}

/// The folder's viewed mark sits in the viewed column, left of its glyph: one indent left of its
/// files' checkboxes, at their size.
fn assert_marks_line_up(harness: &Harness<'_, ReviewApp>) {
    let folder = harness.get_by_label("Viewed folder src").rect();
    let file = harness.get_by_label(&format!("Viewed {LONG_FILE}")).rect();
    let indent = file.left() - folder.left();
    assert!((indent - 16.0).abs() < 0.5, "one indent apart: {folder:?} {file:?}");
    assert!((folder.width() - file.width()).abs() < 0.5, "the same hit area: {folder:?}");
    let row = harness.get_by_role_and_label(Role::TreeItem, "src").rect();
    assert!(folder.right() < row.left() + 48.0, "left of the folder glyph: {folder:?} {row:?}");
}

/// Opens the pull request from the list and [`LONG_FILE`] in its Files tab.
pub(super) fn open_long_file(harness: &mut Harness<'_, ReviewApp>) {
    open_pull_request(harness);
    files_tab(harness);
    open_in_tree(harness, LONG_FILE);
    wait_for_diff(harness, LONG_FILE);
}

#[test]
fn the_file_tree_moves_sides_and_keeps_the_open_file() {
    let scratch = ScratchDir::new("tree-side");
    let host = GitHubHost::with_pull_request_files(scratch.path(), &[long_file()]);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_long_file(&mut harness);
    for _ in 0..20 {
        harness.key_press(Key::N);
        harness.step();
    }
    harness.run_steps(10);

    // The tree starts on the left, as before the setting existed.
    assert_tree_at(&harness, TreeSide::Left);
    assert_marks_line_up(&harness);
    let top = focus_mode::top_line(&harness.ctx, Surface::Files).expect("the diff's top line");
    assert!(top.0 > 50, "the diff scrolled into the file: {top:?}");
    let width = tree_width_on_screen(&harness);

    // Its header's switch moves it right, in the frame of the click, with one save. The switch
    // takes the keyboard focus and keeps it across the move.
    harness.get_by_label(TO_RIGHT).focus();
    harness.step();
    harness.key_press(Key::Enter);
    harness.step();
    assert_tree_at(&harness, TreeSide::Right);
    assert_eq!(saved_settings(&harness).file_tree_side, TreeSide::Right, "the side is saved");
    harness.run_steps(3);
    assert!(harness.get_by_label(TO_LEFT).is_focused(), "the switch keeps the focus");
    let now = tree_width_on_screen(&harness);
    assert!((now - width).abs() < 0.5, "the tree keeps its width: {now} vs {width}");
    assert_eq!(
        focus_mode::top_line(&harness.ctx, Surface::Files),
        Some(top),
        "the open file keeps its line on top"
    );
    let search = harness.get_by_role(Role::TextInput).rect();
    assert!(search.left() > diff_rect(&harness).right(), "the whole pane moved: {search:?}");

    // And back.
    harness.click_at(harness.get_by_label(TO_LEFT).rect());
    harness.step();
    assert_tree_at(&harness, TreeSide::Left);
    assert_eq!(saved_settings(&harness).file_tree_side, TreeSide::Left);
    harness.run_steps(3);
    let now = tree_width_on_screen(&harness);
    assert!((now - width).abs() < 0.5, "the tree keeps its width: {now} vs {width}");

    // Focus mode: the strip's switch sits at the tree's end and its menu moves the tree.
    harness.key_press_modifiers(FOCUS_MODE_KEY.modifiers, FOCUS_MODE_KEY.logical_key);
    harness.run_steps(3);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::B);
    harness.run_steps(3);
    assert_tree_at(&harness, TreeSide::Left);
    let switch = harness.get_by_label_contains("Hide the file tree").rect();
    assert!(switch.right() < 360.0, "the switch sits at the strip's left end: {switch:?}");
    harness.secondary_click_at(harness.get_by_label_contains("Hide the file tree").rect());
    harness.run_steps(2);
    harness.click_at(harness.get_by_role_and_label(Role::MenuItem, TO_RIGHT).rect());
    harness.run_steps(3);
    assert_tree_at(&harness, TreeSide::Right);
    assert_eq!(saved_settings(&harness).file_tree_side, TreeSide::Right);
    let switch = harness.get_by_label_contains("Hide the file tree").rect();
    assert!(switch.left() > 1080.0, "the switch follows to the right end: {switch:?}");

    // With the tree hidden the menu still moves it.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::B);
    harness.run_steps(3);
    assert!(harness.query_by_role_and_label(Role::TreeItem, LONG_FILE).is_none());
    harness.secondary_click_at(harness.get_by_label_contains("Show the file tree").rect());
    harness.run_steps(2);
    harness.click_at(harness.get_by_role_and_label(Role::MenuItem, TO_LEFT).rect());
    harness.run_steps(3);
    assert_eq!(side(&harness), TreeSide::Left);
    assert_eq!(saved_settings(&harness).file_tree_side, TreeSide::Left);
    let switch = harness.get_by_label_contains("Show the file tree").rect();
    assert!(switch.right() < 360.0, "the switch is back at the left end: {switch:?}");
}
