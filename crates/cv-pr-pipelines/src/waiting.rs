//! What the tab shows while its runs or stages are on the way: a skeleton in the shape of what
//! comes, or, while a rate limit holds the requests back, how long it lasts and the ways forward.
//! Also what a run page shows in place of stages that did not load, or that the run has none of.

use cv_base::theme::{icons, style};
use cv_pr_session::session::PullRequestSession;
use cv_runtime::intents;
use cv_runtime::load::Live;
use cv_services::Services;
use cv_services::connection::traffic::retry_now;
use cv_shell::rate_limit_banner::{countdown, until_next_tick};
use cv_widgets::provider_icon;
use cv_widgets::skeleton;
use cv_widgets::states::{self, Action};
use egui::Ui;
use review_core::ProviderKind;
use review_core::http::Api;

use super::graph::layout as graph_layout;
use super::graph::stage_card;

/// The ways out of waiting for a rate limit.
enum WaitChoice<'a> {
    AddToken,
    RetryNow,
    Open(&'a str),
}

/// While this connection's REST requests wait out a rate-limit pause, which lasts up to an hour on
/// gitlab.com (an IP without a token gets 60 requests an hour), the tab says so with the ways
/// forward instead of a skeleton. Returns false when nothing waits.
fn rate_limit_wait(ui: &mut Ui, session: &PullRequestSession, services: &Services) -> bool {
    let Some(limit) = services
        .rate_limited()
        .into_iter()
        .find(|limit| limit.connection.id == session.key.connection_id && limit.api == Api::Rest)
    else {
        return false;
    };
    // The countdown wakes the page when its text changes: once a minute while it counts minutes.
    ui.ctx().request_repaint_after(until_next_tick(limit.resumes_in));
    let platform = session.kind.label();
    let open_label = states::open_on_platform(session.kind);
    let web_url = session.detail.ready().map(|detail| detail.summary.web_url.clone());
    let mut choices = Vec::new();
    if !session.has_token {
        choices.push((Action::primary(icons::KEY, "Add a token"), WaitChoice::AddToken));
    }
    choices.push((Action::secondary(icons::ARROW_CLOCKWISE, "Retry now"), WaitChoice::RetryNow));
    if let Some(url) = web_url.as_deref() {
        choices.push((
            Action::secondary(provider_icon::glyph(session.kind), open_label.as_str()),
            WaitChoice::Open(url),
        ));
    }
    let resumes_in = countdown(limit.resumes_in);
    let detail = if session.has_token {
        format!(
            "{platform} asked this connection to slow down. The pipelines load on their own in {resumes_in}."
        )
    } else {
        format!(
            "{platform} allows a connection without a token only a few requests an hour. The pipelines load on their own in {resumes_in}; a token raises the limit."
        )
    };
    let actions: Vec<Action<'_>> = choices.iter().map(|(action, _)| *action).collect();
    let title = format!("Waiting for {platform}");
    let picked = egui::Frame::new()
        .inner_margin(egui::Margin::same(style::PAGE_MARGIN_X))
        .show(ui, |ui| states::message_with_actions(ui, icons::CLOCK, &title, &detail, &actions))
        .inner;
    match picked.map(|index| &choices[index].1) {
        Some(WaitChoice::AddToken) => intents::request_fix(ui.ctx(), &session.key.connection_id),
        Some(WaitChoice::RetryNow) => retry_now(&limit.origin),
        Some(WaitChoice::Open(url)) => ui.ctx().open_url(egui::OpenUrl::new_tab(*url)),
        None => {}
    }
    true
}

/// Placeholder in the shape of the checks table: toolbar, header and rows.
pub fn checks_skeleton(ui: &mut Ui) {
    let margin = egui::Margin::symmetric(style::PAGE_MARGIN_X, 20);
    egui::Frame::new().inner_margin(margin).show(ui, |ui| {
        ui.horizontal(|ui| {
            skeleton::bar(ui, 160.0, 20.0);
            for width in [56.0, 72.0, 80.0, 96.0, 80.0] {
                skeleton::bar(ui, width, 28.0);
            }
        });
        ui.add_space(style::SPACE_M + 2.0);
        for _ in 0..6 {
            ui.horizontal(|ui| {
                ui.add_space(style::SPACE_L);
                skeleton::circle(ui, 18.0);
                ui.add_space(style::SPACE_M);
                skeleton::bar(ui, 260.0, 14.0);
            });
            ui.add_space(style::ROW_TWO_LINE - 18.0);
        }
    });
}

/// Placeholder in the shape of the stage view: pivot and a row of cards.
pub fn stage_skeleton(ui: &mut Ui) {
    let margin = egui::Margin::symmetric(style::PAGE_MARGIN_X, 12);
    egui::Frame::new().inner_margin(margin).show(ui, |ui| {
        ui.horizontal(|ui| {
            skeleton::bar(ui, 56.0, 18.0);
            skeleton::bar(ui, 40.0, 18.0);
        });
        ui.add_space(24.0);
        ui.horizontal_top(|ui| {
            for _ in 0..4 {
                skeleton::bar(ui, graph_layout::CARD_WIDTH, stage_card::HEADER_HEIGHT);
                ui.add_space(graph_layout::COLUMN_GAP);
            }
        });
    });
}

/// Waits for the data while a rate limit holds it back, else shows `skeleton` in its shape.
pub fn placeholder(
    ui: &mut Ui,
    session: &PullRequestSession,
    services: &Services,
    skeleton: fn(&mut Ui),
) {
    if !rate_limit_wait(ui, session, services) {
        skeleton(ui);
    }
}

/// A check that reports only a status: where its details are.
pub fn status_only(ui: &mut Ui, platform: ProviderKind, web_url: Option<&str>, open_label: &str) {
    let actions: Vec<Action<'_>> = web_url
        .iter()
        .map(|_| Action::primary(provider_icon::glyph(platform), open_label))
        .collect();
    let detail =
        format!("This check reports only a status. Its details are on {}.", platform.label());
    if states::message_with_actions(ui, icons::INFO, "No job details", &detail, &actions).is_some()
        && let Some(url) = web_url
    {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

/// What the run page shows in place of stages still on their way, stages that did not load, or a
/// run with no jobs yet. True when the user asked for the stages again (Retry, Refresh).
pub fn no_stages(
    ui: &mut Ui,
    run_id: &str,
    session: &PullRequestSession,
    services: &Services,
) -> bool {
    let load = session.stages(run_id);
    match (load.and_then(Live::ready), load.and_then(Live::error)) {
        (Some(_), _) => {
            let refresh = [Action::secondary(icons::ARROW_CLOCKWISE, "Refresh")];
            let no_jobs = "The run has no jobs yet.";
            states::message_with_actions(ui, icons::INFO, "No jobs", no_jobs, &refresh).is_some()
        }
        (None, Some(message)) => egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(style::PAGE_MARGIN_X, 8))
            .show(ui, |ui| {
                let retry = [Action::secondary(icons::ARROW_CLOCKWISE, "Retry")];
                states::error_with_actions(ui, message, &retry)
            })
            .inner
            .is_some(),
        (None, None) => {
            placeholder(ui, session, services, stage_skeleton);
            false
        }
    }
}
