//! What the Markdown and the rich editor share, so they look the same: the text area's height
//! and margin, and the formatting toolbar itself (its buttons, gaps, hairline dividers between
//! groups, which groups fit the row and the "More formatting" menu for the rest). Each editor
//! brings its own item tables and active state.

use cv_base::theme::{self, icons, motion, style};
use cv_widgets::{focus, glyph, row};
use egui::{Align2, Color32, Margin, Sense, Stroke, Ui, vec2};

/// The square icon buttons, also the mode switch's height.
pub const BUTTON_SIZE: f32 = 28.0;
pub const BUTTON_GAP: f32 = 2.0;
/// A divider's slot, its gaps to the buttons around it included.
pub const DIVIDER_WIDTH: f32 = 13.0;
/// The text area grows with its content up to this height, then scrolls.
pub const MAX_HEIGHT: f32 = 360.0;
/// One row of body text in the text area.
pub const ROW_HEIGHT: f32 = 22.0;
/// The inset of the text inside the text area's frame.
pub const TEXT_MARGIN: Margin = Margin::symmetric(12, 10);

/// One toolbar button: what it does, how it looks and the shortcut its tooltip names.
pub struct Item<A> {
    pub action: A,
    pub icon: &'static str,
    pub label: &'static str,
    pub shortcut: &'static str,
}

pub const fn item<A>(
    action: A,
    icon: &'static str,
    label: &'static str,
    shortcut: &'static str,
) -> Item<A> {
    Item { action, icon, label, shortcut }
}

/// Draws the groups right-aligned into the rest of the row; returns the clicked button's action.
pub fn show<A: Copy>(
    ui: &mut Ui,
    groups: &[Vec<&'static Item<A>>],
    is_active: impl Fn(A) -> bool,
    is_enabled: bool,
) -> Option<A> {
    let widths: Vec<f32> = groups.iter().map(|group| group_width(group.len())).collect();
    let shown = visible_groups(&widths, ui.available_width());
    let mut clicked = None;
    ui.add_enabled_ui(is_enabled, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = BUTTON_GAP;
            if shown < groups.len() {
                if let Some(action) = overflow(ui, &groups[shown..], &is_active) {
                    clicked = Some(action);
                }
                divider(ui);
            }
            for (index, group) in groups[..shown].iter().enumerate().rev() {
                for item in group.iter().rev() {
                    if button(ui, item, is_active(item.action)).clicked() {
                        clicked = Some(item.action);
                    }
                }
                if index > 0 {
                    divider(ui);
                }
            }
        });
    });
    clicked
}

/// The width of a group of `buttons` buttons.
fn group_width(buttons: usize) -> f32 {
    buttons as f32 * BUTTON_SIZE + buttons.saturating_sub(1) as f32 * BUTTON_GAP
}

/// How many groups fit into `width`, the rest going into the overflow menu.
fn visible_groups(group_widths: &[f32], width: f32) -> usize {
    let total = |count: usize, with_overflow: bool| {
        let groups: f32 = group_widths[..count].iter().sum();
        let dividers = count.saturating_sub(1) as f32 * DIVIDER_WIDTH;
        let overflow = if with_overflow { DIVIDER_WIDTH + BUTTON_SIZE } else { 0.0 };
        groups + dividers + overflow
    };
    if total(group_widths.len(), false) <= width {
        return group_widths.len();
    }
    (0..group_widths.len()).rev().find(|&count| total(count, true) <= width).unwrap_or(0)
}

/// The hairline between two groups.
fn divider(ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) =
        ui.allocate_exact_size(vec2(DIVIDER_WIDTH - 2.0 * BUTTON_GAP, BUTTON_SIZE), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.shrink2(vec2(0.0, 6.0)).y_range(),
        Stroke::new(1.0, palette.border),
    );
}

fn button<A>(ui: &mut Ui, item: &Item<A>, is_active: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(BUTTON_SIZE, BUTTON_SIZE), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), is_active, item.label)
    });
    let response = response.on_hover_text(format!("{} ({})", item.label, item.shortcut));
    if ui.is_rect_visible(rect) {
        let rest = if is_active { palette.selection } else { Color32::TRANSPARENT };
        let fill = if ui.is_enabled() {
            row::state_fill(ui, &response, rest)
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
        let color = if !ui.is_enabled() {
            palette.text_disabled
        } else if is_active {
            palette.accent_text
        } else {
            palette.text_secondary
        };
        let hover = row::hover_amount(ui, &response);
        let color = if is_active { color } else { motion::mix(color, palette.text, hover) };
        let font = icons::state_font(&palette, icons::SIZE, is_active);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, item.icon, font, color);
        focus::ring(ui, &response, rect);
    }
    response
}

fn overflow<A: Copy>(
    ui: &mut Ui,
    groups: &[Vec<&'static Item<A>>],
    is_active: impl Fn(A) -> bool,
) -> Option<A> {
    let label = "More formatting";
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(BUTTON_SIZE, BUTTON_SIZE), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    let response = response.on_hover_text(label);
    let fill = row::state_fill(ui, &response, Color32::TRANSPARENT);
    ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    let font = icons::rest_font(&palette, icons::SIZE);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        icons::DOTS_THREE,
        font,
        palette.text_secondary,
    );
    focus::ring(ui, &response, rect);
    let mut clicked = None;
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width(220.0);
        for (index, group) in groups.iter().enumerate() {
            if index > 0 {
                ui.separator();
            }
            for item in group {
                let entry = egui::Button::selectable(
                    is_active(item.action),
                    format!("{}   {}", item.icon, item.label),
                )
                .shortcut_text(egui::RichText::new(item.shortcut).color(palette.text_secondary))
                .min_size(vec2(ui.available_width(), BUTTON_SIZE));
                let entry = ui.add(entry);
                glyph::speak_as(&entry, item.label);
                if entry.clicked() {
                    clicked = Some(item.action);
                }
            }
        }
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::visible_groups;

    #[test]
    fn groups_overflow_from_the_right() {
        let widths = [118.0, 58.0, 88.0, 88.0, 28.0];
        let all: f32 = widths.iter().sum::<f32>() + 4.0 * 13.0;
        assert_eq!(visible_groups(&widths, all), 5);
        // The overflow button needs room too, so one pixel short drops two groups.
        assert_eq!(visible_groups(&widths, all - 1.0), 3);
        assert_eq!(visible_groups(&widths, 130.0), 0);
        assert_eq!(visible_groups(&widths, 170.0), 1);
    }
}
