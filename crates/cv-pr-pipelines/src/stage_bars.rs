//! A run's stages at a glance: one short bar per stage in its status color, each naming its stage
//! on hover, and "4/5" for how many succeeded, skipped stages left out. Status checks without
//! stages get one bar of their own status.

use cv_base::theme::{self, Palette, style};
use cv_widgets::{badges, elide, skeleton};
use egui::{Align2, Color32, FontId, Id, Rect, Sense, Ui, pos2, vec2};
use review_core::{CheckState, PipelineJob, RunStatus};

const BAR_WIDTH: f32 = 18.0;
const BAR_HEIGHT: f32 = 6.0;
const BAR_GAP: f32 = 4.0;
const LABEL_GAP: f32 = 6.0;

/// What is known of a run's stages.
#[derive(Clone, Copy)]
pub enum Stages<'a> {
    /// A status check: no stages, its own status stands in.
    Own(RunStatus),
    Loading,
    Ready(&'a [PipelineJob]),
    /// They could not be loaded; the run's own status stands in.
    Unavailable(RunStatus),
    /// An older run's, not asked for: they load when the run is opened. Its own status stands in.
    OnDemand(RunStatus),
    /// A job: one bar for its stage in the job's status, "manual" beside it while it waits for a
    /// person to start it.
    Job {
        status: RunStatus,
        is_manual: bool,
    },
}

/// The bar color of a status; skipped and neutral stages stay quiet.
pub fn color(status: RunStatus, palette: &Palette) -> Color32 {
    match status {
        RunStatus::Succeeded => palette.success,
        RunStatus::Failed => palette.error,
        RunStatus::Running | RunStatus::Queued => palette.accent,
        RunStatus::Canceled => palette.text_secondary,
        RunStatus::Skipped | RunStatus::Neutral => palette.text_disabled,
    }
}

/// Paints the bars left-aligned and vertically centered in `rect`; stages that do not fit are
/// summed up as "+N". `name` names the run (or the job's stage) in the single bar's tooltip.
pub fn show(ui: &Ui, rect: Rect, id: Id, name: &str, stages: Stages<'_>) {
    let palette = theme::palette(ui.ctx());
    let top = rect.center().y - BAR_HEIGHT / 2.0;
    let bar = |index: usize| {
        Rect::from_min_size(
            pos2(rect.left() + index as f32 * (BAR_WIDTH + BAR_GAP), top),
            vec2(BAR_WIDTH, BAR_HEIGHT),
        )
    };
    let small = FontId::proportional(style::SMALL_SIZE);
    match stages {
        Stages::Loading => {
            for index in 0..3 {
                skeleton::paint_bar(ui, bar(index));
            }
        }
        Stages::Own(status) | Stages::Unavailable(status) => {
            paint_bar(
                ui,
                bar(0),
                id.with(0),
                color(status, &palette),
                &format!("{name}: {}", badges::run_status_label(status)),
            );
        }
        Stages::OnDemand(status) => {
            let tooltip = format!(
                "{name}: {}. An older run; its stages load when you open it.",
                badges::run_status_label(status)
            );
            paint_bar(ui, bar(0), id.with(0), color(status, &palette), &tooltip);
        }
        Stages::Job { status, is_manual } => {
            let tooltip = if is_manual {
                format!("{name}: manual, not started")
            } else {
                format!("{name}: {}", badges::run_status_label(status))
            };
            paint_bar(ui, bar(0), id.with(0), color(status, &palette), &tooltip);
            if is_manual {
                ui.painter().text(
                    pos2(bar(0).right() + LABEL_GAP, rect.center().y),
                    Align2::LEFT_CENTER,
                    "manual",
                    small,
                    palette.text_secondary,
                );
            }
        }
        Stages::Ready(stages) => {
            let (succeeded, counted) = succeeded_of(stages);
            let label = (stages.len() > 1 && counted > 0).then(|| format!("{succeeded}/{counted}"));
            let label_width =
                label.as_ref().map_or(0.0, |text| elide::text_width(ui, text, &small) + LABEL_GAP);
            let room = ((rect.width() - label_width + BAR_GAP) / (BAR_WIDTH + BAR_GAP))
                .floor()
                .max(1.0) as usize;
            let shown = stages.len().min(room);
            for (index, stage) in stages.iter().take(shown).enumerate() {
                let tooltip = format!("{}: {}", stage.name, badges::run_status_label(stage.status));
                paint_bar(ui, bar(index), id.with(index), color(stage.status, &palette), &tooltip);
            }
            let hidden = stages.len() - shown;
            let label = match (label, hidden) {
                (Some(label), 0) => Some(label),
                (Some(label), hidden) => Some(format!("+{hidden}  {label}")),
                (None, _) => None,
            };
            if let Some(label) = label {
                let left = bar(shown.max(1) - 1).right() + LABEL_GAP;
                ui.painter().text(
                    pos2(left, rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    small,
                    palette.text_secondary,
                );
            }
        }
    }
}

/// Succeeded stages and the stages that count: skipped, canceled and neutral ones finished without
/// a result and count neither way, as a list row's checks do.
fn succeeded_of(stages: &[PipelineJob]) -> (usize, usize) {
    let counted =
        stages.iter().filter(|stage| CheckState::from(stage.status) != CheckState::Neutral);
    counted.fold((0, 0), |(succeeded, all), stage| {
        (succeeded + usize::from(stage.status == RunStatus::Succeeded), all + 1)
    })
}

fn paint_bar(ui: &Ui, rect: Rect, id: Id, color: Color32, tooltip: &str) {
    ui.painter().rect_filled(rect, BAR_HEIGHT / 2.0, color);
    // A taller hover target than the bar itself; it only senses hover, so the row's click stays.
    let target = rect.expand2(vec2(BAR_GAP / 2.0, 8.0));
    ui.interact(target, id, Sense::hover()).on_hover_text(tooltip);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_stages_count_neither_as_succeeded_nor_as_open() {
        let stage = |status| PipelineJob { status, ..PipelineJob::default() };
        let stages = [
            stage(RunStatus::Succeeded),
            stage(RunStatus::Skipped),
            stage(RunStatus::Running),
            stage(RunStatus::Neutral),
            stage(RunStatus::Canceled),
        ];
        assert_eq!(succeeded_of(&stages), (1, 2));
        assert_eq!(succeeded_of(&[stage(RunStatus::Skipped)]), (0, 0));
    }
}
