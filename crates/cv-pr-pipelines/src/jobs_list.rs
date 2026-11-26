//! The "Jobs" pivot: every job of the run in one flat list with its stage and duration.

use cv_base::theme::{self, icons, style};
use cv_widgets::{badges, buttons, row};
use egui::{Align2, FontId, Rect, Sense, Ui, pos2, vec2};
use review_core::{PipelineJob, ProviderKind, RerunAction, RunStatus};

use super::attempts::{self, Known};
use super::graph::stage_card;
use super::job_tree::{self, LogTarget};
use super::rerun;

const ROW_HEIGHT: f32 = 40.0;
const HEADER_HEIGHT: f32 = 32.0;
const RERUN_WIDTH: f32 = 36.0;

pub enum JobsAction {
    Job(LogTarget),
    Rerun(RerunAction),
    AddToken,
    Attempt { node_id: String, number: u32 },
}

struct Row<'a> {
    job: &'a PipelineJob,
    stage: &'a PipelineJob,
}

pub fn show(
    ui: &mut Ui,
    stages: &[PipelineJob],
    platform: ProviderKind,
    has_token: bool,
    is_busy: bool,
    known: Known<'_>,
) -> Option<JobsAction> {
    let palette = theme::palette(ui.ctx());
    let rows: Vec<Row<'_>> = stages
        .iter()
        .flat_map(|stage| {
            let jobs: Vec<&PipelineJob> = if stage.children.is_empty() {
                vec![stage]
            } else {
                stage.children.iter().collect()
            };
            jobs.into_iter().map(move |job| Row { job, stage })
        })
        .collect();
    let mut action = None;
    let can_rerun_any = rows.iter().any(|row| !row.job.reruns.is_empty());
    if can_rerun_any && !has_token {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let add = buttons::subtle(ui, icons::KEY, "Add a token to rerun jobs")
                .on_hover_text(rerun::NEEDS_TOKEN);
            if add.clicked() {
                action = Some(JobsAction::AddToken);
            }
        });
    }
    let width = ui.available_width();
    let stage_x = (width * 0.55).max(220.0);
    let (header, _) = ui.allocate_exact_size(vec2(width, HEADER_HEIGHT), Sense::hover());
    let small = FontId::proportional(style::SMALL_SIZE);
    let painter = ui.painter_at(header);
    painter.text(
        pos2(header.left() + 44.0, header.center().y),
        Align2::LEFT_CENTER,
        "Job",
        small.clone(),
        palette.text_secondary,
    );
    painter.text(
        pos2(header.left() + stage_x, header.center().y),
        Align2::LEFT_CENTER,
        "Stage",
        small.clone(),
        palette.text_secondary,
    );
    painter.text(
        pos2(header.right() - RERUN_WIDTH - 12.0, header.center().y),
        Align2::RIGHT_CENTER,
        "Duration",
        small,
        palette.text_secondary,
    );
    painter.hline(header.x_range(), header.bottom() - 0.5, egui::Stroke::new(1.0, palette.border));
    let context = RowContext { stage_x, platform, has_token, is_busy, known };
    egui::ScrollArea::vertical().id_salt("jobs_list").auto_shrink(false).show_rows(
        ui,
        ROW_HEIGHT,
        rows.len(),
        |ui, range| {
            for row in &rows[range] {
                if let Some(picked) = job_row(ui, row, context) {
                    action = Some(picked);
                }
            }
        },
    );
    action
}

/// What every row of the list shares.
#[derive(Clone, Copy)]
struct RowContext<'a> {
    /// Where the stage column starts, from the row's left.
    stage_x: f32,
    platform: ProviderKind,
    has_token: bool,
    is_busy: bool,
    known: Known<'a>,
}

fn job_row(ui: &mut Ui, row: &Row<'_>, context: RowContext<'_>) -> Option<JobsAction> {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::ListItem, true, format!("Job {}", row.job.name))
    });
    row::list_background(ui, &response, rect, false);
    let painter = ui.painter_at(rect);
    painter.hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0, palette.border));
    let (icon, color) = badges::run_status_icon(row.job.status, &palette);
    painter.text(
        pos2(rect.left() + 16.0, rect.center().y),
        Align2::LEFT_CENTER,
        icon,
        FontId::proportional(16.0),
        color,
    );
    let text_color =
        if row.job.status == RunStatus::Skipped { palette.text_secondary } else { palette.text };
    let name_left = rect.left() + 44.0;
    let (name, is_cut) = stage_card::ellipsized(
        ui,
        &row.job.name,
        FontId::proportional(style::BODY_SIZE),
        text_color,
        rect.left() + context.stage_x - 16.0 - name_left,
    );
    painter.galley(pos2(name_left, rect.center().y - name.size().y / 2.0), name, text_color);
    let duration_right = rect.right() - RERUN_WIDTH - 12.0;
    let has_badge = !row.job.previous_attempts.is_empty();
    let badge_width =
        if has_badge { attempts::badge_width(ui, row.job, false) + style::SPACE_M } else { 0.0 };
    let stage_width = duration_right - 90.0 - badge_width - (rect.left() + context.stage_x);
    paint_stage_and_duration(ui, &painter, row, rect, context.stage_x, stage_width);
    cv_widgets::focus::ring(ui, &response, rect);
    let parent_log =
        if std::ptr::eq(row.job, row.stage) { None } else { row.stage.log_id.as_deref() };
    let clicked = job_tree::click_target(row.job, parent_log, row.stage.web_url.as_deref());
    let mut action = None;
    if has_badge {
        let right = pos2(duration_right - 90.0, rect.center().y);
        let id = ui.scope_id().with(("jobs_list_attempts", &row.job.id));
        let badge =
            attempts::Badge { node: row.job, is_compact: false, known: context.known, fade: 1.0 };
        if let Some(number) = attempts::badge_button(ui, id, &painter, right, badge) {
            action = Some(JobsAction::Attempt { node_id: row.job.id.clone(), number });
        }
    }
    if let Some(rerun) = rerun_button(ui, row, rect, context) {
        action = Some(rerun);
    }
    let response = match (job_tree::no_log_hint(row.job, &clicked), is_cut) {
        (Some(hint), _) => response.on_hover_text(hint),
        (None, true) => response.on_hover_text(&row.job.name),
        (None, false) => response,
    };
    if response.clicked() && action.is_none() {
        action = Some(JobsAction::Job(clicked));
    }
    action
}

/// The job's stage (none for a stage without jobs, shown as its own job), `stage_width` wide, and
/// its duration before the rerun column.
fn paint_stage_and_duration(
    ui: &Ui,
    painter: &egui::Painter,
    row: &Row<'_>,
    rect: Rect,
    stage_x: f32,
    stage_width: f32,
) {
    let palette = theme::palette(ui.ctx());
    if !std::ptr::eq(row.job, row.stage) {
        let body = FontId::proportional(style::BODY_SIZE);
        let (stage, _) =
            stage_card::ellipsized(ui, &row.stage.name, body, palette.text_secondary, stage_width);
        painter.galley(
            pos2(rect.left() + stage_x, rect.center().y - stage.size().y / 2.0),
            stage,
            palette.text_secondary,
        );
    }
    if let Some(duration) = super::duration(row.job.started_at, row.job.finished_at) {
        painter.text(
            pos2(rect.right() - RERUN_WIDTH - 12.0, rect.center().y),
            Align2::RIGHT_CENTER,
            duration,
            FontId::proportional(style::SMALL_SIZE),
            palette.text_secondary,
        );
    }
}

/// The job's first rerun as an icon button at the row's right end, disabled without a token or
/// while another rerun starts.
fn rerun_button(
    ui: &mut Ui,
    row: &Row<'_>,
    rect: Rect,
    context: RowContext<'_>,
) -> Option<JobsAction> {
    let rerun_action = row.job.reruns.first()?;
    let button_rect = Rect::from_center_size(
        pos2(rect.right() - RERUN_WIDTH / 2.0 - 4.0, rect.center().y),
        vec2(28.0, 28.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(button_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let label = rerun::label(rerun_action.kind, context.platform);
    let tooltip = if context.has_token {
        format!("{label}: {}", row.job.name)
    } else {
        rerun::NEEDS_TOKEN.to_owned()
    };
    let button = child
        .add_enabled_ui(context.has_token && !context.is_busy, |ui| {
            buttons::icon(ui, icons::ARROW_CLOCKWISE, &tooltip)
        })
        .inner
        .on_disabled_hover_text(&tooltip);
    button.clicked().then(|| JobsAction::Rerun(rerun_action.clone()))
}
