//! Earlier attempts of a stage or job, as Azure DevOps shows them: a badge on retried cards and
//! rows ("2 attempts" with the earlier attempt's outcome), a switcher above the log ("Attempt 2
//! of 2") listing every attempt with its outcome, duration and start, and the words that sum a
//! retry up ("passed on retry"). The node is always the latest attempt; counts stay its own.

use cv_base::theme::{self, Palette, icons, motion, style};
use cv_widgets::{badges, elide, row, time_text};
use egui::{
    Align2, Color32, FontId, Id, Painter, PopupCloseBehavior, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, pos2, vec2,
};
use review_core::{PipelineAttempt, PipelineJob, RunStatus, Timestamp};

const BADGE_HEIGHT: f32 = 20.0;
const BADGE_PADDING: f32 = 6.0;
const ICON_SIZE: f32 = icons::SIZE_SMALL;
const POPUP_WIDTH: f32 = 320.0;
const OPTION_HEIGHT: f32 = style::ROW_TWO_LINE - 8.0;

/// Earlier attempts loaded on demand (Azure DevOps) so far, by attempt target.
#[derive(Default, Clone, Copy)]
pub struct Known<'a> {
    pub loaded: &'a [(&'a str, &'a PipelineJob)],
}

impl<'a> Known<'a> {
    /// The attempt's node: reported with the run, or loaded since.
    pub fn job(&self, attempt: &'a PipelineAttempt) -> Option<&'a PipelineJob> {
        attempt.job().or_else(|| {
            let target = attempt.lazy_target()?;
            self.loaded.iter().find(|(loaded, _)| *loaded == target).map(|(_, job)| *job)
        })
    }
}

/// One attempt as the switcher lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Attempt {
    pub number: u32,
    /// `None` while an attempt listed by id only is not loaded.
    pub status: Option<RunStatus>,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub is_latest: bool,
}

impl Attempt {
    /// "Failed, latest": its outcome, or that it opens on demand, and whether it is the latest.
    fn state(&self) -> String {
        let state = self.status.map_or("opens on demand", badges::run_status_label);
        let latest = if self.is_latest { ", latest" } else { "" };
        format!("{state}{latest}")
    }
}

/// The icon of an attempt's outcome, or a history mark while it is not loaded.
fn status_icon(status: Option<RunStatus>, palette: &Palette) -> (&'static str, Color32) {
    status.map_or((icons::CLOCK_COUNTER_CLOCKWISE, palette.text_secondary), |status| {
        badges::run_status_icon(status, palette)
    })
}

/// Every attempt of `node`, newest first.
pub fn list(node: &PipelineJob, known: Known<'_>) -> Vec<Attempt> {
    let latest = Attempt {
        number: node.attempt_number(),
        status: Some(node.status),
        started_at: node.started_at,
        finished_at: node.finished_at,
        is_latest: true,
    };
    let earlier = node.previous_attempts.iter().rev().map(|attempt| {
        let job = known.job(attempt);
        Attempt {
            number: attempt.number,
            status: job.map(|job| job.status),
            started_at: job.and_then(|job| job.started_at),
            finished_at: job.and_then(|job| job.finished_at),
            is_latest: false,
        }
    });
    std::iter::once(latest).chain(earlier).collect()
}

/// The node whose attempts a stage card shows: the stage itself, or the one job of a stage that
/// stands for its job (GitHub and GitLab group jobs into stages that are never retried as such).
pub fn retried(stage: &PipelineJob) -> Option<&PipelineJob> {
    if !stage.previous_attempts.is_empty() {
        return Some(stage);
    }
    match stage.children.as_slice() {
        [only] if !only.previous_attempts.is_empty() => Some(only),
        _ => None,
    }
}

/// "passed on retry", "failed after 2 attempts": a retried node's outcome in a few words; `None`
/// for a node that ran once.
pub fn outcome(node: &PipelineJob) -> Option<String> {
    let count = node.attempt_count();
    if count < 2 {
        return None;
    }
    Some(match node.status {
        RunStatus::Succeeded | RunStatus::Neutral => "passed on retry".to_owned(),
        RunStatus::Failed => format!("failed after {count} attempts"),
        RunStatus::Canceled => format!("canceled in attempt {}", node.attempt_number()),
        RunStatus::Running | RunStatus::Queued => {
            format!("attempt {} running", node.attempt_number())
        }
        RunStatus::Skipped => format!("skipped in attempt {}", node.attempt_number()),
    })
}

/// The tooltip of a badge: every attempt's outcome, and how to switch.
pub fn tooltip(node: &PipelineJob, known: Known<'_>) -> String {
    let lines: Vec<String> = list(node, known)
        .iter()
        .rev()
        .map(|attempt| format!("Attempt {}: {}", attempt.number, attempt.state()))
        .collect();
    format!(
        "{} attempts\n{}\nClick to open an earlier attempt",
        node.attempt_count(),
        lines.join("\n")
    )
}

/// The icon of the attempt before the latest: its outcome, or a history mark while it is not loaded.
fn earlier_icon(
    node: &PipelineJob,
    known: Known<'_>,
    palette: &Palette,
) -> (&'static str, Color32) {
    let status =
        node.previous_attempts.last().and_then(|attempt| known.job(attempt)).map(|job| job.status);
    status_icon(status, palette)
}

fn badge_text(node: &PipelineJob, is_compact: bool) -> String {
    let count = node.attempt_count();
    if is_compact { count.to_string() } else { format!("{count} attempts") }
}

/// Width of the badge, for the space a row keeps for it.
pub fn badge_width(ui: &Ui, node: &PipelineJob, is_compact: bool) -> f32 {
    let font = FontId::proportional(style::SMALL_SIZE);
    let text = ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap(badge_text(node, is_compact), font, Color32::WHITE).size().x
    });
    BADGE_PADDING * 2.0 + ICON_SIZE + style::SPACE_XS + text
}

/// One node's attempt badge as a row or card line paints it.
#[derive(Clone, Copy)]
pub struct Badge<'a> {
    pub node: &'a PipelineJob,
    /// Compact badges ("2") fit job rows, full ones ("2 attempts") card lines.
    pub is_compact: bool,
    pub known: Known<'a>,
    /// Dims the badge while a card opens; 1.0 when it is fully shown.
    pub fade: f32,
}

/// Paints the badge with its right edge at `right_center`; returns its rect.
pub fn paint_badge(ui: &Ui, painter: &Painter, right_center: Pos2, badge: Badge<'_>) -> Rect {
    let Badge { node, is_compact, known, fade } = badge;
    let palette = theme::palette(ui.ctx());
    let width = badge_width(ui, node, is_compact);
    let rect = Rect::from_min_size(
        pos2(right_center.x - width, right_center.y - BADGE_HEIGHT / 2.0),
        vec2(width, BADGE_HEIGHT),
    );
    painter.rect(
        rect,
        BADGE_HEIGHT / 2.0,
        palette.surface_subtle.gamma_multiply(fade),
        Stroke::new(1.0, palette.border.gamma_multiply(fade)),
        StrokeKind::Inside,
    );
    let (icon, color) = earlier_icon(node, known, &palette);
    let icon_center = pos2(rect.left() + BADGE_PADDING + ICON_SIZE / 2.0, rect.center().y);
    painter.text(
        icon_center,
        Align2::CENTER_CENTER,
        icon,
        icons::rest_font(&palette, ICON_SIZE),
        color.gamma_multiply(fade),
    );
    painter.text(
        pos2(icon_center.x + ICON_SIZE / 2.0 + style::SPACE_XS, rect.center().y),
        Align2::LEFT_CENTER,
        badge_text(node, is_compact),
        FontId::proportional(style::SMALL_SIZE),
        palette.text_secondary.gamma_multiply(fade),
    );
    rect
}

/// A clickable badge that opens the attempt list; returns the attempt picked. `id` must be
/// stable for the badge (it keys the popup).
pub fn badge_button(
    ui: &mut Ui,
    id: Id,
    painter: &Painter,
    right_center: Pos2,
    badge: Badge<'_>,
) -> Option<u32> {
    let rect = paint_badge(ui, painter, right_center, badge);
    let Badge { node, known, .. } = badge;
    let response = ui.interact(rect.intersect(painter.clip_rect()), id, Sense::click());
    let label = format!("{}, {} attempts. Switch attempt", node.name, node.attempt_count());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    let hover = row::hover_amount(ui, &response);
    if hover > 0.0 {
        painter.rect_stroke(
            rect,
            BADGE_HEIGHT / 2.0,
            Stroke::new(1.0, theme::palette(ui.ctx()).border_strong.gamma_multiply(hover)),
            StrokeKind::Inside,
        );
    }
    cv_widgets::focus::ring(ui, &response, rect);
    let response = response.on_hover_text(tooltip(node, known));
    popup(ui, &response, &list(node, known), node.attempt_number())
}

/// The switcher above a log: "Attempt 2 of 2" with the shown attempt's outcome; returns the
/// attempt picked. `shown` is the attempt on screen; `height` that of the bar it sits in.
pub fn switcher(
    ui: &mut Ui,
    node: &PipelineJob,
    shown: u32,
    known: Known<'_>,
    height: f32,
) -> Option<u32> {
    let palette = theme::palette(ui.ctx());
    let attempts = list(node, known);
    let current = attempts.iter().find(|attempt| attempt.number == shown);
    let text = format!("Attempt {shown} of {}", attempts.len());
    let font = FontId::proportional(style::BODY_SIZE);
    let text_width = ui.painter().layout_no_wrap(text.clone(), font.clone(), palette.text).size().x;
    let width =
        style::SPACE_S * 2.0 + ICON_SIZE + style::SPACE_S + text_width + style::SPACE_S + 12.0;
    let (rect, button) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let is_open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&button));
    let label = format!("{text}. Switch attempt");
    button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, &label));
    let hover = row::hover_amount(ui, &button);
    let fill = if button.is_pointer_button_down_on() {
        palette.pressed
    } else {
        motion::mix(palette.surface_subtle, palette.hover, hover)
    };
    let border = if is_open {
        palette.accent
    } else {
        motion::mix(palette.border_strong, palette.text_secondary, hover)
    };
    ui.painter().rect(
        rect,
        style::RADIUS_CONTROL,
        fill,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    let (icon, color) = status_icon(current.and_then(|attempt| attempt.status), &palette);
    let icon_center = pos2(rect.left() + style::SPACE_S + ICON_SIZE / 2.0, rect.center().y);
    ui.painter().text(
        icon_center,
        Align2::CENTER_CENTER,
        icon,
        icons::rest_font(&palette, ICON_SIZE),
        color,
    );
    ui.painter().text(
        pos2(icon_center.x + ICON_SIZE / 2.0 + style::SPACE_S, rect.center().y),
        Align2::LEFT_CENTER,
        &text,
        font,
        palette.text,
    );
    let caret = if is_open { icons::CARET_UP } else { icons::CARET_DOWN };
    ui.painter().text(
        pos2(rect.right() - style::SPACE_S, rect.center().y),
        Align2::RIGHT_CENTER,
        caret,
        icons::rest_font(&palette, icons::SIZE_SMALL),
        palette.text_secondary,
    );
    cv_widgets::focus::ring(ui, &button, rect);
    let button = button.on_hover_text("Switch between this job's attempts");
    popup(ui, &button, &attempts, shown)
}

/// The attempt list below `anchor`, toggled by it; returns the attempt picked and closes.
fn popup(ui: &Ui, anchor: &Response, attempts: &[Attempt], shown: u32) -> Option<u32> {
    let popup_id = egui::Popup::default_response_id(anchor);
    let picked = egui::Popup::from_toggle_button_response(anchor)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .width(POPUP_WIDTH)
        .gap(style::SPACE_XS)
        .accessible_name("Switch attempt")
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                egui::Popup::close_id(ui.ctx(), popup_id);
            }
            let mut picked = None;
            for attempt in attempts {
                if option(ui, attempt, attempt.number == shown) {
                    picked = Some(attempt.number);
                }
            }
            picked
        })
        .and_then(|response| response.inner);
    if picked.is_some() {
        egui::Popup::close_id(ui.ctx(), popup_id);
    }
    picked
}

/// The line under an attempt's title: "Failed, started 5 minutes ago", or that it loads on demand.
fn detail_text(attempt: &Attempt) -> String {
    match (attempt.status, attempt.started_at) {
        (None, _) => "Loads when you open it".to_owned(),
        (Some(status), Some(started)) => format!(
            "{}, started {}",
            badges::run_status_label(status),
            time_text::relative(started)
        ),
        (Some(status), None) => badges::run_status_label(status).to_owned(),
    }
}

/// One attempt: outcome, "Attempt 1", "Latest", its start below; duration on the right.
fn option(ui: &mut Ui, attempt: &Attempt, is_current: bool) -> bool {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), OPTION_HEIGHT), Sense::click());
    let label = format!("Attempt {}, {}", attempt.number, attempt.state());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, is_current, &label)
    });
    let hover = row::hover_amount(ui, &response);
    let fill = if is_current { palette.selection } else { palette.hover.gamma_multiply(hover) };
    ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    let (icon, color) = status_icon(attempt.status, &palette);
    let left = rect.left() + style::SPACE_S;
    let title_y = rect.top() + OPTION_HEIGHT * 0.34;
    let detail_y = rect.top() + OPTION_HEIGHT * 0.70;
    ui.painter().text(
        pos2(left + 8.0, title_y),
        Align2::CENTER_CENTER,
        icon,
        icons::rest_font(&palette, icons::SIZE),
        color,
    );
    let text_left = left + 16.0 + style::SPACE_S;
    let title = format!("Attempt {}", attempt.number);
    let title_galley = ui.painter().layout_no_wrap(
        title,
        theme::fonts::semibold_font(style::BODY_SIZE),
        palette.text,
    );
    let title_width = title_galley.size().x;
    ui.painter().galley(
        pos2(text_left, title_y - title_galley.size().y / 2.0),
        title_galley,
        palette.text,
    );
    let small = FontId::proportional(style::SMALL_SIZE);
    if attempt.is_latest {
        ui.painter().text(
            pos2(text_left + title_width + style::SPACE_S, title_y),
            Align2::LEFT_CENTER,
            "Latest",
            small.clone(),
            palette.accent,
        );
    }
    let detail = detail_text(attempt);
    let duration = super::duration(attempt.started_at, attempt.finished_at);
    let duration_width = if duration.is_some() { 72.0 } else { 0.0 };
    let detail = elide::line(
        ui.painter(),
        &detail,
        small.clone(),
        palette.text_secondary,
        rect.right() - style::SPACE_S - duration_width - text_left,
    );
    ui.painter().galley(
        pos2(text_left, detail_y - detail.size().y / 2.0),
        detail,
        palette.text_secondary,
    );
    if let Some(duration) = duration {
        ui.painter().text(
            pos2(rect.right() - style::SPACE_S, title_y),
            Align2::RIGHT_CENTER,
            duration,
            small,
            palette.text_secondary,
        );
    }
    cv_widgets::focus::ring(ui, &response, rect);
    response.clicked()
}

#[cfg(test)]
mod tests {
    use review_core::AttemptSource;

    use super::*;

    fn node(status: RunStatus, attempt: u32, previous: Vec<PipelineAttempt>) -> PipelineJob {
        PipelineJob {
            id: format!("job-{attempt}"),
            name: "test".to_owned(),
            status,
            attempt,
            previous_attempts: previous,
            ..PipelineJob::default()
        }
    }

    fn loaded(status: RunStatus, number: u32) -> PipelineAttempt {
        PipelineAttempt {
            number,
            source: AttemptSource::Loaded(Box::new(node(status, number, Vec::new()))),
        }
    }

    #[test]
    fn lists_the_latest_attempt_first_and_knows_loaded_ones() {
        let lazy = PipelineAttempt { number: 1, source: AttemptSource::Lazy("t1".to_owned()) };
        let job = node(RunStatus::Succeeded, 3, vec![lazy, loaded(RunStatus::Failed, 2)]);
        let numbers: Vec<(u32, Option<RunStatus>, bool)> = list(&job, Known::default())
            .iter()
            .map(|a| (a.number, a.status, a.is_latest))
            .collect();
        assert_eq!(
            numbers,
            [
                (3, Some(RunStatus::Succeeded), true),
                (2, Some(RunStatus::Failed), false),
                (1, None, false)
            ]
        );
        let first = node(RunStatus::Failed, 1, Vec::new());
        let loaded_now = [("t1", &first)];
        let known = Known { loaded: &loaded_now };
        assert_eq!(
            list(&job, known)[2].status,
            Some(RunStatus::Failed),
            "a loaded attempt shows its outcome"
        );
        assert!(tooltip(&job, known).contains("Attempt 1: Failed"));
    }

    #[test]
    fn sums_up_a_retry_in_words() {
        assert_eq!(outcome(&node(RunStatus::Succeeded, 1, Vec::new())), None);
        assert_eq!(
            outcome(&node(RunStatus::Succeeded, 2, vec![loaded(RunStatus::Failed, 1)])).as_deref(),
            Some("passed on retry")
        );
        let failed = node(
            RunStatus::Failed,
            3,
            vec![loaded(RunStatus::Failed, 1), loaded(RunStatus::Failed, 2)],
        );
        assert_eq!(outcome(&failed).as_deref(), Some("failed after 3 attempts"));
    }

    #[test]
    fn a_stage_of_one_job_shows_that_jobs_attempts() {
        let job = node(RunStatus::Succeeded, 2, vec![loaded(RunStatus::Failed, 1)]);
        let stage = PipelineJob {
            id: "stage".to_owned(),
            children: vec![job.clone()],
            ..PipelineJob::default()
        };
        assert_eq!(retried(&stage).map(|node| node.id.as_str()), Some("job-2"));
        let two = PipelineJob { children: vec![job.clone(), job], ..stage };
        assert!(retried(&two).is_none(), "its jobs show their own badges");
    }

    #[test]
    fn an_attempt_not_loaded_says_when_it_loads() {
        let attempt = |status| Attempt {
            number: 1,
            status,
            started_at: None,
            finished_at: None,
            is_latest: false,
        };
        assert_eq!(detail_text(&attempt(None)), "Loads when you open it");
        assert_eq!(detail_text(&attempt(Some(RunStatus::Failed))), "Failed", "no start known");
    }
}
