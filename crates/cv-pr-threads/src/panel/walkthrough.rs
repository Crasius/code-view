//! The panel with a 500-thread review, driven through the accessibility tree: only the rows near
//! the view are laid out, a jump to thread 480 (as a step or "Next unread" makes one) shows it,
//! and N steps on from there.

use cv_base::theme;
use egui::accesskit::{Role, Toggled};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::ReviewThread;
use review_store::{ThemePreference, ThreadFilters};

use super::{Keys, PanelEvent, PanelInputs, show};
use crate::{ThreadsUi, samples};

const HEIGHT: f32 = 800.0;

struct Scene {
    threads: Vec<ReviewThread>,
    files: Vec<String>,
    ui: ThreadsUi,
    filters: ThreadFilters,
    revealed: Vec<String>,
}

fn harness() -> Harness<'static, Scene> {
    let threads = samples::review(500);
    let scene = Scene {
        ui: samples::review_ui(&threads),
        threads,
        files: (0..samples::REVIEW_FILES).map(|index| samples::review_file(index).0).collect(),
        filters: ThreadFilters { active: false, resolved: false, unread: false, mine: false },
        revealed: Vec::new(),
    };
    Harness::builder()
        .with_size([1180.0, HEIGHT])
        .with_context_setup(theme::fonts::install)
        .build_ui_state(
            |ui, scene: &mut Scene| {
                theme::activate(ui.ctx(), ThemePreference::Light);
                let file_order: Vec<&str> = scene.files.iter().map(String::as_str).collect();
                let inputs = PanelInputs {
                    threads: &scene.threads,
                    file_order: &file_order,
                    filters: &mut scene.filters,
                    width: 380.0,
                };
                let (events, _) = show(ui, &mut scene.ui, inputs);
                for event in events {
                    if let PanelEvent::Reveal { thread_id, .. } = event {
                        scene.revealed.push(thread_id);
                    }
                }
            },
            scene,
        )
}

/// The thread rows in the accessibility tree: laid out this frame.
fn rows(harness: &Harness<'_, Scene>) -> usize {
    harness
        .query_all_by(|node| {
            node.role() == Role::Button && node.label().is_some_and(|label| label.contains(" · "))
        })
        .count()
}

/// The selected row's top and bottom.
fn selected_row(harness: &Harness<'_, Scene>) -> Option<(f32, f32)> {
    harness
        .query_by(|node| node.role() == Role::Button && node.toggled() == Some(Toggled::True))
        .map(|node| (node.rect().top(), node.rect().bottom()))
}

#[test]
fn a_long_list_lays_out_only_rows_near_the_view_and_shows_a_jumped_to_thread() {
    let mut harness = harness();
    harness.run_steps(3);
    let laid_out = rows(&harness);
    assert!(
        (5..60).contains(&laid_out),
        "only the rows near the view are laid out, not all 500: {laid_out}"
    );
    assert!(selected_row(&harness).is_none());

    // A jump to thread 480, far down the list.
    let panel = &mut harness.state_mut().ui.panel;
    panel.selected = Some("r-480".to_owned());
    panel.scroll_to_selected = true;
    panel.keys = Keys::Panel;
    harness.run_steps(4);
    let (top, bottom) = selected_row(&harness).expect("the jumped-to row is laid out");
    assert!(top >= 0.0 && bottom <= HEIGHT, "the jumped-to row is in view: {top}..{bottom}");
    assert!(rows(&harness) < 60, "the rows around it, not those between");

    // N steps on to the next thread and shows it.
    harness.key_press(egui::Key::N);
    harness.run_steps(4);
    let next = harness.state().ui.panel.selected.clone().expect("a selected thread");
    assert_ne!(next, "r-480");
    assert_eq!(harness.state().revealed.last(), Some(&next), "the step shows the thread");
    let (top, bottom) = selected_row(&harness).expect("the next row is laid out");
    assert!(top >= 0.0 && bottom <= HEIGHT, "the next row is in view");
}
