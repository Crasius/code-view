//! One stage card of the graph: status, name, "N jobs completed" and duration; expanded it lists
//! the stage's jobs and offers the stage's reruns, as Azure DevOps does.

use cv_base::theme::{self, icons, motion, style};
use cv_widgets::{badges, elide, row};
use egui::{
    Align2, Color32, FontId, Id, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2,
};
use review_core::{PipelineJob, ProviderKind, RunStatus};

use super::super::attempts::{self, Known};
use super::super::job_tree::{self, LogTarget};
use super::super::rerun::{Picked, RerunButton, TOKEN_ROW_HEIGHT};

/// Padding 12 above and below, the name line and the summary line between.
pub const HEADER_HEIGHT: f32 = 68.0;
const HEADER_PADDING_Y: f32 = style::SPACE_M;
const JOB_ROW_HEIGHT: f32 = style::ROW_COMPACT;
const FOOTER_PADDING: f32 = style::SPACE_M;
const TOGGLE_SIZE: f32 = 28.0;
const SIDE: f32 = style::SPACE_ROW_X;

#[derive(Clone, Copy)]
pub struct CardContext<'a> {
    pub platform: ProviderKind,
    pub has_token: bool,
    pub is_busy: bool,
    /// All stages of the run, to name the jobs a job needs (they may sit in other stages).
    pub stages: &'a [PipelineJob],
    /// Earlier attempts loaded so far, for the outcome their badges show.
    pub known: Known<'a>,
}

pub enum CardAction {
    Toggle,
    /// The header was clicked: show the stage's log.
    OpenStage,
    Job(LogTarget),
    Rerun(review_core::RerunAction),
    AddToken,
    /// An attempt picked from a badge: the node (the stage, or a job) and the attempt's number.
    Attempt {
        node_id: String,
        number: u32,
    },
}

/// Height of the card fully expanded.
pub fn expanded_height(stage: &PipelineJob, context: &CardContext) -> f32 {
    let list = stage.children.len() as f32 * JOB_ROW_HEIGHT;
    let footer = if stage.reruns.is_empty() {
        0.0
    } else {
        let token_row = if context.has_token { 0.0 } else { TOKEN_ROW_HEIGHT };
        style::CONTROL_HEIGHT + FOOTER_PADDING * 2.0 + token_row
    };
    HEADER_HEIGHT + 1.0 + list + footer
}

/// `opening` is 0 when collapsed, 1 when expanded, in between while animating.
pub fn height(stage: &PipelineJob, context: &CardContext, opening: f32) -> f32 {
    HEADER_HEIGHT + (expanded_height(stage, context) - HEADER_HEIGHT) * opening
}

/// `visible` is the part of the canvas on screen; rows outside it are skipped.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    stage: &PipelineJob,
    opening: f32,
    is_highlighted: bool,
    visible: Rect,
    context: &CardContext,
) -> Option<CardAction> {
    let palette = theme::palette(ui.ctx());
    let id = ui.scope_id().with(("stage_card", &stage.id));
    let mut action = None;

    let header = ui.interact(header_rect(rect), id.with("header"), Sense::click());
    header.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Stage {}", stage.name))
    });
    let card = Card { rect, id, opening, hover: row::hover_amount(ui, &header) };
    let highlight = motion::toggle(ui.ctx(), id.with("highlight"), is_highlighted, motion::MEDIUM);
    let painter = ui.painter().with_clip_rect(visible);
    card.paint_frame(&painter, &palette, highlight);

    let (icon, color) = badges::run_status_icon(stage.status, &palette);
    let title_y = card.title_y();
    painter.text(
        pos2(rect.left() + SIDE, title_y),
        Align2::LEFT_CENTER,
        icon,
        icons::rest_font(&palette, icons::SIZE),
        color,
    );
    let name_left = rect.left() + SIDE + 26.0;
    let name_width = rect.right() - TOGGLE_SIZE - SIDE - 4.0 - name_left;
    let title_font = theme::fonts::semibold_font(style::BODY_SIZE);
    let title = elide::line(ui.painter(), &stage.name, title_font, palette.text, name_width);
    let is_cut = title.elided;
    painter.galley(pos2(name_left, title_y - title.size().y / 2.0), title, palette.text);
    if let Some(picked) = card.summary_line(ui, &painter, stage, context) {
        action = Some(picked);
    }
    cv_widgets::focus::ring(ui, &header, header_rect(rect));
    let header = if is_cut { header.on_hover_text(&stage.name) } else { header };
    if header.clicked() {
        action = Some(CardAction::OpenStage);
    }
    if card.toggle(ui, &painter, stage) {
        action = Some(CardAction::Toggle);
    }
    if opening <= 0.0 {
        return action;
    }
    if let Some(picked) = card.body(ui, &painter, stage, visible, context) {
        action = Some(picked);
    }
    action
}

/// The header strip at the top of a card at `card`.
fn header_rect(card: Rect) -> Rect {
    Rect::from_min_size(card.min, vec2(card.width(), HEADER_HEIGHT))
}

/// The card being drawn this frame.
#[derive(Clone, Copy)]
struct Card {
    rect: Rect,
    id: Id,
    /// How far the card is open, 0 to 1.
    opening: f32,
    /// How far the pointer hovers its header, 0 to 1.
    hover: f32,
}

impl Card {
    /// The middle of the name line.
    fn title_y(self) -> f32 {
        self.rect.top() + HEADER_PADDING_Y + 10.0
    }

    /// Shadow, fill, the header's hover, the border and the highlight ring (`highlight`, 0 to 1).
    fn paint_frame(self, painter: &egui::Painter, palette: &theme::Palette, highlight: f32) {
        let Self { rect, opening, hover, .. } = self;
        painter.add(style::card_shadow(palette).as_shape(rect, style::RADIUS_CARD));
        painter.rect_filled(rect, style::RADIUS_CARD, palette.surface);
        if hover > 0.0 {
            let top =
                egui::CornerRadius { nw: style::RADIUS_CARD, ne: style::RADIUS_CARD, sw: 0, se: 0 };
            let corners =
                if opening > 0.0 { top } else { egui::CornerRadius::same(style::RADIUS_CARD) };
            painter.rect_filled(header_rect(rect), corners, palette.hover.gamma_multiply(hover));
        }
        let border = motion::mix(palette.border, palette.border_strong, hover);
        painter.rect_stroke(rect, style::RADIUS_CARD, Stroke::new(1.0, border), StrokeKind::Inside);
        if highlight > 0.0 {
            painter.rect_stroke(
                rect,
                style::RADIUS_CARD,
                Stroke::new(2.0, palette.accent.gamma_multiply(highlight)),
                StrokeKind::Inside,
            );
        }
    }

    /// "N jobs completed", the attempt badge of a retried stage and the duration; returns the
    /// attempt picked from the badge.
    fn summary_line(
        self,
        ui: &mut Ui,
        painter: &egui::Painter,
        stage: &PipelineJob,
        context: &CardContext,
    ) -> Option<CardAction> {
        let rect = self.rect;
        let palette = theme::palette(ui.ctx());
        let summary_y = rect.top() + HEADER_HEIGHT - HEADER_PADDING_Y - 8.0;
        let small = FontId::proportional(style::SMALL_SIZE);
        let duration = super::super::duration(stage.started_at, stage.finished_at);
        let duration_width =
            duration.as_ref().map_or(0.0, |text| elide::text_width(ui, text, &small) + 8.0);
        let retried = attempts::retried(stage);
        let badge_width =
            retried.map_or(0.0, |node| attempts::badge_width(ui, node, false) + style::SPACE_S);
        let summary_width = rect.width() - SIDE * 2.0 - duration_width - badge_width;
        let summary = elide::line(
            ui.painter(),
            &summary(stage, context.platform),
            small.clone(),
            palette.text_secondary,
            summary_width,
        );
        painter.galley(
            pos2(rect.left() + SIDE, summary_y - summary.size().y / 2.0),
            summary,
            palette.text_secondary,
        );
        if let Some(duration) = duration {
            painter.text(
                pos2(rect.right() - SIDE, summary_y),
                Align2::RIGHT_CENTER,
                duration,
                small,
                palette.text_secondary,
            );
        }
        let node = retried?;
        let right = pos2(rect.right() - SIDE - duration_width, summary_y);
        let badge = attempts::Badge { node, is_compact: false, known: context.known, fade: 1.0 };
        let number = attempts::badge_button(ui, self.id.with("attempts"), painter, right, badge)?;
        Some(CardAction::Attempt { node_id: node.id.clone(), number })
    }

    /// The expand/collapse chevron at the header's right end; true when clicked.
    fn toggle(self, ui: &Ui, painter: &egui::Painter, stage: &PipelineJob) -> bool {
        let palette = theme::palette(ui.ctx());
        let toggle_rect = Rect::from_min_size(
            pos2(self.rect.right() - SIDE - TOGGLE_SIZE + 6.0, self.title_y() - TOGGLE_SIZE / 2.0),
            vec2(TOGGLE_SIZE, TOGGLE_SIZE),
        );
        let is_expanded = self.opening > 0.5;
        let toggle_label = if is_expanded { "Collapse stage" } else { "Expand stage" };
        let toggle = ui.interact(toggle_rect, self.id.with("toggle"), Sense::click());
        toggle.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::Button,
                true,
                is_expanded,
                format!("{toggle_label} {}", stage.name),
            )
        });
        let toggle_hover = row::hover_amount(ui, &toggle);
        if toggle_hover > 0.0 || toggle.is_pointer_button_down_on() {
            let fill = if toggle.is_pointer_button_down_on() {
                palette.pressed
            } else {
                palette.pressed.gamma_multiply(toggle_hover)
            };
            painter.rect_filled(toggle_rect, style::RADIUS_CONTROL, fill);
        }
        let chevron = if is_expanded { icons::CARET_UP } else { icons::CARET_DOWN };
        let chevron_color =
            motion::mix(palette.text_secondary, palette.text, toggle_hover.max(self.hover));
        painter.text(
            toggle_rect.center(),
            Align2::CENTER_CENTER,
            chevron,
            icons::rest_font(&palette, icons::SIZE),
            chevron_color,
        );
        cv_widgets::focus::ring(ui, &toggle, toggle_rect);
        toggle.on_hover_text(toggle_label).clicked()
    }

    /// The open card's job rows and its reruns, within `visible`.
    fn body(
        self,
        ui: &mut Ui,
        painter: &egui::Painter,
        stage: &PipelineJob,
        visible: Rect,
        context: &CardContext,
    ) -> Option<CardAction> {
        let Self { rect, opening, .. } = self;
        let palette = theme::palette(ui.ctx());
        let header_bottom = header_rect(rect).bottom();
        let body = Rect::from_min_max(pos2(rect.left(), header_bottom), rect.max);
        let clip = body.intersect(visible);
        painter.hline(rect.x_range().shrink(1.0), header_bottom, Stroke::new(1.0, palette.border));
        let mut action = None;
        let mut top = header_bottom + 1.0;
        let has_footer = !stage.reruns.is_empty();
        for (index, job) in stage.children.iter().enumerate() {
            let row_rect = Rect::from_min_size(
                pos2(rect.left() + 1.0, top),
                vec2(rect.width() - 2.0, JOB_ROW_HEIGHT),
            );
            top += JOB_ROW_HEIGHT;
            if !row_rect.intersects(clip) {
                continue;
            }
            if index > 0 {
                painter.hline(
                    row_rect.x_range(),
                    row_rect.top(),
                    Stroke::new(1.0, palette.border.gamma_multiply(opening)),
                );
            }
            let is_last = index + 1 == stage.children.len() && !has_footer;
            let slot = JobSlot { rect: row_rect, clip, opening, is_last };
            if let Some(picked) = job_row(ui, slot, job, stage, context) {
                action = Some(picked);
            }
        }
        if !has_footer {
            return action;
        }
        if !stage.children.is_empty() {
            painter.hline(
                rect.x_range().shrink(1.0),
                top,
                Stroke::new(1.0, palette.border.gamma_multiply(opening)),
            );
        }
        let footer = Rect::from_min_max(
            pos2(rect.left() + SIDE, top + FOOTER_PADDING),
            pos2(rect.right() - SIDE, rect.bottom()),
        );
        if footer.intersects(clip) && opening > 0.99 {
            let mut child =
                ui.new_child(UiBuilder::new().id_salt(self.id.with("footer")).max_rect(footer));
            child.set_clip_rect(clip.intersect(ui.clip_rect()));
            let button = RerunButton {
                reruns: &stage.reruns,
                platform: context.platform,
                has_token: context.has_token,
                is_busy: context.is_busy,
                is_full_width: true,
            };
            match button.show(&mut child) {
                Some(Picked::Rerun(rerun)) => action = Some(CardAction::Rerun(rerun)),
                Some(Picked::AddToken) => action = Some(CardAction::AddToken),
                None => {}
            }
        }
        action
    }
}

/// Where a job row sits in its card this frame.
#[derive(Clone, Copy)]
struct JobSlot {
    rect: Rect,
    /// What of the card is on screen.
    clip: Rect,
    /// How far the card is open, 0 to 1; the row fades in with it.
    opening: f32,
    /// The last row rounds the hover fill into the card's bottom corners.
    is_last: bool,
}

/// A retried job shows a compact attempt badge; the one job of a stage shows it on the card's
/// summary line instead.
fn job_row(
    ui: &mut Ui,
    slot: JobSlot,
    job: &PipelineJob,
    stage: &PipelineJob,
    context: &CardContext,
) -> Option<CardAction> {
    let JobSlot { rect, clip, opening, is_last } = slot;
    let palette = theme::palette(ui.ctx());
    let response = ui.interact(
        rect.intersect(clip),
        ui.scope_id().with(("stage_job", &stage.id, &job.id)),
        Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &job.name));
    let painter = ui.painter().with_clip_rect(clip);
    let fill = row::state_fill(ui, &response, Color32::TRANSPARENT);
    if fill != Color32::TRANSPARENT {
        let corner = style::RADIUS_CARD - 1;
        let radius = if is_last {
            egui::CornerRadius { nw: 0, ne: 0, sw: corner, se: corner }
        } else {
            egui::CornerRadius::ZERO
        };
        painter.rect_filled(rect, radius, fill);
    }
    let fade = opening.clamp(0.0, 1.0);
    let (icon, color) = badges::run_status_icon(job.status, &palette);
    let side = SIDE - 1.0;
    painter.text(
        pos2(rect.left() + side, rect.center().y),
        Align2::LEFT_CENTER,
        icon,
        icons::rest_font(&palette, icons::SIZE),
        color.gamma_multiply(fade),
    );
    let small = FontId::proportional(style::SMALL_SIZE);
    let duration = super::super::duration(job.started_at, job.finished_at);
    let duration_width =
        duration.as_ref().map_or(0.0, |text| elide::text_width(ui, text, &small) + style::SPACE_M);
    let has_badge = !job.previous_attempts.is_empty()
        && attempts::retried(stage).is_none_or(|node| node.id != job.id);
    let badge_width =
        if has_badge { attempts::badge_width(ui, job, true) + style::SPACE_S } else { 0.0 };
    let name_left = rect.left() + side + 15.0 + style::SPACE_S;
    let text_color =
        if job.status == RunStatus::Skipped { palette.text_secondary } else { palette.text };
    let name_width = rect.right() - side - duration_width - badge_width - name_left;
    let name =
        elide::line(ui.painter(), &job.name, FontId::proportional(13.0), text_color, name_width);
    let is_cut = name.elided;
    painter.galley(
        pos2(name_left, rect.center().y - name.size().y / 2.0),
        name,
        text_color.gamma_multiply(fade),
    );
    if let Some(duration) = duration {
        painter.text(
            pos2(rect.right() - side, rect.center().y),
            Align2::RIGHT_CENTER,
            duration,
            small,
            palette.text_secondary.gamma_multiply(fade),
        );
    }
    cv_widgets::focus::ring(ui, &response, rect);
    let clicked = job_tree::click_target(job, stage.log_id.as_deref(), stage.web_url.as_deref());
    let note = job_tree::no_log_hint(job, &clicked).or_else(|| is_cut.then(|| job.name.clone()));
    let hint: Vec<String> = note.into_iter().chain(waits_for(job, context.stages)).collect();
    let response = if hint.is_empty() { response } else { response.on_hover_text(hint.join("\n")) };
    let picked = response.clicked().then_some(CardAction::Job(clicked));
    if has_badge && opening > 0.99 {
        let right = pos2(rect.right() - side - duration_width, rect.center().y);
        let id = ui.scope_id().with(("stage_job_attempts", &stage.id, &job.id));
        if let Some(number) = attempts::badge_button(
            ui,
            id,
            &painter,
            right,
            attempts::Badge { node: job, is_compact: true, known: context.known, fade },
        ) {
            return Some(CardAction::Attempt { node_id: job.id.clone(), number });
        }
    }
    picked
}

/// "Waits for: a, b": the jobs this job needs, by name, wherever they sit in the run.
fn waits_for(job: &PipelineJob, stages: &[PipelineJob]) -> Option<String> {
    if job.depends_on.is_empty() {
        return None;
    }
    let jobs = || stages.iter().flat_map(|stage| &stage.children);
    let names: Vec<&str> = job
        .depends_on
        .iter()
        .filter_map(|id| jobs().find(|other| other.id == *id).map(|other| other.name.as_str()))
        .collect();
    (!names.is_empty()).then(|| format!("Waits for: {}", names.join(", ")))
}

/// "N jobs completed" and its variants, as the card's second line.
pub fn summary(stage: &PipelineJob, platform: ProviderKind) -> String {
    let total = stage.children.len();
    let finished = stage.children.iter().filter(|job| job.status.is_finished()).count();
    let failed = stage.children.iter().filter(|job| job.status == RunStatus::Failed).count();
    let jobs = |count: usize| if count == 1 { "job" } else { "jobs" };
    match stage.status {
        RunStatus::Skipped if platform == ProviderKind::AzureDevOps => {
            "Skipped due to conditions".to_owned()
        }
        RunStatus::Skipped => "Skipped".to_owned(),
        _ if total == 0 => badges::run_status_label(stage.status).to_owned(),
        RunStatus::Failed if failed > 0 => format!("{failed} of {total} {} failed", jobs(total)),
        RunStatus::Queued if finished == 0 => "Waiting to run".to_owned(),
        RunStatus::Running | RunStatus::Queued => {
            format!("{finished} of {total} {} completed", jobs(total))
        }
        RunStatus::Canceled => format!("Canceled, {finished} {} completed", jobs(finished)),
        _ => format!("{total} {} completed", jobs(total)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(status: RunStatus, jobs: &[RunStatus]) -> PipelineJob {
        PipelineJob {
            status,
            children: jobs
                .iter()
                .map(|status| PipelineJob { status: *status, ..PipelineJob::default() })
                .collect(),
            ..PipelineJob::default()
        }
    }

    #[test]
    fn summarizes_like_azure_devops() {
        use RunStatus::{Failed, Queued, Running, Skipped, Succeeded};
        let azure = ProviderKind::AzureDevOps;
        assert_eq!(summary(&stage(Succeeded, &[Succeeded]), azure), "1 job completed");
        assert_eq!(summary(&stage(Succeeded, &[Succeeded; 6]), azure), "6 jobs completed");
        assert_eq!(summary(&stage(Failed, &[Succeeded, Failed]), azure), "1 of 2 jobs failed");
        assert_eq!(
            summary(&stage(Running, &[Succeeded, Running, Queued]), azure),
            "1 of 3 jobs completed"
        );
        assert_eq!(summary(&stage(Queued, &[Queued]), azure), "Waiting to run");
        assert_eq!(summary(&stage(Skipped, &[]), azure), "Skipped due to conditions");
        assert_eq!(summary(&stage(Skipped, &[]), ProviderKind::GitLab), "Skipped");
    }
}
