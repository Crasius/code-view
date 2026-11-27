use egui::Ui;

use cv_editor::markdown_editor::{self, Outcome};
use cv_pr_session::session::PullRequestSession;
use cv_pr_threads::{GENERAL_KEY, PostState, ThreadsUi};
use cv_services::Services;
use cv_widgets::settle;

/// "Add a comment" box for general pull request comments. Its draft is the one the threads panel
/// writes too and is kept on disk; the text stays until the post succeeded.
///
/// Posting hands the text on: the box empties at once (busy until the platform has it) while the
/// comment shows at the end of the discussion, where the posted one lands. A failure brings the
/// text back with the reason and Retry. The box's height eases through each hand-off, so the
/// discussion below slides instead of jumping.
pub(crate) fn show(
    ui: &mut Ui,
    session: &PullRequestSession,
    services: &Services,
    threads: &mut ThreadsUi,
) {
    let state = threads.post_state(GENERAL_KEY).cloned();
    let id = egui::Id::unique(("general_comment", session.key.storage_key()));
    let is_settling = threads.is_settling(GENERAL_KEY);
    settle::height(ui, id, is_settling, |ui| {
        let is_pending = state.as_ref().is_some_and(PostState::is_pending);
        let options = markdown_editor::Options {
            can_cancel: false,
            is_busy: is_pending,
            platform: Some(session.kind),
            ..markdown_editor::Options::default()
        };
        if is_pending {
            // The text is on its way and shown in the discussion; the draft stays on disk until
            // the platform has it.
            markdown_editor::show(ui, id, &mut String::new(), &options);
            return;
        }
        let output = markdown_editor::show(ui, id, threads.text_mut(GENERAL_KEY), &options);
        threads.keep_draft(GENERAL_KEY, None);
        if let Some(PostState::Failed { message, needs_token }) = &state {
            ui.add_space(cv_base::theme::style::SPACE_S);
            threads.failure_actions(ui, GENERAL_KEY, message, *needs_token);
        }
        if matches!(output, Some(Outcome::Submitted { .. })) {
            threads.post_general(session, services);
        }
    });
}
