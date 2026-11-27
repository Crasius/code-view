//! The thread card's states, driven through the accessibility tree like a user. No network, no
//! tokens.

use cv_base::theme;
use cv_testkit::Clicks;
use cv_widgets::focus;
use egui::Margin;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{ReviewThread, ThreadStatus};
use review_store::ThemePreference;

use super::super::samples::{self, comment, jonas, lines, mara, thread};
use super::super::{CardOptions, PostState, ThreadsUi, drafts, show_card};

struct States {
    cards: Vec<(&'static str, ReviewThread)>,
    threads: ThreadsUi,
}

fn states() -> States {
    let path = "src/cart.rs";
    let active = {
        let mut thread = samples::discussion("t-active", lines(path, 84, 87));
        thread.comments.truncate(2);
        thread
    };
    let mut new = samples::discussion("t-new", lines(path, 96, 96));
    new.comments.push(comment(
        "t-new-4",
        &mara(),
        12,
        "Also: `apply_discount` takes the code as typed. Lowercase it first?",
    ));
    new.comments.push(comment("t-new-5", &jonas(), 6, "Good point, pushing a fix."));
    let resolved = samples::resolved("t-resolved", Some(lines(path, 116, 116)));
    let mut outdated = thread(
        "t-outdated",
        ThreadStatus::Active,
        Some(lines(path, 49, 49)),
        vec![comment(
            "t-outdated-1",
            &mara(),
            600,
            "Is this `println!` meant to stay? It prints on every add.",
        )],
    );
    outdated.is_outdated = true;
    let sending = thread(
        "t-sending",
        ThreadStatus::Active,
        Some(lines(path, 103, 103)),
        vec![comment("t-sending-1", &jonas(), 90, "Should `add` take the quantity?")],
    );
    let failed = thread(
        "t-failed",
        ThreadStatus::Active,
        Some(lines(path, 112, 113)),
        vec![comment("t-failed-1", &mara(), 70, "Drop the comment on the lowercase call?")],
    );
    let mut threads = samples::page_ui();
    threads.set_post_state(
        &drafts::reply_key("t-sending"),
        PostState::Sending,
        "Yes, with `quantity: u32`; callers pass 1 today.",
    );
    let failure = PostState::Failed {
        message: "GitHub refused the token (401 Bad credentials).".into(),
        needs_token: true,
    };
    threads.set_post_state(&drafts::reply_key("t-failed"), failure, "It explains why, keep it.");
    // A failed reply opens its composer again with the text.
    threads.open_reply_for_tests("t-failed", "It explains why, keep it.");
    States {
        cards: vec![
            ("Active, reply box closed", active),
            ("New replies since the last visit", new),
            ("Resolved, collapsed to one line", resolved),
            ("Outdated: the line changed since", outdated),
            ("Reply on its way", sending),
            ("Reply failed: the composer is back with the text, Retry and Fix token", failed),
        ],
        threads,
    }
}

fn states_harness(scene: States) -> Harness<'static, States> {
    Harness::builder()
        .with_size([860.0, 2200.0])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(
            |ui, scene: &mut States| {
                focus::track_modality(ui.ctx());
                let palette = theme::activate(ui.ctx(), ThemePreference::Light);
                ui.painter().rect_filled(ui.max_rect(), 0, palette.ground);
                egui::Frame::new().inner_margin(Margin::same(24)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for (caption, thread) in &scene.cards {
                        ui.label(
                            egui::RichText::new(*caption)
                                .size(theme::style::SMALL_SIZE)
                                .color(palette.text_secondary),
                        );
                        ui.add_space(theme::style::SPACE_XS);
                        show_card(
                            ui,
                            thread,
                            &mut scene.threads,
                            &CardOptions { show_location: false },
                        );
                        ui.add_space(theme::style::SPACE_L + 4.0);
                    }
                });
                focus::paint_for_focused_widget(ui.ctx());
            },
            scene,
        )
}

#[test]
fn thread_states_show_their_cues_and_the_status_menu() {
    let mut harness = states_harness(states());
    harness.run_steps(20);
    assert!(
        harness.query_by_label_contains("2 new replies").is_some(),
        "the new replies are announced"
    );
    assert!(
        harness.query_by_label_contains("Resolved thread: ").is_some(),
        "the resolved thread is one line"
    );
    assert!(harness.query_by_label("Fix token").is_some(), "a refused token offers its fix");
    assert!(harness.query_by_label("Retry").is_some());
    assert!(
        harness.query_by_label_contains("Posting").is_some(),
        "a reply on its way says so in place of its time"
    );
    assert!(
        harness.query_all_by_label("Author").count() > 0,
        "the pull request's author is marked"
    );

    // The status pill is a menu with the states every platform has.
    let resolved_before = harness.query_all_by_label("Resolved").count();
    harness.click_at(
        harness.query_all_by_label("Status: Active").next().expect("an active thread").rect(),
    );
    harness.run_steps(6);
    assert_eq!(
        harness.query_all_by_label("Resolved").count(),
        resolved_before + 1,
        "the menu offers Resolved"
    );
}

#[test]
fn a_collapsed_thread_opens_and_jumping_to_new_replies_marks_them_read() {
    let mut harness = states_harness(states());
    harness.run_steps(2);
    harness.click_at(harness.get_by_label_contains("Resolved thread: ").rect());
    harness.run_steps(2);
    assert!(
        harness.query_by_label_contains("Resolved thread: ").is_none(),
        "Show opens the whole card"
    );
    harness.click_at(harness.get_by_label("Jump to first new").rect());
    harness.run_steps(2);
    let scene = harness.state();
    let new = &scene.cards[1].1;
    assert!(!scene.threads.is_unread(new), "jumping to the new replies reads them");
}
