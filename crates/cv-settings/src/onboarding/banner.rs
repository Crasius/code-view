//! A strip above every screen while a connection's token was refused: what is missing, and the
//! ways out (fix it in the wizard, open the token page or the problem's own pages such as
//! "Authorize for acme", dismiss).

use cv_base::theme::{self, icons};
use cv_services::Services;
use cv_widgets::{buttons, glyph, provider_icon};
use egui::{Align, Frame, Layout, Margin, RichText, Stroke, Ui};

use super::guides;

/// Most problems shown at once; the rest follow when these are fixed or dismissed.
const MAX_SHOWN: usize = 2;
/// Most action buttons per problem ("Authorize for …" per organization); the title names them all.
const MAX_ACTIONS: usize = 3;

pub fn show(ui: &mut Ui, services: &mut Services) {
    let issues = services.providers.issues().all();
    if issues.is_empty() {
        return;
    }
    let palette = theme::palette(ui.ctx());
    let mut dismissed = Vec::new();
    for (connection_id, problem) in issues.into_iter().take(MAX_SHOWN) {
        let Some(connection) = services.connection(&connection_id).cloned() else {
            dismissed.push(connection_id);
            continue;
        };
        Frame::new()
            .fill(palette.warning_surface)
            .stroke(Stroke::new(1.0, palette.warning.gamma_multiply(0.4)))
            .inner_margin(Margin::symmetric(16, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    glyph::decorative(
                        ui,
                        RichText::new(icons::WARNING).size(18.0).color(palette.warning),
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.horizontal(|ui| {
                            provider_icon::show(
                                ui,
                                connection.kind,
                                connection.base_url(),
                                icons::SIZE,
                            );
                            ui.label(
                                RichText::new(format!("{}: {}", connection.name, problem.title))
                                    .font(theme::fonts::semibold_font(theme::style::BODY_SIZE))
                                    .color(palette.text),
                            );
                        });
                        ui.label(
                            RichText::new(&problem.fix)
                                .size(theme::style::SMALL_SIZE)
                                .color(palette.text),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if buttons::icon(ui, icons::X, "Dismiss until it happens again").clicked() {
                            dismissed.push(connection_id.clone());
                        }
                        // The problem's own pages fix it directly; otherwise the token page does.
                        if problem.actions.is_empty() {
                            let token_page = guides::token_page(&connection);
                            open_button(ui, "Open token page", &token_page);
                        }
                        for action in problem.actions.iter().take(MAX_ACTIONS).rev() {
                            open_button(ui, &action.label, &action.url);
                        }
                        super::fix_token_button(ui, &connection_id);
                    });
                });
            });
    }
    for connection_id in dismissed {
        services.providers.issues().clear(&connection_id);
    }
}

fn open_button(ui: &mut Ui, label: &str, url: &str) {
    if buttons::secondary_icon(ui, icons::ARROW_SQUARE_OUT, label).on_hover_text(url).clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}
