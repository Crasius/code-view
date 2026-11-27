//! What the kittest scenes of this crate share: the light theme with the app's fonts, a long
//! document of numbered lines, and that document in a scroll area with the lightbox over it.

use cv_base::theme;
use egui::Ui;
use review_store::ThemePreference;

use super::RenderOptions;

/// The app's fonts and its light theme, as a harness's context set-up.
pub(super) fn light(ctx: &egui::Context) {
    theme::fonts::install(ctx);
    theme::activate(ctx, ThemePreference::Light);
}

/// `count` paragraphs "Line <n> of the document.", from 0.
pub(super) fn lines(count: usize) -> String {
    (0..count)
        .map(|index| format!("Line {index} of the document."))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `text` rendered in a vertical scroll area, scrolled to `offset` when it is set (once: it is
/// taken), with the lightbox over it.
pub(super) fn scrolled(ui: &mut Ui, offset: &mut Option<f32>, text: &str, options: RenderOptions) {
    let mut area = egui::ScrollArea::vertical().id_salt("document").auto_shrink(false);
    if let Some(offset) = offset.take() {
        area = area.vertical_scroll_offset(offset);
    }
    area.show(ui, |ui| super::render(ui, "doc", text, options));
    super::show_lightbox(ui.ctx());
}
