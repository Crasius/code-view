//! What the user asks of a rendered Markdown diff, in the file view or in a card of the folder
//! summary: commenting on a block's lines or showing them goes to the file's text diff, which
//! scrolls there once it shows; a thread's reply or status goes to the threads.

use review_git::ChangedFile;
use review_store::DiffLayout;

use super::super::{Env, FilesTab};
use cv_pr_diff::diff_view::LineRequest;
use cv_pr_rendered::markdown_diff::RenderedEvent;

/// Carries out `event` of the rendered diff of `path`.
pub fn apply(tab: &mut FilesTab, env: &mut Env<'_>, path: &str, event: RenderedEvent) {
    match event {
        RenderedEvent::Comment { side, lines } => {
            tab.preview.show_source(path);
            tab.line_request = Some((path.to_owned(), LineRequest::Comment { side, lines }));
        }
        RenderedEvent::ShowSource { side, lines } => {
            tab.preview.show_source(path);
            tab.line_request =
                Some((path.to_owned(), LineRequest::Top { side, line: lines.start }));
        }
        RenderedEvent::ShowThreadSource { thread_id } => tab.show_in_text_diff(path, &thread_id),
        RenderedEvent::Thread(event) => {
            if let Some(jump) = env.threads_ui.run(event, env.session, env.services, env.actions) {
                tab.jump(jump);
            }
        }
        RenderedEvent::ShowSideBySide => tab.switch_layout(env.services, DiffLayout::SideBySide),
        RenderedEvent::SplitResized | RenderedEvent::WidthResized => {}
    }
}

/// [`apply`] for the rendered card of `files[file]` in the folder summary: what goes to the text
/// diff opens the file first.
pub fn apply_from_summary(
    tab: &mut FilesTab,
    ui: &egui::Ui,
    env: &mut Env<'_>,
    files: &[ChangedFile],
    file: usize,
    event: RenderedEvent,
) {
    let path = files[file].path.clone();
    let opens_text_diff = matches!(
        event,
        RenderedEvent::Comment { .. }
            | RenderedEvent::ShowSource { .. }
            | RenderedEvent::ShowThreadSource { .. }
    );
    if opens_text_diff {
        tab.mark_asked();
        tab.selected_path = Some(path.clone());
        tab.tree.scroll_to(file);
        ui.ctx().request_repaint();
    }
    apply(tab, env, &path, event);
}
