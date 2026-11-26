//! The note above the Files tab while it shows one of the comparisons a live update offers
//! ("what was pushed since you looked", "the version before the push"), with the way back.

use egui::{RichText, Ui};

use cv_base::theme::{self, icons, style};
use cv_pr_session::session::CompareRange;
use cv_widgets::buttons;

/// Returns true when the user asked to see all changes again.
pub fn show(ui: &mut Ui, range: &CompareRange) -> bool {
    let text = match range {
        CompareRange::SinceSeen { seen, head } => format!(
            "Showing what was pushed since you last looked: {} to {}",
            short(seen),
            short(head)
        ),
        CompareRange::Previous { head, .. } => {
            format!("Showing the version before the latest push, up to {}", short(head))
        }
        CompareRange::AllChanges | CompareRange::Commit { .. } => return false,
    };
    let palette = theme::palette(ui.ctx());
    let mut back = false;
    egui::Frame::new()
        .fill(palette.surface_subtle)
        .inner_margin(egui::Margin::symmetric(16, 2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = style::SPACE_S;
                super::banner::glyph(ui, icons::INFO, palette.accent_text);
                ui.label(RichText::new(text).color(palette.text));
                back = buttons::subtle(ui, icons::GIT_DIFF, "Show all changes").clicked();
            });
        });
    back
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}
