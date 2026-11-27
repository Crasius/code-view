//! The excerpt in a card, driven through the accessibility tree like a user: New only with the
//! commented line marked, Diff with the removed line, Split with the removed line left and the
//! added one right (and Diff on a card too narrow for it), "Show more", the line numbers leading
//! to the file, and a thread on a removed line opening in Diff; above the excerpt, the location
//! link apart from its meta. The code is handed in as loaded; the end-to-end path through the
//! mirror is `app::thread_code_walkthrough`.

use std::fmt::Write as _;

use cv_base::theme;
use cv_testkit::Clicks;
use cv_widgets::focus;
use egui::{Margin, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{ProviderKind, ReviewThread, ThreadAnchor, ThreadStatus};
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

    harness.click_at(harness.get_by_label("Diff").rect());
    harness.run_steps(3);
    assert!(
        harness.query_by_label_contains("Removed line 20: ").is_some(),
        "Diff shows the line the change removed"
    );
    assert!(harness.query_by_label_contains("Added line 21: ").is_some());

    harness.click_at(harness.get_by_label("Show 10 more lines above").rect());
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Line 7: ").is_some(), "ten more lines above");
    harness.click_at(harness.get_by_label("Collapse").rect());
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Line 7: ").is_none(), "Collapse goes back");

    harness.click_at(harness.get_by_label("Show line 20 in the file").rect());
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
    harness.click_at(
        harness.get_all_by_label("Diff").nth(1).expect("the second card's switch").rect(),
    );
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
    harness.click_at(harness.get_by_label("Split").rect());
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
    harness.click_at(
        harness.get_by_label("Show the commented line in the file, from the old side").rect(),
    );
    harness.run_steps(2);
    harness.click_at(harness.get_by_label("Show line 20 in the file").rect());
    harness.run_steps(2);
    let reveal = ThreadEvent::Reveal { thread_id: "t-head".into() };
    assert_eq!(harness.state().events, [reveal.clone(), reveal]);
}

#[test]
fn an_old_side_thread_follows_a_split_pick_and_is_marked_on_the_left() {
    let mut harness = harness_at(scene(vec![on_removed_line(), on_head_line()]), WIDE);
    harness.run_steps(3);
    // Split picked on the other card is the last pick: the old-side thread follows it, not Diff.
    harness.click_at(
        harness.get_all_by_label("Split").nth(1).expect("the second card's switch").rect(),
    );
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
    harness.click_at(harness.get_by_label("Split").rect());
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

#[test]
fn show_more_grows_both_sides_of_split() {
    let mut harness = harness_at(scene(vec![on_head_line()]), WIDE);
    harness.run_steps(3);
    harness.click_at(harness.get_by_label("Split").rect());
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Line 7: ").is_none());
    harness.click_at(harness.get_by_label("Show 10 more lines above").rect());
    harness.run_steps(3);
    let added = line_rect(&harness, "Added line 20, commented: ");
    let grown = line_rect(&harness, "Line 7: ");
    assert!(grown.right() > added.left(), "line 7 is on both sides");
    assert!(grown.left() < added.left() - added.width() / 2.0);
    harness.click_at(harness.get_by_label("Show 10 more lines below").rect());
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Line 33: ").is_some(), "and below");
    harness.click_at(harness.get_by_label("Collapse").rect());
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Line 7: ").is_none(), "Collapse goes back");
    assert!(harness.query_by_label_contains("Removed line 20: ").is_some(), "still in Split");
}
