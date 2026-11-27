//! The run's stages and jobs beside the log, as on Azure DevOps' log page: every stage and job,
//! plus the tasks or steps of the job whose log is open. A click always opens the item in the
//! app: its log, a step's log, or (without any log yet) its state; the platform only opens from
//! an explicit "Open in …" action.

use cv_base::theme::{self, icons, style};
use cv_widgets::{badges, elide, row as row_style};
use egui::{Align2, Sense, Ui, pos2, vec2};
use review_core::{PipelineJob, RunStatus};

use super::attempts::{self, Known};
use super::tree::find;

const ROW_HEIGHT: f32 = 32.0;
const INDENT: f32 = 18.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogTarget {
    pub job_id: String,
    /// `None` while the item has no log at all (queued, skipped, not started yet): the log pane
    /// then shows the item's state and the way to the platform.
    pub log_id: Option<String>,
    pub title: String,
    /// The item on the platform, for logs the API does not serve (anonymous GitHub).
    pub web_url: Option<String>,
}

/// `job` opening `log`, with its own page, else the one inherited from its parent.
fn target(job: &PipelineJob, log: Option<&str>, inherited_url: Option<&str>) -> LogTarget {
    LogTarget {
        job_id: job.id.clone(),
        log_id: log.map(str::to_owned),
        title: job.name.clone(),
        web_url: job.web_url.as_deref().or(inherited_url).map(str::to_owned),
    }
}

/// What a click on an item opens in the app, always: its own log, else its first failed step's,
/// else its first step's (Azure DevOps creates a job's own log only once the job finished, its
/// steps' logs as they finish), else its parent's; without any log the item itself, whose state
/// the log pane explains.
pub fn click_target(
    job: &PipelineJob,
    inherited_log: Option<&str>,
    inherited_url: Option<&str>,
) -> LogTarget {
    let own = job.log_id.as_deref().map(|log| target(job, Some(log), inherited_url));
    own.or_else(|| first_log_below(job, inherited_url))
        .unwrap_or_else(|| target(job, inherited_log, inherited_url))
}

/// [`click_target`] for the item with `id` anywhere in the run, with what it inherits from its
/// parents; `None` when the run has no such item (any more).
pub fn resolve(stages: &[PipelineJob], id: &str) -> Option<LogTarget> {
    fn walk(
        nodes: &[PipelineJob],
        id: &str,
        log: Option<&str>,
        url: Option<&str>,
    ) -> Option<LogTarget> {
        nodes.iter().find_map(|node| {
            if node.id == id {
                return Some(click_target(node, log, url));
            }
            walk(
                &node.children,
                id,
                node.log_id.as_deref().or(log),
                node.web_url.as_deref().or(url),
            )
        })
    }
    walk(stages, id, None, None)
}

/// The row's tooltip line about its log; `None` when a click opens a log.
pub fn no_log_hint(job: &PipelineJob, target: &LogTarget) -> Option<String> {
    target
        .log_id
        .is_none()
        .then(|| format!("{}: {}; shows its state here", job.name, no_log_reason(job.status)))
}

/// Why an item has no log, in a few words.
pub fn no_log_reason(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "waiting to start, no log yet",
        RunStatus::Running => "no log yet",
        RunStatus::Skipped => "skipped, no log",
        RunStatus::Canceled => "canceled before it ran, no log",
        RunStatus::Succeeded | RunStatus::Failed | RunStatus::Neutral => "no log",
    }
}

struct Row<'a> {
    job: &'a PipelineJob,
    depth: usize,
    /// What the item inherits from its parents.
    inherited_log: Option<&'a str>,
    inherited_url: Option<&'a str>,
}

impl<'a> Row<'a> {
    fn log_id(&self) -> Option<&'a str> {
        self.job.log_id.as_deref().or(self.inherited_log)
    }

    fn web_url(&self) -> Option<&'a str> {
        self.job.web_url.as_deref().or(self.inherited_url)
    }
}

/// `attempt` is the node shown in an earlier attempt (its id in `stages`) and that attempt's
/// number; its row says so, and retried rows carry their attempt count.
pub fn show(
    ui: &mut Ui,
    stages: &[PipelineJob],
    selected_job: Option<&str>,
    attempt: Option<(&str, u32)>,
    known: Known<'_>,
) -> Option<LogTarget> {
    let mut rows = Vec::new();
    for stage in stages {
        flatten(stage, 0, None, None, selected_job, &mut rows);
    }
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt("job_tree").auto_shrink(false).show_rows(
        ui,
        ROW_HEIGHT,
        rows.len(),
        |ui, range| {
            for row in &rows[range] {
                let is_selected = selected_job == Some(row.job.id.as_str());
                if let Some(clicked) = tree_row(ui, row, is_selected, attempt, known) {
                    picked = Some(clicked);
                }
            }
        },
    );
    picked
}

/// One item: status, name, attempt and duration; returns what a click on it opens.
fn tree_row(
    ui: &mut Ui,
    row: &Row<'_>,
    is_selected: bool,
    attempt: Option<(&str, u32)>,
    known: Known<'_>,
) -> Option<LogTarget> {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TreeItem, true, &row.job.name));
    row_style::list_background(ui, &response, rect, is_selected);
    let left = rect.left() + 12.0 + row.depth as f32 * INDENT;
    let (icon, color) = badges::run_status_icon(row.job.status, &palette);
    let painter = ui.painter_at(rect);
    painter.text(
        pos2(left, rect.center().y),
        Align2::LEFT_CENTER,
        icon,
        icons::rest_font(&palette, icons::SIZE),
        color,
    );
    let duration = super::duration(row.job.started_at, row.job.finished_at);
    let duration_width = if duration.is_some() { 72.0 } else { 8.0 };
    let badge_right = pos2(rect.right() - duration_width, rect.center().y);
    let attempt_number = attempt.filter(|(id, _)| *id == row.job.id).map(|(_, number)| number);
    let right_width =
        duration_width + paint_attempt(ui, &painter, badge_right, row.job, attempt_number, known);
    let name_color =
        if row.job.status == RunStatus::Skipped { palette.text_secondary } else { palette.text };
    let font = if row.depth == 0 {
        theme::fonts::semibold_font(style::BODY_SIZE)
    } else {
        egui::FontId::proportional(style::BODY_SIZE)
    };
    let max_width = (rect.right() - right_width - left - 22.0).max(0.0);
    let galley = elide::line(ui.painter(), &row.job.name, font, name_color, max_width);
    let is_cut = galley.elided;
    painter.galley(pos2(left + 22.0, rect.center().y - galley.size().y / 2.0), galley, name_color);
    if let Some(duration) = duration {
        painter.text(
            pos2(rect.right() - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            duration,
            egui::FontId::proportional(style::SMALL_SIZE),
            palette.text_secondary,
        );
    }
    cv_widgets::focus::ring(ui, &response, rect);
    let clicked = click_target(row.job, row.inherited_log, row.inherited_url);
    let hint = row_hint(row.job, &clicked, is_cut);
    let response = if hint.is_empty() { response } else { response.on_hover_text(hint) };
    response.clicked().then_some(clicked)
}

/// "Attempt N" on the row of the node shown in an earlier attempt, else the attempt count of a
/// retried node, right-aligned at `right`; returns the width it took with its gap (0 for none).
fn paint_attempt(
    ui: &Ui,
    painter: &egui::Painter,
    right: egui::Pos2,
    job: &PipelineJob,
    shown_attempt: Option<u32>,
    known: Known<'_>,
) -> f32 {
    let palette = theme::palette(ui.ctx());
    if let Some(number) = shown_attempt {
        let chip = format!("Attempt {number}");
        let galley = painter.layout_no_wrap(
            chip,
            egui::FontId::proportional(style::SMALL_SIZE),
            palette.accent,
        );
        let size = galley.size();
        painter.galley(pos2(right.x - size.x, right.y - size.y / 2.0), galley, palette.accent);
        return size.x + style::SPACE_S;
    }
    if job.previous_attempts.is_empty() {
        return 0.0;
    }
    let badge = attempts::Badge { node: job, is_compact: true, known, fade: 1.0 };
    attempts::paint_badge(ui, painter, right, badge).width() + style::SPACE_S
}

/// The row's tooltip: why it has no log, else its full name when cut, then its attempts.
fn row_hint(job: &PipelineJob, clicked: &LogTarget, is_cut: bool) -> String {
    let attempts_hint = (!job.previous_attempts.is_empty())
        .then(|| format!("{} attempts; switch between them above the log", job.attempt_count()));
    let first = match (no_log_hint(job, clicked), is_cut) {
        (Some(hint), _) => Some(hint),
        (None, true) => Some(job.name.clone()),
        (None, false) => None,
    };
    first.into_iter().chain(attempts_hint).collect::<Vec<_>>().join("\n")
}

/// Stages and jobs always; deeper levels only on the way to the selected item.
fn flatten<'a>(
    job: &'a PipelineJob,
    depth: usize,
    inherited_log: Option<&'a str>,
    inherited_url: Option<&'a str>,
    selected: Option<&str>,
    rows: &mut Vec<Row<'a>>,
) {
    let row = Row { job, depth, inherited_log, inherited_url };
    let (log_id, web_url) = (row.log_id(), row.web_url());
    rows.push(row);
    let shows_children = depth == 0
        || selected.is_some_and(|selected| find(std::slice::from_ref(job), selected).is_some());
    if shows_children {
        for child in &job.children {
            flatten(child, depth + 1, log_id, web_url, selected, rows);
        }
    }
}

/// The log to open first for a stage: its first failed item with a log, else its first item
/// with a log.
pub fn initial_target(stage: &PipelineJob) -> Option<LogTarget> {
    first_with_log(stage, None, None, is_failed).or_else(|| first_with_log(stage, None, None, any))
}

/// The first failed item with a log in a stage, whatever the stage's own status.
pub fn failed_target(stage: &PipelineJob) -> Option<LogTarget> {
    first_with_log(stage, None, None, is_failed)
}

/// The first failed, else the first item with a log below `job` (not `job` itself).
fn first_log_below(job: &PipelineJob, inherited_url: Option<&str>) -> Option<LogTarget> {
    let url = job.web_url.as_deref().or(inherited_url);
    let first = |wants: fn(&PipelineJob) -> bool| {
        job.children.iter().find_map(|child| first_with_log(child, None, url, wants))
    };
    first(is_failed).or_else(|| first(any))
}

/// The first item `wants`, in tree order from `job` down, that has a log of its own or one it
/// inherits; `log` and `url` are what `job` inherits from its parents.
fn first_with_log(
    job: &PipelineJob,
    log: Option<&str>,
    url: Option<&str>,
    wants: fn(&PipelineJob) -> bool,
) -> Option<LogTarget> {
    let own_log = job.log_id.as_deref().or(log);
    if own_log.is_some() && wants(job) {
        return Some(target(job, own_log, url));
    }
    let own_url = job.web_url.as_deref().or(url);
    job.children.iter().find_map(|child| first_with_log(child, own_log, own_url, wants))
}

fn is_failed(job: &PipelineJob) -> bool {
    job.status == RunStatus::Failed
}

fn any(_: &PipelineJob) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        id: &str,
        status: RunStatus,
        log: Option<&str>,
        children: Vec<PipelineJob>,
    ) -> PipelineJob {
        PipelineJob {
            id: id.to_owned(),
            name: id.to_owned(),
            status,
            log_id: log.map(str::to_owned),
            web_url: Some(format!("https://ado/{id}")),
            children,
            ..PipelineJob::default()
        }
    }

    #[test]
    fn opens_the_first_failed_log_and_inherits_job_logs() {
        let step = node("step", RunStatus::Failed, None, Vec::new());
        let job = node("job", RunStatus::Failed, Some("log-1"), vec![step]);
        let stage = node(
            "stage",
            RunStatus::Failed,
            None,
            vec![node("ok", RunStatus::Succeeded, Some("log-0"), Vec::new()), job],
        );
        let target = initial_target(&stage).expect("a log");
        assert_eq!((target.job_id.as_str(), target.log_id.as_deref()), ("job", Some("log-1")));
    }

    #[test]
    fn shows_tasks_of_the_selected_job_only() {
        let job_a = node(
            "a",
            RunStatus::Succeeded,
            Some("1"),
            vec![node("a1", RunStatus::Succeeded, None, Vec::new())],
        );
        let job_b = node(
            "b",
            RunStatus::Failed,
            Some("2"),
            vec![node("b1", RunStatus::Failed, None, Vec::new())],
        );
        let stage = node("stage", RunStatus::Failed, None, vec![job_a, job_b]);
        let mut rows = Vec::new();
        flatten(&stage, 0, None, None, Some("b1"), &mut rows);
        let ids: Vec<&str> = rows.iter().map(|row| row.job.id.as_str()).collect();
        assert_eq!(ids, vec!["stage", "a", "b", "b1"]);
    }

    fn opened(target: &LogTarget) -> (&str, Option<&str>) {
        (target.job_id.as_str(), target.log_id.as_deref())
    }

    #[test]
    fn a_job_with_its_own_log_opens_it() {
        let job = node(
            "job",
            RunStatus::Failed,
            Some("9"),
            vec![node("t", RunStatus::Failed, Some("10"), Vec::new())],
        );
        assert_eq!(opened(&click_target(&job, None, None)), ("job", Some("9")));
    }

    /// Azure DevOps, job still running: the job has no log yet, its finished steps have theirs.
    #[test]
    fn a_running_job_without_its_own_log_opens_a_step_log() {
        let steps = vec![
            node("init", RunStatus::Succeeded, Some("140"), Vec::new()),
            node("build", RunStatus::Failed, Some("217"), Vec::new()),
            node("test", RunStatus::Running, None, Vec::new()),
        ];
        let job = node("job", RunStatus::Running, None, steps);
        assert_eq!(
            opened(&click_target(&job, None, None)),
            ("build", Some("217")),
            "the first failed step first"
        );
        let mut passing = job;
        passing.children[1].status = RunStatus::Succeeded;
        assert_eq!(
            opened(&click_target(&passing, None, None)),
            ("init", Some("140")),
            "else the first step with a log"
        );
    }

    #[test]
    fn a_step_without_its_own_log_inherits_the_job_log() {
        let step = node("step", RunStatus::Succeeded, None, Vec::new());
        assert_eq!(
            opened(&click_target(&step, Some("5"), Some("https://ado/job"))),
            ("step", Some("5"))
        );
    }

    #[test]
    fn queued_skipped_and_logless_jobs_open_in_the_app_with_their_page() {
        for status in
            [RunStatus::Queued, RunStatus::Skipped, RunStatus::Running, RunStatus::Canceled]
        {
            let job = node(
                "job",
                status,
                None,
                vec![node("pending", RunStatus::Queued, None, Vec::new())],
            );
            let target = click_target(&job, None, Some("https://ado/stage"));
            assert_eq!(opened(&target), ("job", None), "{status:?}");
            assert_eq!(target.web_url.as_deref(), Some("https://ado/job"));
            assert!(
                no_log_hint(&job, &target)
                    .is_some_and(|hint| hint.ends_with("shows its state here"))
            );
        }
    }

    #[test]
    fn resolves_any_item_of_the_run_with_its_inherited_log() {
        let job = node(
            "job",
            RunStatus::Succeeded,
            Some("7"),
            vec![node("step", RunStatus::Succeeded, None, Vec::new())],
        );
        let waiting = node("waiting", RunStatus::Queued, None, Vec::new());
        let stages = vec![node("stage", RunStatus::Running, None, vec![job, waiting])];
        assert_eq!(resolve(&stages, "step").as_ref().map(opened), Some(("step", Some("7"))));
        assert_eq!(resolve(&stages, "waiting").as_ref().map(opened), Some(("waiting", None)));
        assert_eq!(
            resolve(&stages, "stage").as_ref().map(opened),
            Some(("job", Some("7"))),
            "a stage opens its first log"
        );
        assert_eq!(resolve(&stages, "gone"), None);
    }
}
