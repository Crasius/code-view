//! The discussion with a 500-thread review, driven through the accessibility tree: only the
//! cards near the view are laid out, at the top and after scrolling to the end.

use egui::Margin;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::ReviewThread;
use review_store::ThemePreference;

use super::cards;
use cv_base::theme;
use cv_pr_threads::{ThreadsUi, samples};
use cv_widgets::scroll;

/// The location link of thread 1's card, near the top of the discussion.
const THREAD_1: &str = "src/module_0/file_1.rs:39";

struct Scene {
    threads: Vec<ReviewThread>,
    ui: ThreadsUi,
    /// Where the discussion scrolls to next frame.
    scroll_to: Option<f32>,
    /// The height of the discussion, as last laid out.
    height: f32,
}

/// The cards in the accessibility tree: an open card has a status pill, a collapsed one is one
/// labelled line.
fn cards_laid_out(harness: &Harness<'_, Scene>) -> usize {
    harness.query_all_by_label_contains("Status: ").count()
        + harness.query_all_by_label_contains("Resolved thread: ").count()
}

#[test]
fn a_long_discussion_lays_out_only_the_cards_near_the_view() {
    let threads = samples::review(500);
    let scene = Scene { ui: samples::review_ui(&threads), threads, scroll_to: None, height: 0.0 };
    let mut harness = Harness::builder()
        .with_size([1100.0, 900.0])
        .with_context_setup(theme::fonts::install)
        .build_ui_state(
            |ui, scene: &mut Scene| {
                theme::activate(ui.ctx(), ThemePreference::Light);
                let mut page = scroll::page("overview", Margin::same(16));
                if let Some(offset) = scene.scroll_to.take() {
                    page = page.vertical_scroll_offset(offset);
                }
                scene.height = page
                    .show(ui, |ui| {
                        let visible: Vec<&ReviewThread> = scene.threads.iter().collect();
                        cards(ui, &visible, &[], &mut scene.ui);
                    })
                    .content_size
                    .y;
            },
            scene,
        );
    harness.run_steps(3);
    let at_top = cards_laid_out(&harness);
    assert!((1..40).contains(&at_top), "the cards near the top, not all 500: {at_top}");
    assert!(harness.query_by_label_contains(THREAD_1).is_some(), "thread 1 is near the top");

    let end = harness.state().height - 900.0;
    harness.state_mut().scroll_to = Some(end);
    harness.run_steps(4);
    let at_end = cards_laid_out(&harness);
    assert!((1..40).contains(&at_end), "the cards near the end: {at_end}");
    assert!(
        harness.query_by_label_contains(THREAD_1).is_none(),
        "the cards at the top are not laid out any more"
    );
}
