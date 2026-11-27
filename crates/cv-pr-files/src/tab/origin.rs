//! "Show original version" in the Files tab: the version an outdated thread was written on, in
//! the place of the file view while it shows, and focus mode's strip for it.

use super::super::Env;
use super::super::focus_strip;
use super::super::origin_view::{OriginEvent, OriginInputs, OriginView};
use super::choices::diff_options;
use cv_pr_session::session::PullRequestSession;
use cv_pr_threads::{Jump, ThreadsUi};
use cv_services::Services;

/// What the original version did this frame.
pub enum Shown {
    /// None is open: the file view or the summary shows.
    No,
    /// It took the place of the file view; a thread card in it may ask to go elsewhere.
    Yes(Option<Jump>),
}

/// Opens the original version asked for last frame, once the threads are known.
pub fn open_pending(
    slot: &mut Option<OriginView>,
    pending: &mut Option<String>,
    session: &PullRequestSession,
) {
    if let Some(thread_id) = pending.take() {
        let all = session.threads.ready().map_or(&[][..], Vec::as_slice);
        *slot = all.iter().find(|thread| thread.id == thread_id).and_then(OriginView::new);
    }
}

/// The original version of a thread while one is open; Back closes it.
pub fn show(slot: &mut Option<OriginView>, ui: &mut egui::Ui, env: &mut Env<'_>) -> Shown {
    let Some(origin) = slot.as_mut() else {
        return Shown::No;
    };
    let session = &*env.session;
    let all = session.threads.ready().map_or(&[][..], Vec::as_slice);
    if let Some(thread) = all.iter().find(|thread| thread.id == origin.thread_id()) {
        origin.update_thread(thread);
    }
    let web_url = web_url(origin.thread_id(), session, env.threads_ui);
    let settings = &env.services.settings;
    let mut split = settings.panes.diff_split;
    let mut inputs = OriginInputs {
        mirror: session
            .mirror
            .ready()
            .zip(session.detail.ready())
            .map(|(mirror, detail)| (mirror.git.clone(), detail.target_sha.clone())),
        layout: settings.diff_layout,
        options: diff_options(settings),
        font_size: settings.code_font_size,
        wrap: settings.wrap_lines,
        split: &mut split,
        platform: session.kind,
        web_url,
    };
    let event = origin.show(ui, &mut inputs, env.services, env.threads_ui);
    env.services.settings.panes.diff_split = split;
    match event {
        Some(OriginEvent::Back) => {
            *slot = None;
            ui.ctx().request_repaint();
            Shown::Yes(None)
        }
        Some(OriginEvent::Thread(event)) => {
            Shown::Yes(env.threads_ui.run(event, env.session, env.services, env.actions))
        }
        None => Shown::Yes(None),
    }
}

/// Focus mode's bar while the original version shows. Back to latest lays the frame out again, so
/// the file's own bar replaces this one before anything is painted.
pub fn show_strip(
    slot: &mut Option<OriginView>,
    ui: &mut egui::Ui,
    session: &PullRequestSession,
    threads_ui: &ThreadsUi,
    services: &mut Services,
) {
    let Some(origin) = slot.as_mut() else {
        return;
    };
    let web_url = web_url(origin.thread_id(), session, threads_ui);
    match focus_strip::show_origin(ui, &origin.face(), web_url.as_deref(), session.kind, services) {
        Some(focus_strip::OriginAction::Back) => {
            *slot = None;
            ui.ctx().request_discard("the original version closed");
        }
        Some(focus_strip::OriginAction::Retry) => origin.retry(),
        None => {}
    }
}

/// The comment of a thread on the platform: its own page where the platform gave one, else its
/// place on the pull request.
fn web_url(
    thread_id: &str,
    session: &PullRequestSession,
    threads_ui: &ThreadsUi,
) -> Option<String> {
    let page = threads_ui.context.web_url.as_deref();
    let thread = session.threads.ready()?.iter().find(|thread| thread.id == thread_id)?;
    let own = thread.origin.as_ref().and_then(|origin| origin.web_url.clone());
    own.or_else(|| cv_pr_threads::thread_url(session.provider.as_ref(), page?, thread))
}
