//! Empty, loading and error states. A state that tells the user to go somewhere or do something
//! carries that action as a button right there, so no message is a dead end.

use cv_base::theme::{self, icons, style};
use egui::{Align2, RichText, Ui, vec2};
use review_core::ProviderKind;

use super::{buttons, glyph};

const DETAIL_WIDTH: f32 = 480.0;
const BADGE_SIZE: f32 = 64.0;

/// A button offered by an empty or error state. The view acts on the returned index.
#[derive(Clone, Copy)]
pub struct Action<'a> {
    pub icon: &'a str,
    pub label: &'a str,
    pub is_primary: bool,
}

impl<'a> Action<'a> {
    pub fn primary(icon: &'a str, label: &'a str) -> Self {
        Self { icon, label, is_primary: true }
    }

    pub fn secondary(icon: &'a str, label: &'a str) -> Self {
        Self { icon, label, is_primary: false }
    }
}

/// "Open on GitHub", "Open in Azure DevOps": the platform named, never just "the platform".
pub fn open_on_platform(kind: ProviderKind) -> String {
    format!("Open {}", on_platform(kind))
}

/// "on GitHub", "in Azure DevOps".
pub fn on_platform(kind: ProviderKind) -> String {
    match kind {
        ProviderKind::AzureDevOps => format!("in {}", kind.label()),
        ProviderKind::GitHub | ProviderKind::GitLab => format!("on {}", kind.label()),
    }
}

/// Centered message with an icon for empty and error states.
pub fn message(ui: &mut Ui, icon: &str, title: &str, detail: &str) {
    message_with_actions(ui, icon, title, detail, &[]);
}

/// Centered message with buttons below it; returns the index of the clicked action.
pub fn message_with_actions(
    ui: &mut Ui,
    icon: &str,
    title: &str,
    detail: &str,
    actions: &[Action<'_>],
) -> Option<usize> {
    let palette = theme::palette(ui.ctx());
    let mut clicked = None;
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        let (badge, _) = ui.allocate_exact_size(vec2(BADGE_SIZE, BADGE_SIZE), egui::Sense::hover());
        ui.painter().circle_filled(badge.center(), BADGE_SIZE / 2.0, palette.surface_subtle);
        ui.painter().text(
            badge.center(),
            Align2::CENTER_CENTER,
            icon,
            icons::rest_font(&palette, 28.0),
            palette.text_secondary,
        );
        ui.add_space(style::SPACE_M);
        ui.label(RichText::new(title).font(theme::fonts::semibold_font(style::SECTION_SIZE)));
        if !detail.is_empty() {
            ui.add_space(style::SPACE_XS);
            let width = DETAIL_WIDTH.min(ui.available_width());
            ui.allocate_ui(vec2(width, 0.0), |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(detail).color(palette.text_secondary))
                });
            });
        }
        if !actions.is_empty() {
            ui.add_space(style::SPACE_L);
            clicked = centered_actions(ui, actions);
        }
    });
    clicked
}

pub fn loading(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(RichText::new(text).color(theme::palette(ui.ctx()).text_secondary));
    });
}

/// Inline error bar.
pub fn error(ui: &mut Ui, text: &str) {
    error_with_actions(ui, text, &[]);
}

/// Inline error bar with the ways out of it (retry, open on the platform, fix the token);
/// returns the index of the clicked action.
pub fn error_with_actions(ui: &mut Ui, text: &str, actions: &[Action<'_>]) -> Option<usize> {
    let color = theme::palette(ui.ctx()).error;
    bar(ui, color, text, actions)
}

/// Inline warning bar, like [`error_with_actions`] for things that block but did not fail.
pub fn warning_with_actions(ui: &mut Ui, text: &str, actions: &[Action<'_>]) -> Option<usize> {
    let color = theme::palette(ui.ctx()).warning;
    bar(ui, color, text, actions)
}

/// A "Details" toggle under an error, closed at first: the platform's raw answer for those who
/// need it, with a button to copy it. Never the message itself.
pub fn details(ui: &mut Ui, id_salt: &str, details: &str) {
    let palette = theme::palette(ui.ctx());
    egui::CollapsingHeader::new(
        RichText::new("Details").size(style::SMALL_SIZE).color(palette.text_secondary),
    )
    .id_salt(id_salt)
    .default_open(false)
    .show(ui, |ui| {
        ui.add(
            egui::Label::new(
                RichText::new(details)
                    .monospace()
                    .size(style::SMALL_SIZE)
                    .color(palette.text_secondary),
            )
            .wrap(),
        );
        if buttons::subtle(ui, icons::COPY, "Copy details").clicked() {
            ui.ctx().copy_text(details.to_owned());
        }
    });
}

fn bar(ui: &mut Ui, tone: egui::Color32, text: &str, actions: &[Action<'_>]) -> Option<usize> {
    let palette = theme::palette(ui.ctx());
    let mut clicked = None;
    egui::Frame::new()
        .fill(tone.gamma_multiply(if palette.is_dark { 0.14 } else { 0.07 }))
        .stroke(egui::Stroke::new(1.0, tone.gamma_multiply(0.35)))
        .corner_radius(style::RADIUS_CARD)
        .inner_margin(egui::Margin::symmetric(style::SPACE_M as i8, style::SPACE_S as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                glyph::decorative(ui, RichText::new(icons::WARNING).size(16.0).color(tone));
                ui.vertical(|ui| {
                    ui.label(RichText::new(text).color(palette.text));
                    if !actions.is_empty() {
                        ui.add_space(style::SPACE_XS);
                        ui.horizontal_wrapped(|ui| clicked = action_buttons(ui, actions));
                    }
                });
            });
        });
    clicked
}

fn centered_actions(ui: &mut Ui, actions: &[Action<'_>]) -> Option<usize> {
    let gap = style::SPACE_S;
    let width: f32 =
        actions.iter().map(|action| buttons::icon_button_width(ui, action.label)).sum::<f32>()
            + gap * (actions.len() - 1) as f32;
    ui.allocate_ui_with_layout(
        vec2(width, style::CONTROL_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = gap;
            action_buttons(ui, actions)
        },
    )
    .inner
}

fn action_buttons(ui: &mut Ui, actions: &[Action<'_>]) -> Option<usize> {
    let mut clicked = None;
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
    clicked
}
