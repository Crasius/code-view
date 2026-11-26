#[cfg(test)]
mod frame_budget;
#[cfg(test)]
mod walkthrough;

use egui::{RichText, Ui};
use review_core::{ReviewThread, ThreadStatus};

use cv_base::theme::{self, icons, motion};
use cv_pr_session::actions::ActionRunner;
use cv_pr_session::session::PullRequestSession;
use cv_pr_threads::{CardOptions, Jump, PostState, ThreadsUi, lazy_list};
use cv_services::Services;
use cv_widgets::states::{self, Action};
use cv_widgets::{checkbox, settle, skeleton, tabs};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Active,
    Resolved,
}

/// All conversations, newest last. Returns where a card asked the Files tab to go.
pub fn show(
    ui: &mut Ui,
    session: &mut PullRequestSession,
    services: &Services,
    actions: &mut ActionRunner,
    threads_ui: &mut ThreadsUi,
) -> Option<Jump> {
    let palette = theme::palette(ui.ctx());
    let is_ready = session.threads.ready().is_some();
    let opacity = motion::reveal(ui.ctx(), egui::Id::unique(("timeline", &session.key)), is_ready);
    if let Some(message) = session.threads.error() {
        let message = message.to_owned();
        let retry = [Action::secondary(icons::ARROW_CLOCKWISE, "Retry")];
        if states::error_with_actions(ui, &message, &retry).is_some() {
            session.reload_threads(services);
        }
        return None;
    }
    let Some(threads) = session.threads.ready() else {
        skeleton::thread_cards(ui, 3);
        return None;
    };
    ui.multiply_opacity(opacity);
    let (filter, show_system) = filter_bar(ui, threads);
    ui.add_space(8.0);

    let visible: Vec<_> = threads
        .iter()
        .filter(|thread| show_system || !thread.is_system)
        .filter(|thread| match filter {
            Filter::All => true,
            Filter::Active => thread.status == ThreadStatus::Active,
            Filter::Resolved => thread.status == ThreadStatus::Resolved,
        })
        .collect();
    // General comments on their way show at the end, where the posted ones land.
    let pending: Vec<(String, ReviewThread)> = if filter == Filter::Resolved {
        Vec::new()
    } else {
        threads_ui
            .pending_threads()
            .into_iter()
            .filter(|(_, thread)| thread.anchor.is_none())
            .collect()
    };
    if visible.is_empty() && pending.is_empty() {
        ui.label(RichText::new("No comments yet.").color(palette.text_secondary));
        return None;
    }
    let events = cards(ui, &visible, &pending, threads_ui);
    let mut jump = None;
    for event in events {
        if let Some(target) = threads_ui.run(event, session, services, actions) {
            jump = Some(target);
        }
    }
    jump
}

/// "Discussion" with the All/Active/Resolved filter and "Show updates", both kept for the
/// session; returns the filter and whether system updates show.
fn filter_bar(ui: &mut Ui, threads: &[ReviewThread]) -> (Filter, bool) {
    let filter_id = ui.scope_id().with("timeline_filter");
    let mut filter = ui.ctx().data(|data| data.get_temp(filter_id)).unwrap_or(Filter::All);
    let mut show_system =
        ui.ctx().data(|data| data.get_temp(filter_id.with("system"))).unwrap_or(false);
    let count = |status| {
        threads.iter().filter(|thread| !thread.is_system && thread.status == status).count()
    };
    let (active, resolved) = (count(ThreadStatus::Active), count(ThreadStatus::Resolved));
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Discussion")
                .font(theme::fonts::semibold_font(theme::style::SECTION_SIZE)),
        );
        ui.add_space(12.0);
        let all = format!("All {}", active + resolved);
        let active_label = format!("Active {active}");
        let resolved_label = format!("Resolved {resolved}");
        tabs::segmented(
            ui,
            &mut filter,
            &[
                (Filter::All, &all),
                (Filter::Active, &active_label),
                (Filter::Resolved, &resolved_label),
            ],
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            checkbox::checkbox(ui, &mut show_system, "Show updates");
        });
    });
    ui.ctx().data_mut(|data| {
        data.insert_temp(filter_id, filter);
        data.insert_temp(filter_id.with("system"), show_system);
    });
    (filter, show_system)
}

/// The loaded threads' cards, only those near the view laid out ([`lazy_list`]), then the
/// pending ones'; returns what the cards asked for.
fn cards(
    ui: &mut Ui,
    visible: &[&ReviewThread],
    pending: &[(String, ReviewThread)],
    threads_ui: &mut ThreadsUi,
) -> Vec<cv_pr_threads::ThreadEvent> {
    let options = CardOptions { show_location: true };
    let mut events = Vec::new();
    let mut heights = std::mem::take(&mut threads_ui.timeline_heights);
    for thread in visible {
        // A card whose comment was just handed on eases to its new height; the slot of a loaded
        // thread that replaced a pending one is the pending one's, so its height carries over.
        let slot = threads_ui.card_slot(thread);
        let is_settling = threads_ui.is_settling(&thread.id);
        lazy_list::block(ui, &mut heights, &slot, is_settling, |ui| {
            let top = ui.cursor().top();
            let shown =
                settle::height(ui, egui::Id::unique(("timeline_card", &slot)), is_settling, |ui| {
                    cv_pr_threads::show_card(ui, thread, threads_ui, &options)
                });
            // A card the pointer rests on has its file loaded ahead for its Show in file.
            let card =
                egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().top());
            if ui.rect_contains_pointer(card) {
                cv_pr_threads::hover::note(ui.ctx(), threads_ui, thread);
            }
            events.extend(shown);
            ui.add_space(theme::style::SPACE_M);
        });
    }
    threads_ui.timeline_heights = heights;
    for (key, thread) in pending {
        let is_sending = threads_ui.post_state(key) == Some(&PostState::Sending);
        settle::height(
            ui,
            egui::Id::unique(("timeline_card", cv_pr_threads::pending_id(key))),
            true,
            |ui| {
                cv_pr_threads::show_pending_card(ui, thread, threads_ui, &options, is_sending);
            },
        );
        ui.add_space(theme::style::SPACE_M);
    }
    events
}
