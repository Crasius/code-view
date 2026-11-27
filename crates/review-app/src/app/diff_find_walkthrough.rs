//! The find widget of the Files tab's diff, end to end: the real app against the mocked GitHub
//! host ([`super::github_mock`]). Side by side the widget searches the focused pane only: a click
//! into the left pane and Ctrl+F count and mark the old version's hits there, a click into the
//! right pane moves the widget and the search to the new version. The search reaches lines inside
//! collapsed unchanged runs, which open when a hit there becomes current, and the current hit
//! scrolls into its pane. Enter and F3 step down, Shift with either steps up, both wrap; Match
//! case switches by its toggle and by Alt+C, and every control is a Phosphor icon, no glyph text;
//! a hit beyond the right edge scrolls its pane
//! sideways into view; inline one pane searches the whole diff. The widget floats at the top
//! right of its pane, inside it and clear of the overview strip, at three window widths. Esc
//! closes it and hands the keyboard back to the diff. The rendered Markdown diff searches its
//! focused column the same way, its hits layered over the changed words. Needs `git` on PATH, no
//! network.

use std::fmt::Write as _;

use egui::{Key, Modifiers, Pos2, Rect, Role, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_store::DiffLayout;

use super::ReviewApp;
use super::github_mock::{ExtraFile, GitHubHost, open_pull_request};
use super::walkthrough::{
    app, files_tab, open_in_tree, shows_file_view, wait_for, wait_until, wheel,
};
use cv_base::theme;
use cv_code::find_paint::{HitSide, last_current as last_current_find};
use cv_pr_diff::diff_overview::{forget_view_box, last_view_box};
use cv_pr_diff::find::bar::{
    BarNote as FindBarNote, has_keyboard as find_has_keyboard, last_bar as last_find_bar,
};
use cv_pr_rendered::markdown_diff::last_layers as last_find_layers;
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const CODE: &str = "src/needle.rs";
const DOC: &str = "docs/needle.md";
const CLOSE: &str = "Close find (Esc)";

/// 200 lines; "needle" is on unchanged lines 3 (`needle_start`) and 80 (`needle_middle`, inside
/// the collapsed run above the change), twice on the old line 150 and once on the new line 160
/// (`needle_new`). The left pane finds 4, the right one 3, inline 5. The unchanged line 152 ends
/// in "distantmark" far beyond the width of either side.
fn code_file() -> ExtraFile {
    let text = |head: bool| {
        (1..=200).fold(String::new(), |mut text, line| {
            let code = match (line, head) {
                (3, _) => "let needle_start = 0;".to_owned(),
                (80, _) => "let needle_middle = 1;".to_owned(),
                (150, false) => "let needle = needle + 1;".to_owned(),
                (150, true) => "let plain = 2;".to_owned(),
                (160, false) => "let plain = 3;".to_owned(),
                (160, true) => "let needle_new = 4;".to_owned(),
                (152, _) => format!("let long = \"{}\"; // distantmark", "x".repeat(300)),
                _ => format!("let value_{line} = compute({line});"),
            };
            writeln!(text, "    {code}").unwrap();
            text
        })
    };
    ExtraFile { path: CODE, base: text(false).into_bytes(), head: text(true).into_bytes() }
}

/// An edited paragraph that loses "needle" and an unchanged one that keeps it: 2 hits in one
/// column, 2 in Before and 1 in After.
fn doc_file() -> ExtraFile {
    ExtraFile {
        path: DOC,
        base: b"# Notes\n\nA needle in the old text.\n\nAn unchanged needle paragraph.\n".to_vec(),
        head: b"# Notes\n\nA thread in the new text.\n\nAn unchanged needle paragraph.\n".to_vec(),
    }
}

#[test]
fn the_find_widget_searches_the_focused_pane() {
    let scratch = ScratchDir::new("diff-find");
    let host = GitHubHost::with_pull_request_files(scratch.path(), &[code_file(), doc_file()]);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    files_tab(&mut harness);
    harness.state_mut().services.settings.diff_layout = DiffLayout::SideBySide;
    open_in_tree(&mut harness, CODE);
    let label = format!("Diff of {CODE}");
    wait_until(&mut harness, "the text diff", |harness| shows_file_view(harness, CODE));
    let diff = harness.get_by_label(&label).rect();
    let (left, right) = (at_share(diff, 0.25), at_share(diff, 0.7));

    // The left pane has the focus: only the old version's hits count, and only there.
    click(&mut harness, left);
    open_find(&mut harness, left);
    harness.event(egui::Event::Text("needle".to_owned()));
    // The first hit sits in the collapsed run: it opens, and the hit scrolls into the pane.
    wait_for_current(&mut harness, "1 of 4", &label);
    let on_left = note(&harness);
    assert_eq!(on_left.side, HitSide::Old);
    assert!(on_left.pane.right() < right.x, "the widget is over the left pane: {on_left:?}");

    harness.key_press(Key::Enter);
    wait_for_current(&mut harness, "2 of 4", &label);
    harness.key_press(Key::F3);
    wait_for_current(&mut harness, "3 of 4", &label);
    harness.key_press_modifiers(Modifiers::SHIFT, Key::F3);
    wait_for_current(&mut harness, "2 of 4", &label);
    for expected in ["3 of 4", "4 of 4", "1 of 4"] {
        harness.key_press(Key::F3);
        wait_for_current(&mut harness, expected, &label);
    }
    harness.key_press_modifiers(Modifiers::SHIFT, Key::Enter);
    wait_for_current(&mut harness, "4 of 4", &label);

    // A click into the right pane moves the widget and the search there, the text kept.
    click(&mut harness, right);
    wait_for_count(&mut harness, |count| count.ends_with(" of 3") || count == "3 matches");
    let on_right = note(&harness);
    assert_eq!(on_right.side, HitSide::New);
    assert!(on_right.pane.left() >= on_left.pane.right() - 1.0, "moved: {on_right:?}");
    assert!(on_right.rect.left() > on_left.rect.right(), "the widget moved right");
    harness.key_press(Key::F3);
    wait_for_current(&mut harness, "1 of 3", &label);

    switch_options(&mut harness, right);

    // A hit beyond the right edge: its pane scrolls sideways to it, side by side and inline.
    open_find(&mut harness, right);
    harness.event(egui::Event::Text("distantmark".to_owned()));
    wait_for_current(&mut harness, "1 of 1", &label);
    harness.key_press(Key::Enter);
    wait_for_current(&mut harness, "1 of 1", &label);
    harness.state_mut().services.settings.diff_layout = DiffLayout::Inline;
    harness.step();
    harness.key_press(Key::Enter);
    wait_for_current(&mut harness, "1 of 1", &label);

    // Inline one pane searches the whole diff: an unchanged line counts once.
    open_find(&mut harness, diff.center());
    harness.event(egui::Event::Text("needle".to_owned()));
    wait_for_count(&mut harness, |count| count.ends_with(" of 5"));
    assert_eq!(note(&harness).side, HitSide::Both);

    keeps_to_its_pane(&mut harness);

    // Esc closes the widget and the diff has the keyboard again.
    open_find(&mut harness, right);
    harness.key_press(Key::Escape);
    wait_until(&mut harness, "the widget to close", |harness| {
        last_find_bar(&harness.ctx).is_none()
    });
    assert!(harness.query_by_label(CLOSE).is_none(), "the widget is gone");
    let focused = harness.ctx.memory(egui::Memory::focused);
    assert!(focused.is_some(), "the diff has the keyboard");
    assert!(!find_has_keyboard(&harness.ctx), "the field let go of it");

    rendered_columns(&mut harness);
}

/// Match case by its toggle and by Alt+C over the right pane, which finds 3 "needle" (see
/// [`code_file`].
fn switch_options(harness: &mut Harness<'_, ReviewApp>, right: Pos2) {
    let three = |count: &str| count.ends_with(" of 3") || count == "3 matches";
    open_find(harness, right);
    harness.event(egui::Event::Text("NEEDLE".to_owned()));
    wait_for_count(harness, three);
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Match case").rect());
    wait_for_count(harness, |count| count == "No results");
    assert!(last_current_find(&harness.ctx).is_empty(), "no hit is painted");
    open_find(harness, right);
    harness.key_press_modifiers(Modifiers::ALT, Key::C);
    wait_for_count(harness, three);

    for name in ["Match case", "Previous match", "Next match", CLOSE] {
        assert!(harness.query_by_label_contains(name).is_some(), "{name} is in the widget");
    }
    for gone in ["Whole word", "Regular expression", "Find in selection", "Aa", ".*", "\u{2261}"] {
        assert!(harness.query_by_label_contains(gone).is_none(), "{gone} is not");
    }
}

/// The widget keeps inside its pane, clear of the overview strip, at three window widths.
fn keeps_to_its_pane(harness: &mut Harness<'_, ReviewApp>) {
    harness.state_mut().services.settings.diff_layout = DiffLayout::SideBySide;
    for width in [1440.0, 1200.0, 960.0] {
        harness.set_size(vec2(width, 900.0));
        harness.run_steps(4);
        let shown = note(harness);
        let (strip, _) = last_view_box(&harness.ctx).expect("the overview strip");
        assert!(shown.pane.contains_rect(shown.rect), "{width}: inside its pane: {shown:?}");
        assert!(!shown.rect.intersects(strip), "{width}: clear of the strip {strip:?}");
        assert!(shown.rect.top() - shown.pane.top() < 16.0, "{width}: at the pane's top");
        assert!(shown.pane.right() - shown.rect.right() < 16.0, "{width}: at its right");
    }
}

/// The rendered Markdown diff: one column searches the whole diff, its hits over the changed
/// words; side by side each column searches its own version.
fn rendered_columns(harness: &mut Harness<'_, ReviewApp>) {
    harness.state_mut().services.settings.diff_layout = DiffLayout::Inline;
    open_in_tree(harness, DOC);
    let paragraph = "An unchanged needle paragraph.";
    wait_until(harness, "the rendered diff", |harness| {
        harness.query_all_by_label(paragraph).next().is_some()
    });
    assert!(harness.query_by_label(&format!("Diff of {DOC}")).is_none(), "shown rendered");
    let rendered = harness.get_all_by_label(paragraph).next().expect("the paragraph").rect();
    open_find(harness, rendered.center());
    harness.event(egui::Event::Text("needle".to_owned()));
    wait_for_count(harness, |count| count == "1 of 2");
    let pane = note(harness).pane;
    let current = wait_for_painted_current(harness);
    assert!(pane.contains_rect(current), "the current block {current:?} in the column {pane:?}");
    // The edited block keeps its changed words under the hit.
    let changed = theme::palette(&harness.ctx).diff_deleted_word;
    let layers = last_find_layers(&harness.ctx);
    assert!(
        layers.iter().any(|colors| colors.len() >= 2 && colors.first() == Some(&changed)),
        "a block with a hit still carries its changed words: {layers:?}"
    );
    harness.key_press(Key::Enter);
    wait_for_count(harness, |count| count == "2 of 2");

    harness.state_mut().services.settings.diff_layout = DiffLayout::SideBySide;
    harness.run_steps(4);
    // Before and After split the width about evenly: the middle of each from the one shown.
    let shown = note(harness);
    let (center, width) = (shown.pane.center(), shown.pane.width());
    let (before, after) = match shown.side {
        HitSide::Old => (center, center + vec2(width, 0.0)),
        HitSide::New | HitSide::Both => (center - vec2(width, 0.0), center),
    };
    click(harness, after);
    wait_for_count(harness, |count| count.ends_with(" of 1") || count == "1 match");
    assert_eq!(note(harness).side, HitSide::New);
    click(harness, before);
    wait_for_count(harness, |count| count.ends_with(" of 2") || count == "2 matches");
    let before = note(harness);
    assert_eq!(before.side, HitSide::Old);
    assert!(before.pane.contains_rect(before.rect), "inside Before: {before:?}");
}

/// The point `share` of the way across `rect`, halfway down.
fn at_share(rect: Rect, share: f32) -> Pos2 {
    pos2(rect.left() + rect.width() * share, rect.center().y)
}

/// A primary click at `at`.
fn click(harness: &mut Harness<'_, ReviewApp>, at: Pos2) {
    harness.click_point(at, egui::PointerButton::Primary);
    harness.run_steps(2);
}

/// Ctrl+F with the pointer at `over`: the widget opens with its field taking the keyboard (what
/// is typed next goes there, replacing its text).
fn open_find(harness: &mut Harness<'_, ReviewApp>, over: Pos2) {
    harness.hover_at(over);
    harness.step();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::F);
    wait_until(harness, "the find widget", |harness| {
        last_find_bar(&harness.ctx).is_some() && harness.query_by_label(CLOSE).is_some()
    });
}

/// What the widget showed last frame.
fn note(harness: &Harness<'_, ReviewApp>) -> FindBarNote {
    last_find_bar(&harness.ctx).expect("the find widget is open")
}

/// Steps until the widget counts `expected` and the current hit is painted inside the pane the
/// widget searches, in view there and not under the widget unless the view is at the file's top.
fn wait_for_current(harness: &mut Harness<'_, ReviewApp>, expected: &str, label: &str) {
    wait_for_count(harness, |count| count == expected);
    let what = format!("hit {expected} inside the searched pane");
    wait_until(harness, &what, |harness| {
        let diff = harness.get_by_label(label).rect();
        let Some(shown) = last_find_bar(&harness.ctx) else {
            return false;
        };
        let painted = last_current_find(&harness.ctx);
        // Only the first lines of the file, which cannot scroll further down, may sit under it.
        let at_top = last_view_box(&harness.ctx)
            .is_some_and(|(strip, view)| (view.top() - strip.top()).abs() < 1.0);
        !painted.is_empty()
            && painted.iter().all(|(current, visible)| {
                visible.contains_rect(*current)
                    && diff.contains_rect(*current)
                    && shown.pane.contains_rect(*current)
                    && (at_top || !current.intersects(shown.rect))
            })
    });
}

/// Steps until the widget's count passes `is_expected`.
fn wait_for_count(harness: &mut Harness<'_, ReviewApp>, is_expected: impl Fn(&str) -> bool) {
    wait_until(harness, "the find widget's count", |harness| {
        last_find_bar(&harness.ctx).is_some_and(|shown| is_expected(&shown.count))
    });
}

/// Steps until a current hit is painted; where.
fn wait_for_painted_current(harness: &mut Harness<'_, ReviewApp>) -> Rect {
    wait_until(harness, "the current hit painted", |harness| {
        !last_current_find(&harness.ctx).is_empty()
    });
    last_current_find(&harness.ctx).first().expect("painted").0
}

/// Every line edited: a diff of 200 changed lines, far taller than the view.
fn long_code(path: &'static str) -> ExtraFile {
    let text = |version: &str| {
        (1..=200).fold(String::new(), |mut text, line| {
            writeln!(text, "    let {version}_{line} = compute({line});").unwrap();
            text
        })
    };
    ExtraFile { path, base: text("old").into_bytes(), head: text("new").into_bytes() }
}

/// Sixty edited paragraphs, `name` in each.
fn long_doc(path: &'static str, name: &str) -> ExtraFile {
    let text = |version: &str| {
        (1..=60).fold(String::new(), |mut text, number| {
            writeln!(text, "Paragraph {number} of {name}, {version} text.\n").unwrap();
            text
        })
    };
    ExtraFile { path, base: text("old").into_bytes(), head: text("new").into_bytes() }
}

/// Opening a file that was not open before starts at its top, wherever the file before was
/// scrolled to: side by side, inline and in the rendered Markdown diff.
#[test]
fn a_newly_opened_file_starts_at_its_top() {
    let scratch = ScratchDir::new("diff-top");
    let files = [
        long_code("src/a.rs"),
        long_code("src/b.rs"),
        long_code("src/c.rs"),
        long_doc("docs/a.md", "alpha"),
        long_doc("docs/b.md", "beta"),
    ];
    let host = GitHubHost::with_pull_request_files(scratch.path(), &files);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    files_tab(&mut harness);
    harness.state_mut().services.settings.diff_layout = DiffLayout::SideBySide;

    for (from, to) in [("src/a.rs", "src/b.rs"), ("src/b.rs", "src/c.rs")] {
        open_in_tree(&mut harness, from);
        wait_until(&mut harness, "the text diff", |harness| shows_file_view(harness, from));
        let diff = harness.get_by_label(&format!("Diff of {from}")).rect();
        scroll_down(&mut harness, diff.center());
        forget_view_box(&harness.ctx);
        open_in_tree(&mut harness, to);
        wait_until(&mut harness, "the next diff", |harness| shows_file_view(harness, to));
        assert_at_top(&mut harness, to);
        // The second pair goes inline.
        harness.state_mut().services.settings.diff_layout = DiffLayout::Inline;
    }

    open_in_tree(&mut harness, "docs/a.md");
    let first = "Paragraph 1 of alpha, new text.";
    wait_until(&mut harness, "the rendered diff", |harness| {
        harness.query_all_by_label(first).next().is_some()
    });
    let paragraph = harness.get_all_by_label(first).next().expect("the paragraph").rect();
    scroll_down(&mut harness, paragraph.center());
    forget_view_box(&harness.ctx);
    open_in_tree(&mut harness, "docs/b.md");
    wait_until(&mut harness, "the next rendered diff", |harness| {
        harness.query_all_by_label("Paragraph 1 of beta, new text.").next().is_some()
    });
    assert_at_top(&mut harness, "docs/b.md");
}

/// Wheels the diff under `over` far down.
fn scroll_down(harness: &mut Harness<'_, ReviewApp>, over: Pos2) {
    harness.hover_at(over);
    harness.step();
    wait_for(
        harness,
        "the diff scrolled down",
        |harness| {
            last_view_box(&harness.ctx)
                .is_some_and(|(strip, view)| view.top() > strip.top() + 100.0)
                .then_some(())
        },
        |harness| {
            harness.event(wheel(-600.0, egui::TouchPhase::Move, egui::MouseWheelSource::Wheel));
        },
    );
}

/// The overview strip's view box of the diff shown sits at the top of the strip, a few frames
/// after the strip painted one: the box of the file before was forgotten, so a slow load cannot
/// leave it standing in for the new file's.
fn assert_at_top(harness: &mut Harness<'_, ReviewApp>, path: &str) {
    wait_until(harness, "the strip of a scrollable diff", |harness| {
        last_view_box(&harness.ctx).is_some()
    });
    harness.run_steps(8);
    let (strip, view) = last_view_box(&harness.ctx).expect("a scrollable diff");
    assert!(
        (view.top() - strip.top()).abs() < 1.0,
        "{path} opens at its top: view box {view:?} in strip {strip:?}"
    );
}
