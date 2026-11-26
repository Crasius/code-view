//! Centered empty and error states of the Files tab. Each one carries the action that resolves it,
//! so the user never has to work out where to go next.

use egui::{Align, Color32, Layout, RichText, Ui};

use cv_base::theme::{self, icons};
use cv_widgets::{buttons, glyph};

pub struct Action<'a> {
    pub icon: &'a str,
    pub label: &'a str,
    pub is_primary: bool,
}

/// Returns the index of the clicked action.
pub fn show(
    ui: &mut Ui,
    icon: &str,
    icon_color: Color32,
    title: &str,
    detail: &str,
    actions: &[Action<'_>],
) -> Option<usize> {
    let palette = theme::palette(ui.ctx());
    let mut clicked = None;
    ui.vertical_centered(|ui| {
        ui.add_space(56.0);
        glyph::decorative(
            ui,
            RichText::new(icon).font(icons::rest_font(&palette, 34.0)).color(icon_color),
        );
        ui.add_space(10.0);
        ui.label(
            RichText::new(title).font(theme::fonts::semibold_font(theme::style::SECTION_SIZE)),
        );
        if !detail.is_empty() {
            ui.add_space(4.0);
            ui.scope(|ui| {
                ui.set_max_width(560.0);
                ui.label(RichText::new(detail).color(palette.text_secondary));
            });
        }
        if actions.is_empty() {
            return;
        }
        ui.add_space(16.0);
        let width = row_width(ui, actions);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 32.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                // A horizontal layout does not center its items, so the row starts at half the free space.
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                for (index, action) in actions.iter().enumerate() {
                    let response = if action.is_primary {
                        buttons::primary_icon(ui, action.icon, action.label)
                    } else {
                        buttons::secondary_icon(ui, action.icon, action.label)
                    };
                    if response.clicked() {
                        clicked = Some(index);
                    }
                }
            },
        );
    });
    clicked
}

/// Width of the row of action buttons, gaps included.
fn row_width(ui: &Ui, actions: &[Action<'_>]) -> f32 {
    let buttons: f32 =
        actions.iter().map(|action| buttons::icon_button_width(ui, action.label)).sum();
    buttons + ui.spacing().item_spacing.x * actions.len().saturating_sub(1) as f32
}
