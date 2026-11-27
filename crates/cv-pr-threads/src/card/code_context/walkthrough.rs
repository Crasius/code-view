//! The excerpt in a card, driven through the accessibility tree like a user: New only with the
//! commented line marked, Diff with the removed line, Split with the removed line left and the
//! added one right (and Diff on a card too narrow for it), "Show more", the line numbers leading
//! to the file, and a thread on a removed line opening in Diff; above the excerpt, the location
//! link apart from its meta; the skeleton at the height of the code that lands in it. The code is
//! handed in as loaded; the end-to-end path through the mirror is `app::thread_code_walkthrough`.

use std::fmt::Write as _;

use cv_base::theme;
use cv_widgets::focus;
use egui::{Margin, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{FileCategory, ProviderKind, ReviewThread, ThreadAnchor, ThreadStatus};
use review_git::{ChangeKind, ChangedFile};
use review_store::ThemePreference;

use super::super::super::samples::{comment, jonas, lines, mara, thread};
use super::super::super::{CardOptions, ThreadEvent, ThreadsUi, show_card};

const PATH: &str = "src/cart.rs";
const DIFF: &str = "@@ -20 +20,2 @@\n-        total += item.price;\n+        let line = item.price_cents * u64::from(item.quantity);\n+        total += line;\n";

/// 41 lines of a cart; lines 20 and 21 replaced line 20 of the base.
fn head_text() -> String {
    let mut text = String::new();
    for line in 1..=41 {
        let code = match line {
            20 => "        let line = item.price_cents * u64::from(item.quantity);".to_owned(),
            21 => "        total += line;".to_owned(),
            _ => format!("    // step {line} of the cart"),
        };
        writeln!(text, "{code}").unwrap();
    }
    text
}

struct Scene {
    cards: Vec<ReviewThread>,
    threads: ThreadsUi,
    events: Vec<ThreadEvent>,
}

fn on_head_line() -> ReviewThread {
    thread(
        "t-head",
        ThreadStatus::Active,
        Some(lines(PATH, 20, 20)),
        vec![comment("t-head-1", &mara(), 30, "Does `line` need its own name?")],
    )
}

fn on_removed_line() -> ReviewThread {
    let anchor = ThreadAnchor {
        path: PATH.into(),
        old_path: None,
        old_line: Some(20),
        new_line: None,
        range: None,
    };
    thread(
        "t-removed",
        ThreadStatus::Active,
        Some(anchor),
        vec![comment("t-removed-1", &jonas(), 20, "This summed the price without the quantity.")],
    )
}

fn scene(cards: Vec<ReviewThread>) -> Scene {
    let mut threads = ThreadsUi::for_tests(ProviderKind::GitHub);
    threads.code.insert_for_tests(&cards[0], "base", "head", DIFF, &head_text());
    Scene { cards, threads, events: Vec::new() }
}

/// Wide enough for Split's two sides.
const WIDE: f32 = 1240.0;
const NARROW: f32 = 760.0;

fn harness(scene: Scene) -> Harness<'static, Scene> {
    harness_at(scene, 860.0)
}

fn harness_at(scene: Scene, width: f32) -> Harness<'static, Scene> {
    let builder = Harness::builder().with_size([width, 1400.0]).with_context_setup(|ctx| {
        theme::fonts::install(ctx);
        theme::motion::set_reduced(ctx, false);
    });
    builder.build_ui_state(
        |ui, scene: &mut Scene| {
            focus::track_modality(ui.ctx());
            let palette = theme::activate(ui.ctx(), ThemePreference::Light);
            ui.painter().rect_filled(ui.max_rect(), 0, palette.ground);
            egui::Frame::new().inner_margin(Margin::same(24)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for thread in &scene.cards {
                    let options = CardOptions { show_location: true };
                    if let Some(event) = show_card(ui, thread, &mut scene.threads, &options) {
                        scene.events.push(event);
                    }
                    ui.add_space(theme::style::SPACE_L);
                }
            });
            focus::paint_for_focused_widget(ui.ctx());
        },
        scene,
    )
}

#[test]
fn the_excerpt_starts_in_new_only_switches_to_diff_and_grows() {
    let mut harness = harness(scene(vec![on_head_line()]));
    harness.run_steps(3);
    assert!(
        harness.query_by_label_contains("Changed line 20, commented: ").is_some(),
        "New only marks the commented line"
    );
    assert!(
        harness.query_by_label_contains("Removed line").is_none(),
        "New only has no removed lines"
    );
    assert!(harness.query_by_label("Lines 17–23").is_some(), "three lines on each side");

    harness.get_by_label("Diff").click();
    harness.run_steps(3);
    assert!(
        harness.query_by_label_contains("Removed line 20: ").is_some(),
        "Diff shows the line the change removed"
    );
    assert!(harness.query_by_label_contains("Added line 21: ").is_some());

    harness.get_by_label("Show 10 more lines above").click();
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Line 7: ").is_some(), "ten more lines above");
    harness.get_by_label("Collapse").click();
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Line 7: ").is_none(), "Collapse goes back");

    harness.get_by_label("Show line 20 in the file").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().events,
        [ThreadEvent::Reveal { thread_id: "t-head".into() }],
        "the line numbers lead to the file"
    );
}

#[test]
fn a_thread_on_a_removed_line_starts_in_diff_and_the_last_pick_carries_over() {
    let mut harness = harness(scene(vec![on_removed_line(), on_head_line()]));
    harness.run_steps(3);
    assert!(
        harness.query_by_label_contains("Removed line 20, commented: ").is_some(),
        "only Diff shows a removed line"
    );
    assert!(
        harness.query_by_label_contains("Changed line 20, commented: ").is_some(),
        "the other card stays in New only"
    );
    // Picking Diff on one card makes it the mode of every card the user has not switched.
    harness.get_all_by_label("Diff").nth(1).expect("the second card's switch").click();
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Changed line 20").is_none());
    assert_eq!(harness.query_all_by_label_contains("Removed line 20").count(), 2);
}

/// The header inherited the 2 points between its icon buttons, so the path ran into the count
/// ("login.go:2842 comments").
#[test]
fn the_location_link_and_its_meta_read_apart() {
    let mut harness = harness(scene(vec![on_head_line()]));
    harness.run_steps(3);
    let link = harness.get_by_label("src/cart.rs:20").rect();
    let meta = harness.get_by_label("1 comment").rect();
    assert!((link.center().y - meta.center().y).abs() < 2.0, "on one row: {link:?} {meta:?}");
    assert!(meta.left() - link.right() >= 6.0, "a gap between them: {link:?} {meta:?}");
}

/// Where the line labeled `label` is.
fn line_rect(harness: &Harness<'_, Scene>, label: &str) -> Rect {
    harness.get_by_label_contains(label).rect()
}

#[test]
fn split_shows_the_removed_line_left_and_the_added_line_right() {
    let mut harness = harness_at(scene(vec![on_head_line()]), WIDE);
    harness.run_steps(3);
    harness.get_by_label("Split").click();
    harness.run_steps(3);
    let removed = line_rect(&harness, "Removed line 20: ");
    let added = line_rect(&harness, "Added line 20, commented: ");
    assert!(removed.right() <= added.left(), "old left, new right: {removed:?} {added:?}");
    assert_eq!(removed.top(), added.top(), "the removed line beside the line that replaced it");
    assert!(
        harness.query_by_label_contains("Removed line 20, commented").is_none(),
        "the mark is on the new side only"
    );
    let unchanged = line_rect(&harness, "Line 19: ");
    assert!(unchanged.width() > added.width() * 1.5, "an unchanged line is read once, across");
    assert!(harness.query_by_label("Too narrow for side by side").is_none());

    // Both gutters lead to the file.
    harness.get_by_label("Show the commented line in the file, from the old side").click();
    harness.run_steps(2);
    harness.get_by_label("Show line 20 in the file").click();
    harness.run_steps(2);
    let reveal = ThreadEvent::Reveal { thread_id: "t-head".into() };
    assert_eq!(harness.state().events, [reveal.clone(), reveal]);
}

#[test]
fn an_old_side_thread_follows_a_split_pick_and_is_marked_on_the_left() {
    let mut harness = harness_at(scene(vec![on_removed_line(), on_head_line()]), WIDE);
    harness.run_steps(3);
    // Split picked on the other card is the last pick: the old-side thread follows it, not Diff.
    harness.get_all_by_label("Split").nth(1).expect("the second card's switch").click();
    harness.run_steps(3);
    let removed = line_rect(&harness, "Removed line 20, commented: ");
    let added = harness
        .query_all_by_label_contains("Added line 20: ")
        .next()
        .expect("the added line, not commented on the old-side card")
        .rect();
    assert!(removed.right() <= added.left(), "the commented removed line is on the left");
    assert_eq!(
        harness.query_all_by_label_contains("Added line 21").count(),
        2,
        "both cards are in Split"
    );
}

#[test]
fn split_on_a_narrow_card_falls_back_to_diff_until_it_is_wide_enough() {
    let mut harness = harness_at(scene(vec![on_head_line()]), NARROW);
    harness.run_steps(3);
    harness.get_by_label("Split").click();
    harness.run_steps(3);
    assert!(harness.query_by_label("Too narrow for side by side").is_some(), "says why");
    let removed = line_rect(&harness, "Removed line 20: ");
    let added = line_rect(&harness, "Added line 20, commented: ");
    assert_eq!(removed.left(), added.left(), "unified: one line above the other");
    assert!(removed.top() < added.top());

    harness.set_size(egui::vec2(WIDE, 1400.0));
    harness.run_steps(3);
    assert!(harness.query_by_label("Too narrow for side by side").is_none());
    let removed = line_rect(&harness, "Removed line 20: ");
    let added = line_rect(&harness, "Added line 20, commented: ");
    assert!(removed.right() <= added.left(), "side by side again");
}

/// The top of the card's reply box, right under the excerpt.
fn reply_top(harness: &Harness<'_, Scene>) -> f32 {
    harness.get_by_label("Reply").rect().top()
}

/// A card on `line` of `path` whose code is not there yet.
fn waiting_harness(path: &str, line: u32, changed: Vec<ChangedFile>) -> Harness<'static, Scene> {
    let card = thread(
        "t-late",
        ThreadStatus::Active,
        Some(lines(path, line, line)),
        vec![comment("t-late-1", &mara(), 30, "Is this the right place?")],
    );
    let mut threads = ThreadsUi::for_tests(ProviderKind::GitHub);
    threads.code.set_changed_for_tests(changed);
    harness(Scene { cards: vec![card], threads, events: Vec::new() })
}

/// The reply box's top under the skeleton and once the code (`diff` over `text`) landed.
fn reply_tops(harness: &mut Harness<'_, Scene>, diff: &str, text: &str) -> [f32; 2] {
    harness.run_steps(3);
    assert!(harness.query_by_label("Loading the code").is_some(), "the skeleton holds the place");
    let waiting = reply_top(harness);
    let scene = harness.state_mut();
    scene.threads.code.insert_for_tests(&scene.cards[0], "base", "head", diff, text);
    harness.run_steps(3);
    assert!(harness.query_by_label("Loading the code").is_none(), "the code landed");
    [waiting, reply_top(harness)]
}

/// `count` lines of a coupon.
fn coupon(count: u32) -> String {
    (1..=count).fold(String::new(), |mut text, line| {
        writeln!(text, "    // coupon step {line}").unwrap();
        text
    })
}

/// A five-line file the pull request adds: its added lines say how long it is.
#[test]
fn the_skeleton_of_a_short_added_file_has_the_height_of_its_code() {
    let path = "src/coupon.rs";
    let file = ChangedFile {
        path: path.into(),
        old_path: None,
        kind: ChangeKind::Added,
        old_oid: "0".repeat(40),
        new_oid: "2".repeat(40),
        additions: Some(5),
        deletions: Some(0),
        is_binary: false,
        category: FileCategory::Code,
        whitespace_only: false,
    };
    let text = coupon(5);
    let diff = text.lines().fold("@@ -0,0 +1,5 @@\n".to_owned(), |mut diff, line| {
        writeln!(diff, "+{line}").unwrap();
        diff
    });
    let mut harness = waiting_harness(path, 2, vec![file]);
    let [waiting, loaded] = reply_tops(&mut harness, &diff, &text);
    assert_eq!(waiting, loaded, "the reply box stays where it was");
    assert!(harness.query_by_label_contains("more line").is_none(), "nothing more to show");
}

/// The context of line 2 starts at line 1: no "Show more" row above it.
#[test]
fn the_skeleton_near_the_start_of_a_file_leaves_no_room_above() {
    let mut harness = waiting_harness(PATH, 2, Vec::new());
    let [waiting, loaded] = reply_tops(&mut harness, DIFF, &head_text());
    assert_eq!(waiting, loaded, "the reply box stays where it was");
    assert!(harness.query_by_label("Show 10 more lines below").is_some());
}

#[test]
fn the_skeleton_in_the_middle_of_a_file_has_both_rows() {
    let mut harness = waiting_harness(PATH, 20, Vec::new());
    let [waiting, loaded] = reply_tops(&mut harness, DIFF, &head_text());
    assert_eq!(waiting, loaded, "the reply box stays where it was");
    assert!(harness.query_by_label("Show 10 more lines above").is_some());
}

/// A push or a refresh loads the code again: the skeleton takes the height the code had, which a
/// short modified file's first load cannot know.
#[test]
fn a_card_that_loads_again_keeps_the_height_its_code_had() {
    let mut harness = waiting_harness(PATH, 5, Vec::new());
    let text = coupon(6);
    let [_, loaded] =
        reply_tops(&mut harness, "@@ -5 +5 @@\n-    // old step\n+    // coupon step 5\n", &text);
    assert!(harness.query_by_label_contains("more line").is_none(), "the whole file");
    harness.state_mut().threads.code.forget_files_for_tests();
    harness.run_steps(3);
    assert!(harness.query_by_label("Loading the code").is_some(), "it loads again");
    assert_eq!(reply_top(&harness), loaded, "the reply box stays where it was");
}

#[test]
fn show_more_grows_both_sides_of_split() {
    let mut harness = harness_at(scene(vec![on_head_line()]), WIDE);
    harness.run_steps(3);
    harness.get_by_label("Split").click();
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Line 7: ").is_none());
    harness.get_by_label("Show 10 more lines above").click();
    harness.run_steps(3);
    let added = line_rect(&harness, "Added line 20, commented: ");
    let grown = line_rect(&harness, "Line 7: ");
    assert!(grown.right() > added.left(), "line 7 is on both sides");
    assert!(grown.left() < added.left() - added.width() / 2.0);
    harness.get_by_label("Show 10 more lines below").click();
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Line 33: ").is_some(), "and below");
    harness.get_by_label("Collapse").click();
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Line 7: ").is_none(), "Collapse goes back");
    assert!(harness.query_by_label_contains("Removed line 20: ").is_some(), "still in Split");
}
