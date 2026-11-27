//! A running checkout: the running step and the time so far, one bar segment per step, the steps
//! with their durations, the live log (folded until asked for) and Cancel. Closing the popover
//! leaves the checkout running; the title bar shows it until it is done.

use std::time::Instant;

use egui::{Align2, RichText, Sense, Ui, pos2, vec2};

use super::Act;
use cv_base::theme::{self, icons, motion, style};
use cv_services::checkouts::{Job, JobProgress, StepState};
use cv_widgets::buttons;
use cv_widgets::progress_log::{self, format_duration};

const SEGMENT_HEIGHT: f32 = 4.0;
const SEGMENT_GAP: f32 = 4.0;
const STEP_HEIGHT: f32 = 24.0;
const LOG_HEIGHT: f32 = 180.0;

pub(super) fn running(ui: &mut Ui, job: &Job, is_log_open: &mut bool) -> Option<Act> {
    let progress = job.progress();
    let now = Instant::now();
    let palette = theme::palette(ui.ctx());
    let title = progress.current().map_or("Starting", |row| row.label.as_str());
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(theme::fonts::semibold_font(style::SECTION_SIZE)));
        ui.label(
            RichText::new(format_duration(progress.elapsed(now))).color(palette.text_secondary),
        );
    });
    segments(ui, &progress);
    steps(ui, &progress, now);
    let mut act = None;
    ui.horizontal(|ui| {
        let caret = if *is_log_open { icons::CARET_DOWN } else { icons::CARET_RIGHT };
        if buttons::subtle(ui, caret, "Log").clicked() {
            *is_log_open = !*is_log_open;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            progress_log::copy_button(ui, &progress.log);
        });
    });
    if *is_log_open {
        let panel = progress_log::Panel {
            id_salt: "checkout_log",
            name: "Checkout log",
            min_height: LOG_HEIGHT,
            max_height: LOG_HEIGHT,
            is_live: true,
        };
        progress_log::show(ui, &progress.log, &panel);
    }
    ui.label(
        RichText::new("You can close this. It keeps running and the title bar shows it.")
            .size(style::SMALL_SIZE)
            .color(palette.text_secondary),
    );
    if buttons::secondary(ui, "Cancel").clicked() {
        act = Some(Act::CancelJob);
    }
    act
}

/// One segment per step: done, running (or failed) and waiting.
fn segments(ui: &mut Ui, progress: &JobProgress) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), SEGMENT_HEIGHT), Sense::hover());
    let count = progress.steps.len().max(1) as f32;
    let width = ((rect.width() - SEGMENT_GAP * (count - 1.0)) / count).max(0.0);
    for (index, row) in progress.steps.iter().enumerate() {
        let color = match row.state {
            StepState::Done => palette.success,
            StepState::Running => palette.accent,
            StepState::Failed => palette.error,
            StepState::Waiting => palette.border_strong,
        };
        let left = rect.left() + index as f32 * (width + SEGMENT_GAP);
        let segment =
            egui::Rect::from_min_size(pos2(left, rect.top()), vec2(width, SEGMENT_HEIGHT));
        ui.painter().rect_filled(segment, SEGMENT_HEIGHT / 2.0, color);
    }
}

/// The steps: their state's icon, label and duration (the running one's so far).
pub(super) fn steps(ui: &mut Ui, progress: &JobProgress, now: Instant) {
    let palette = theme::palette(ui.ctx());
    let reduced = motion::is_reduced(ui.ctx());
    for row in &progress.steps {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), STEP_HEIGHT), Sense::hover());
        let slot = egui::Rect::from_center_size(
            pos2(rect.left() + icons::SIZE / 2.0, rect.center().y),
            vec2(icons::SIZE, icons::SIZE),
        );
        let (icon, color, state) = match row.state {
            StepState::Done => (Some(icons::CHECK), palette.success, "done"),
            StepState::Failed => (Some(icons::X), palette.error, "failed"),
            StepState::Waiting => (Some(icons::CIRCLE_DASHED), palette.text_disabled, "waiting"),
            StepState::Running if reduced => (Some(icons::CIRCLE_NOTCH), palette.accent, "running"),
            StepState::Running => (None, palette.accent, "running"),
        };
        match icon {
            Some(icon) => {
                ui.painter().text(
                    slot.center(),
                    Align2::CENTER_CENTER,
                    icon,
                    icons::rest_font(&palette, icons::SIZE),
                    color,
                );
            }
            None => egui::Spinner::new().size(icons::SIZE - 2.0).color(color).paint_at(ui, slot),
        }
        let text_color = match row.state {
            StepState::Waiting => palette.text_secondary,
            StepState::Failed => palette.error,
            StepState::Running | StepState::Done => palette.text,
        };
        ui.painter().text(
            pos2(slot.right() + style::SPACE_S, rect.center().y),
            Align2::LEFT_CENTER,
            &row.label,
            egui::FontId::proportional(style::BODY_SIZE),
            text_color,
        );
        let took = match row.state {
            StepState::Running => Some(progress.step_elapsed(now)),
            _ => row.took,
        };
        let took_text = took.map(format_duration).unwrap_or_default();
        if !took_text.is_empty() {
            ui.painter().text(
                pos2(rect.right(), rect.center().y),
                Align2::RIGHT_CENTER,
                &took_text,
                egui::FontId::proportional(style::SMALL_SIZE),
                palette.text_secondary,
            );
        }
        let description = format!("{}, {state} {took_text}", row.label);
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ListItem, true, &description));
    }
}
