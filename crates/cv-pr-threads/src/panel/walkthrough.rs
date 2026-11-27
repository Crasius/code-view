//! The panel driven through the accessibility tree like a user, no network, no tokens: a small
//! review stepped through with the keyboard and filtered, the general comment's composer, and a
//! 500-thread review where only the rows near the view are laid out, a jump to thread 480 (as a
//! step or "Next unread" makes one) shows it, and N steps on from there.

use cv_base::theme;
use cv_testkit::Clicks;
use cv_widgets::focus;
use egui::Margin;
use egui::accesskit::{Role, Toggled};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{ReviewThread, ThreadStatus};
use review_store::{ThemePreference, ThreadFilters};

use super::{Keys, PanelEvent, PanelInputs, show};
use crate::samples::{comment, erika, jonas, lines, mara, thread};
use crate::{CardOptions, ThreadsUi, samples, show_card};

struct PanelScene {
    threads: Vec<ReviewThread>,
    ui: ThreadsUi,
    filters: ThreadFilters,
    revealed: Vec<String>,
}

fn panel_scene() -> PanelScene {
    let cart = "src/cart.rs";
    let payment = "src/checkout/payment.rs";
    let mut unread = samples::discussion("p-cart-total", lines(cart, 107, 109));
    unread.comments.push(comment("p-cart-total-4", &jonas(), 8, "Pushed in the latest update."));
    let mine = thread(
        "p-payment",
        ThreadStatus::Active,
        Some(lines(payment, 31, 31)),
        vec![
            comment(
                "p-payment-1",
                &erika(),
                140,
                "Retrying a declined card here charges twice when the first call timed out.",
            ),
            comment(
                "p-payment-2",
                &jonas(),
                120,
                "The idempotency key covers that, see `charge_once`.",
            ),
        ],
    );
    let mut outdated = thread(
        "p-lib",
        ThreadStatus::Active,
        Some(lines("src/lib.rs", 12, 12)),
        vec![comment("p-lib-1", &mara(), 900, "Re-export `Cart` here?")],
    );
    outdated.is_outdated = true;
    let general = thread(
        "p-general",
        ThreadStatus::Active,
        None,
        vec![
            comment(
                "p-general-1",
                &mara(),
                50,
                "Nice cleanup overall. Can we get a test for the discount codes before merging?",
            ),
            comment("p-general-2", &jonas(), 4, "Added one in the last push."),
        ],
    );
    PanelScene {
        threads: vec![
            general,
            samples::resolved("p-cart-empty", Some(lines(cart, 116, 116))),
            unread,
            mine,
            outdated,
        ],
        ui: samples::page_ui(),
        filters: ThreadFilters::default(),
        revealed: Vec::new(),
    }
}

fn panel_harness(scene: PanelScene) -> Harness<'static, PanelScene> {
    let builder = Harness::builder().with_size([1180.0, 800.0]).with_context_setup(|ctx| {
        theme::fonts::install(ctx);
        theme::motion::set_reduced(ctx, false);
    });
    builder.build_ui_state(
        |ui, scene: &mut PanelScene| {
            focus::track_modality(ui.ctx());
            let palette = theme::activate(ui.ctx(), ThemePreference::Light);
            ui.painter().rect_filled(ui.max_rect(), 0, palette.ground);
            let file_order = ["src/cart.rs", "src/checkout/payment.rs", "src/lib.rs"];
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
            // The diff beside it: the selected thread's card stands in for it.
            let selected = scene.ui.panel.selected.clone();
            egui::Frame::new().inner_margin(Margin::same(24)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(thread) =
                    scene.threads.iter().find(|thread| Some(&thread.id) == selected.as_ref())
                {
                    show_card(ui, thread, &mut scene.ui, &CardOptions { show_location: true });
                }
            });
            focus::paint_for_focused_widget(ui.ctx());
        },
        scene,
    )
}

#[test]
fn the_panel_steps_through_threads_with_the_keyboard() {
    let mut harness = panel_harness(panel_scene());
    harness.run_steps(2);
    assert!(harness.query_by_label("Active 4").is_some(), "the chip counts the active threads");
    assert!(harness.query_by_label("Unread 2").is_some(), "two threads have news");
    assert!(harness.query_by_label("Mine 2").is_some(), "threads the user wrote in");
    // A click in the panel hands it the arrow keys.
    harness.click_at(harness.get_by_label_contains("Mara Lind, lines 107–109").rect());
    harness.run_steps(2);
    assert_eq!(harness.state().revealed, ["p-cart-total"]);
    harness.key_press(egui::Key::ArrowDown);
    harness.run_steps(2);
    harness.key_press(egui::Key::N);
    harness.run_steps(2);
    assert_eq!(
        harness.state().revealed,
        ["p-cart-total", "p-payment", "p-lib"],
        "files in tree order"
    );
    // Unread only.
    harness.click_at(harness.get_by_label("Unread 2").rect());
    harness.run_steps(2);
    assert!(harness.state().filters.unread);
    assert!(
        harness.query_by_label_contains("Erika Brandt").is_none(),
        "read threads leave the list"
    );
}

#[test]
fn general_comment_opens_its_composer() {
    let mut scene = panel_scene();
    scene.ui.panel.selected = Some("p-cart-total".into());
    let mut harness = panel_harness(scene);
    harness.run_steps(20);

    harness.click_at(harness.get_by_label("General comment").rect());
    harness.run_steps(10);
    assert!(harness.state().ui.panel.is_writing_general, "the click opens the composer");
    assert!(
        harness.query_by_label("General comment").is_none(),
        "the composer takes the place of the footer button"
    );
}

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
        files: (0..samples::REVIEW_FILES).map(samples::review_path).collect(),
        filters: ThreadFilters::ALL,
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
