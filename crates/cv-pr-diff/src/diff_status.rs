//! A small pill floating over the bottom-right corner of the diff, only while there is something
//! to say: syntax coloring progress on large files, why a file stays plain, or why the full file
//! could not be loaded, each with the action that resolves it. Floating means it never shifts rows.

use egui::{Align2, Area, Frame, Id, Margin, Order, Rect, RichText, Ui, vec2};

use cv_base::theme::{self, icons};
use cv_code::contents::{Coloring, FileContents};
use cv_widgets::{buttons, glyph};

/// Files with fewer lines color fast enough that a progress note would only flicker.
const PROGRESS_MIN_LINES: usize = 20_000;

pub enum StatusAction {
    ColorAnyway,
    Retry,
}

pub fn show(ui: &Ui, viewport: Rect, contents: Option<&FileContents>) -> Option<StatusAction> {
    let contents = contents?;
    let status = if let Some(error) = &contents.error {
        Status::Failed(error)
    } else {
        match contents.coloring {
            Coloring::Running { done, total } if total >= PROGRESS_MIN_LINES => {
                Status::Coloring(done * 100 / total.max(1))
            }
            Coloring::Skipped { bytes } => Status::Skipped(bytes),
            Coloring::Waiting | Coloring::Running { .. } | Coloring::Done => return None,
        }
    };
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    Area::new(Id::unique("diff_status"))
        .order(Order::Middle)
        .fixed_pos(viewport.right_bottom() - vec2(24.0, 18.0))
        .pivot(Align2::RIGHT_BOTTOM)
        .constrain_to(viewport)
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(palette.surface)
                .stroke(egui::Stroke::new(1.0, palette.border))
                .corner_radius(16)
                .inner_margin(Margin::symmetric(14, 6))
                .shadow(ui.visuals().popup_shadow)
                .show(ui, |ui| {
                    ui.set_max_width(viewport.width() * 0.6);
                    ui.horizontal(|ui| match status {
                        Status::Coloring(percent) => {
                            ui.spinner();
                            ui.label(
                                RichText::new(format!("Coloring syntax… {percent}%"))
                                    .color(palette.text_secondary),
                            );
                        }
                        Status::Skipped(bytes) => {
                            glyph::decorative(
                                ui,
                                RichText::new(icons::INFO).color(palette.text_secondary),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "Syntax colors are off for this {} file",
                                    format_size(bytes)
                                ))
                                .color(palette.text_secondary),
                            );
                            if buttons::subtle(ui, icons::PAINT_BRUSH, "Color anyway").clicked() {
                                action = Some(StatusAction::ColorAnyway);
                            }
                        }
                        Status::Failed(error) => {
                            glyph::decorative(
                                ui,
                                RichText::new(icons::WARNING).color(palette.error),
                            );
                            ui.label(
                                RichText::new("Hidden lines and syntax colors are unavailable.")
                                    .color(palette.text_secondary),
                            )
                            .on_hover_text(error);
                            if buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Retry")
                                .on_hover_text(error)
                                .clicked()
                            {
                                action = Some(StatusAction::Retry);
                            }
                        }
                    });
                });
        });
    action
}

enum Status<'a> {
    Coloring(usize),
    Skipped(usize),
    Failed(&'a str),
}

fn format_size(bytes: usize) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    format!("{:.1} MB", bytes as f64 / MIB)
}
