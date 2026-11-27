//! What the user can do with their own comment in a card: the "Comment actions" menu (Edit,
//! Delete), the composer an edit opens in place of the body, and the delete question inside the
//! card. Platform dialogs never show; a failure stays in the card with its reason and Retry.

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, style};
use cv_editor::markdown_editor::{self, Outcome};
use cv_runtime::intents;
use cv_widgets::{buttons, glyph};
use egui::{Align, Layout, RichText, Ui};
use review_core::provider::CommentActions;
use review_core::{ReviewThread, ThreadComment};

use super::super::edits::Failure;
use super::super::{ThreadEvent, ThreadsUi};
use super::reply::{FailureAction, failure_frame, failure_row};

/// The menu button's name, also for screen readers.
const MENU: &str = "Comment actions";

/// The "Comment actions" button with Edit and Delete as `actions` allow; opens the composer or
/// the delete question when one is picked.
pub(super) fn menu(
    ui: &mut Ui,
    thread: &ReviewThread,
    comment: &ThreadComment,
    threads: &mut ThreadsUi,
    actions: CommentActions,
) {
    let palette = theme::palette(ui.ctx());
    let button = buttons::icon(ui, icons::DOTS_THREE, MENU);
    let (mut edit, mut delete) = (false, false);
    egui::Popup::menu(&button).show(|ui| {
        ui.set_min_width(160.0);
        if actions.can_edit {
            edit = buttons::menu_item(ui, icons::PENCIL_SIMPLE, "Edit", None, None).clicked();
        }
        if actions.can_delete {
            let item = buttons::menu_item(ui, icons::TRASH, "Delete", None, Some(palette.error));
            delete = item.clicked();
        }
    });
    if edit {
        threads.edits.start_edit(&thread.id, &comment.id, &comment.body);
    } else if delete {
        threads.edits.ask_delete(&thread.id, &comment.id);
    }
}

/// The composer on the comment's text, with Save and Cancel; under it the reason a save failed.
pub(super) fn editor(
    ui: &mut Ui,
    thread: &ReviewThread,
    comment: &ThreadComment,
    threads: &mut ThreadsUi,
    failure: Option<&Failure>,
) -> Option<ThreadEvent> {
    let grab_focus = threads.edits.take_focus(&thread.id, &comment.id);
    let platform = threads.context.platform;
    let options = markdown_editor::Options {
        hint: "Edit your comment…",
        submit_label: "Save",
        can_cancel: true,
        grab_focus,
        min_rows: 2,
        initial: Some(&comment.body),
        platform: Some(platform),
        footer_hint: Some(concat!(shortcut!(command Enter), " saves")),
        leading_buttons: true,
        ..markdown_editor::Options::default()
    };
    let id = egui::Id::unique(("comment_edit", &thread.id, &comment.id));
    let text = threads.edits.text_mut(&thread.id, &comment.id)?;
    let mut event = match markdown_editor::show(ui, id, text, &options) {
        Some(Outcome::Submitted { .. }) => Some(edit_event(thread, comment, text.clone())),
        Some(Outcome::Cancelled | Outcome::Dismissed) => {
            threads.edits.cancel(&thread.id, &comment.id);
            None
        }
        None => None,
    };
    if let Some(failure) = failure {
        ui.add_space(6.0);
        match failure_row(ui, &failure.message, failure.needs_token) {
            Some(FailureAction::Retry) => {
                let body = threads.edits.text_mut(&thread.id, &comment.id).cloned();
                event = body.map(|body| edit_event(thread, comment, body));
            }
            Some(FailureAction::Edit) => threads.edits.clear_failure(&thread.id, &comment.id),
            Some(FailureAction::FixToken) => {
                intents::request_fix(ui.ctx(), &threads.context.connection_id);
            }
            None => {}
        }
    }
    event
}

fn edit_event(thread: &ReviewThread, comment: &ThreadComment, body: String) -> ThreadEvent {
    ThreadEvent::EditComment { thread_id: thread.id.clone(), comment_id: comment.id.clone(), body }
}

/// "Delete this comment?" with Delete and Cancel, inside the card under the comment; after a
/// failed try its reason, Retry and Cancel (or Fix token).
pub(super) fn delete_question(
    ui: &mut Ui,
    thread: &ReviewThread,
    comment: &ThreadComment,
    threads: &mut ThreadsUi,
    failure: Option<&Failure>,
) -> Option<ThreadEvent> {
    let palette = theme::palette(ui.ctx());
    let grab_focus = threads.edits.take_focus(&thread.id, &comment.id);
    let (mut confirmed, mut cancelled, mut fix_token) = (false, false, false);
    ui.add_space(style::SPACE_S);
    failure_frame(ui, palette.error, |ui| {
        ui.horizontal(|ui| {
            glyph::decorative(
                ui,
                RichText::new(icons::TRASH)
                    .font(icons::rest_font(&palette, icons::SIZE_SMALL))
                    .color(palette.error),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let cancel = buttons::subtle(ui, icons::X, "Cancel");
                if grab_focus {
                    cancel.request_focus();
                }
                cancelled = cancel.clicked();
                if failure.is_some_and(|failure| failure.needs_token) {
                    fix_token = buttons::subtle(ui, icons::KEY, "Fix token").clicked();
                }
                confirmed = match failure {
                    Some(_) => buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Retry").clicked(),
                    None => buttons::danger(ui, icons::TRASH, "Delete").clicked(),
                };
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let text = match failure {
                        Some(failure) => {
                            format!("Not deleted: {}.", failure.message.trim_end_matches('.'))
                        }
                        None => "Delete this comment? This cannot be undone.".to_owned(),
                    };
                    ui.add(
                        egui::Label::new(
                            RichText::new(text).size(style::SMALL_SIZE + 1.0).color(palette.text),
                        )
                        .wrap(),
                    );
                });
            });
        });
    });
    if fix_token {
        intents::request_fix(ui.ctx(), &threads.context.connection_id);
    }
    if cancelled {
        threads.edits.cancel(&thread.id, &comment.id);
        return None;
    }
    confirmed.then(|| ThreadEvent::DeleteComment {
        thread_id: thread.id.clone(),
        comment_id: comment.id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cv_base::theme;
    use cv_testkit::Clicks;
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use review_core::{Connection, Provider, ProviderKind, ReviewThread, ThreadStatus};
    use review_store::ThemePreference;

    use super::super::super::samples::{self, comment, erika, jonas};
    use super::super::super::{CardOptions, ThreadEvent, ThreadsUi, show_card};

    struct Scene {
        thread: ReviewThread,
        threads: ThreadsUi,
        events: Vec<ThreadEvent>,
    }

    /// A GitHub provider that never sends: only its `comment_actions` are asked.
    fn github() -> Arc<dyn Provider> {
        let connection = Connection {
            id: "test".into(),
            name: "test".into(),
            kind: ProviderKind::GitHub,
            base_url: "http://127.0.0.1:9".into(),
            organizations: Vec::new(),
            token: Some("token".into()),
        };
        Arc::new(provider_github::GitHubProvider::new(connection, reqwest::Client::new()))
    }

    fn harness() -> Harness<'static, Scene> {
        let thread = samples::thread(
            "PRRT_one",
            ThreadStatus::Active,
            None,
            vec![
                comment("1", &jonas(), 30, "Why a Vec here?"),
                comment("2", &erika(), 20, "It grows while parsing."),
            ],
        );
        let mut threads = ThreadsUi::for_tests(ProviderKind::GitHub);
        threads.set_me(erika());
        threads.edits.set_provider(&github());
        let scene = Scene { thread, threads, events: Vec::new() };
        Harness::builder()
            .with_size([900.0, 900.0])
            .with_context_setup(theme::fonts::install)
            .build_ui_state(
                |ui, scene: &mut Scene| {
                    theme::activate(ui.ctx(), ThemePreference::Light);
                    let options = CardOptions { show_location: false };
                    if let Some(event) = show_card(ui, &scene.thread, &mut scene.threads, &options)
                    {
                        scene.events.push(event);
                    }
                },
                scene,
            )
    }

    #[test]
    fn only_the_own_comment_offers_edit_which_saves_the_changed_text() {
        let mut harness = harness();
        harness.run_steps(3);
        assert_eq!(harness.query_all_by_label(super::MENU).count(), 1, "only my comment");
        harness.click_at(harness.get_by_label(super::MENU).rect());
        harness.run_steps(2);
        harness.click_at(harness.get_by_role_and_label(Role::MenuItem, "Edit").rect());
        harness.run_steps(3);
        harness
            .get_by(|node| {
                node.role() == Role::MultilineTextInput
                    && node.value().as_deref() == Some("It grows while parsing.")
            })
            .type_text(" And it is reused.");
        harness.run_steps(2);
        harness.click_at(harness.get_by_role_and_label(Role::Button, "Save").rect());
        harness.run_steps(2);
        assert_eq!(
            harness.state().events,
            vec![ThreadEvent::EditComment {
                thread_id: "PRRT_one".into(),
                comment_id: "2".into(),
                body: "It grows while parsing. And it is reused.".into(),
            }]
        );
    }

    #[test]
    fn delete_asks_inside_the_card_and_cancel_keeps_the_comment() {
        let mut harness = harness();
        harness.run_steps(3);
        harness.click_at(harness.get_by_label(super::MENU).rect());
        harness.run_steps(2);
        harness.click_at(harness.get_by_role_and_label(Role::MenuItem, "Delete").rect());
        harness.run_steps(3);
        assert!(harness.query_by_label("Delete this comment? This cannot be undone.").is_some());
        harness.click_at(harness.get_by_role_and_label(Role::Button, "Cancel").rect());
        harness.run_steps(2);
        assert!(harness.query_by_label("Delete this comment? This cannot be undone.").is_none());
        assert!(harness.state().events.is_empty(), "nothing is deleted");
        harness.click_at(harness.get_by_label(super::MENU).rect());
        harness.run_steps(2);
        harness.click_at(harness.get_by_role_and_label(Role::MenuItem, "Delete").rect());
        harness.run_steps(3);
        harness.click_at(harness.get_by_role_and_label(Role::Button, "Delete").rect());
        harness.run_steps(2);
        assert_eq!(
            harness.state().events,
            vec![ThreadEvent::DeleteComment {
                thread_id: "PRRT_one".into(),
                comment_id: "2".into()
            }]
        );
    }
}
