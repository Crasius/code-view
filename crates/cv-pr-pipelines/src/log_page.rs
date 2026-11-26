//! The pipelines tab's log page beside the run's jobs: the bar with the way back and the attempt
//! switcher, the job list split from the log pane, and the earlier attempt on its way in place
//! of the log. The tab owns the state; this draws it and says what was picked.

use cv_base::theme::{self, icons, style};
use cv_pr_session::session::PullRequestSession;
use cv_services::Services;
use cv_shell::focus_mode;
use cv_widgets::splitter::Splitter;
use cv_widgets::states::{self, Action};
use cv_widgets::{buttons, skeleton};
use egui::{RichText, Ui};
use review_core::PipelineJob;
use review_store::PaneSizes;

use super::attempts::{self, Known};
use super::job_tree::{self, LogTarget};
use super::log::viewer::LogViewer;
use super::panes;
use super::run_header::is_free_key;

/// An earlier attempt's node, as far as it is known.
pub enum AttemptNode {
    Ready(Box<PipelineJob>),
    Loading,
    Failed(String),
    /// The run no longer lists the node or the attempt (a new commit, a reload).
    Gone,
}

/// What the log pane shows instead of a log while an earlier attempt is not there yet.
pub enum AttemptPane {
    Loading(String),
    Failed(String),
}

/// What the log page's job list and log pane picked this frame.
#[derive(Default)]
pub struct LogPicks {
    /// An attempt picked in the switcher: the node and the attempt's number.
    pub attempt: Option<(String, u32)>,
    /// A job or step clicked in the job list.
    pub job: Option<LogTarget>,
    /// The way out picked on an attempt that did not load: 0 is Retry, 1 the latest attempt.
    pub pane: Option<usize>,
}

/// What the log pane shows: the open log, or the earlier attempt on its way.
pub struct LogPane<'a> {
    pub pending: Option<AttemptPane>,
    /// The open item, as the page shows it.
    pub shown: Option<&'a PipelineJob>,
    /// The job (or task) the open log belongs to.
    pub owner: Option<&'a PipelineJob>,
}

/// The job list beside the log.
pub struct JobList<'a> {
    pub stages: &'a [PipelineJob],
    pub selected: Option<&'a str>,
    /// The earlier attempt on screen: its node's id in `stages` and its number.
    pub attempt: Option<(&'a str, u32)>,
    pub known: Known<'a>,
}

/// The node whose attempts the switcher lists, and the attempt on screen.
pub type Switch<'a> = Option<(&'a PipelineJob, u32)>;

/// What the bar above the job list and the log picked.
pub struct BarPicks {
    /// Back to the stages.
    pub back: bool,
    /// An attempt in the switcher: the node and the attempt's number.
    pub attempt: Option<(String, u32)>,
}

/// The bar above the job list and the log: back to the stages, the open step, the attempt
/// switcher. `named_job` is the job the header names.
pub fn log_bar(
    ui: &mut Ui,
    step: Option<&PipelineJob>,
    switch: Switch<'_>,
    known: Known<'_>,
    named_job: Option<&str>,
) -> BarPicks {
    let palette = theme::palette(ui.ctx());
    let border = egui::Stroke::new(1.0, palette.border);
    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), border);
    let mut picks = BarPicks { back: false, attempt: None };
    egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 4)).show(ui, |ui| {
        ui.horizontal(|ui| {
            picks.back = buttons::subtle(ui, icons::ARROW_LEFT, "Stages")
                .on_hover_text("Back to the stage view (Esc)")
                .clicked()
                || (!egui::Popup::is_any_open(ui.ctx()) && is_free_key(ui, egui::Key::Escape));
            if let Some(step) = step {
                ui.separator();
                ui.label(
                    RichText::new(&step.name).font(theme::fonts::semibold_font(style::BODY_SIZE)),
                );
            }
            if let Some((node, shown)) = switch {
                let layout = egui::Layout::right_to_left(egui::Align::Center);
                picks.attempt = ui
                    .with_layout(layout, |ui| bar_switcher(ui, node, shown, known, named_job))
                    .inner;
            }
        });
    });
    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), border);
    picks
}

/// The attempt switcher at the bar's right end, with the outcome of a retried stage around
/// the open job (the header already sums up the job itself).
fn bar_switcher(
    ui: &mut Ui,
    node: &PipelineJob,
    shown: u32,
    known: Known<'_>,
    named_job: Option<&str>,
) -> Option<(String, u32)> {
    let picked = attempts::switcher(ui, node, shown, known, style::CONTROL_HEIGHT - 4.0)
        .map(|number| (node.id.clone(), number));
    if let Some(outcome) = attempts::outcome(node).filter(|_| named_job != Some(node.id.as_str())) {
        let palette = theme::palette(ui.ctx());
        ui.label(RichText::new(outcome).size(style::SMALL_SIZE).color(palette.text_secondary));
    }
    picked
}

/// The earlier attempt on its way, as the log pane shows it in place of the log.
pub fn attempt_pending(state: Option<&AttemptNode>, switch: Switch<'_>) -> Option<AttemptPane> {
    let (node, number) = switch?;
    match state? {
        AttemptNode::Loading => {
            Some(AttemptPane::Loading(format!("Loading attempt {number} of {}…", node.name)))
        }
        AttemptNode::Failed(message) => Some(AttemptPane::Failed(format!(
            "Attempt {number} of {} did not load: {message}",
            node.name
        ))),
        AttemptNode::Ready(_) | AttemptNode::Gone => None,
    }
}

/// Focus mode's log page: the strip with the attempt switcher, the log pane below it.
pub fn focused_log(
    ui: &mut Ui,
    log: &mut LogViewer,
    pane: &LogPane<'_>,
    switch: Switch<'_>,
    known: Known<'_>,
    session: &PullRequestSession,
    services: &mut Services,
) -> LogPicks {
    let mut picks = LogPicks::default();
    log.focus_strip(ui, pane.shown, &services.background, |ui| {
        if let Some((node, number)) = switch
            && let Some(picked) =
                attempts::switcher(ui, node, number, known, focus_mode::strip::CONTROL_HEIGHT)
        {
            picks.attempt = Some((node.id.clone(), picked));
        }
    });
    // The same pane and frame as beside the job list, so the log keeps its place.
    panes::show_second(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::ZERO).show(ui, |ui| {
            picks.pane = attempt_pane(ui, pane, log, session, services);
        });
    });
    picks
}

/// The job list beside the log pane, split by a handle that resizes the list. Returns whether
/// it was resized, and what was picked.
pub fn jobs_beside_log(
    ui: &mut Ui,
    log: &mut LogViewer,
    jobs: &JobList<'_>,
    pane: &LogPane<'_>,
    session: &PullRequestSession,
    services: &mut Services,
) -> (bool, LogPicks) {
    let palette = theme::palette(ui.ctx());
    let mut picks = LogPicks::default();
    let splitter = Splitter::new("pipeline_jobs_split", "Resize the job list")
        .default_value(PaneSizes::DEFAULT.pipeline_jobs);
    let mut jobs_width = services.settings.panes.pipeline_jobs;
    let is_resized = panes::show(
        ui,
        splitter,
        200.0,
        &mut jobs_width,
        |ui| {
            ui.painter().rect_filled(ui.max_rect(), 0, palette.surface);
            picks.job = job_tree::show(ui, jobs.stages, jobs.selected, jobs.attempt, jobs.known);
        },
        |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                picks.pane = attempt_pane(ui, pane, log, session, services);
            });
        },
    );
    services.settings.panes.pipeline_jobs = jobs_width;
    (is_resized, picks)
}

/// The log pane: the open log, or the earlier attempt on its way (loading, or why it did not
/// load with Retry and the way back to the latest). Returns the action picked on a failure.
fn attempt_pane(
    ui: &mut Ui,
    pane: &LogPane<'_>,
    log: &mut LogViewer,
    session: &PullRequestSession,
    services: &mut Services,
) -> Option<usize> {
    let palette = theme::palette(ui.ctx());
    match &pane.pending {
        Some(AttemptPane::Loading(loading)) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(loading).color(palette.text_secondary));
            });
            ui.add_space(8.0);
            skeleton::code_lines(ui, 30, 18.0, false);
            None
        }
        Some(AttemptPane::Failed(failed)) => {
            let actions = [
                Action::primary(icons::ARROW_CLOCKWISE, "Retry"),
                Action::secondary(icons::ARROW_LEFT, "Latest attempt"),
            ];
            states::error_with_actions(ui, failed, &actions)
        }
        None => {
            log.show(ui, pane.shown, pane.owner, session, services);
            None
        }
    }
}
