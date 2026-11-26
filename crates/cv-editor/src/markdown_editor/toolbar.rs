//! The editor's header: the Rich text / Markdown / Preview switch and the Markdown formatting
//! toolbar. Buttons are 28 px icon targets in groups (text style, code, link and quote, lists,
//! suggestion) separated by hairlines (`toolbar_layout`, shared with the rich editor's toolbar).
//! Groups that do not fit move, from the right, into a "More formatting" menu.

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, motion, style};
use cv_widgets::{focus, glyph, row};
use egui::{Align2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::formatting::Format;
use super::toolbar_layout::{BUTTON_GAP, BUTTON_SIZE, divider, group_width, visible_groups};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Format(Format),
    Suggestion,
    Mention,
}

pub struct Item {
    pub action: Action,
    pub icon: &'static str,
    pub label: &'static str,
    pub shortcut: &'static str,
}

const fn item(
    action: Action,
    icon: &'static str,
    label: &'static str,
    shortcut: &'static str,
) -> Item {
    Item { action, icon, label, shortcut }
}

const TEXT_STYLE: &[Item] = &[
    item(Action::Format(Format::Bold), icons::TEXT_B, "Bold", shortcut!(command "B")),
    item(Action::Format(Format::Italic), icons::TEXT_ITALIC, "Italic", shortcut!(command "I")),
    item(
        Action::Format(Format::Strikethrough),
        icons::TEXT_STRIKETHROUGH,
        "Strikethrough",
        shortcut!(command shift "X"),
    ),
    item(Action::Format(Format::Heading), icons::TEXT_H, "Heading", shortcut!(command shift "H")),
];
const CODE: &[Item] = &[
    item(Action::Format(Format::InlineCode), icons::CODE, "Code", shortcut!(command "E")),
    item(
        Action::Format(Format::CodeBlock),
        icons::CODE_BLOCK,
        "Code block",
        shortcut!(command shift "E"),
    ),
];
const LINK_QUOTE: &[Item] = &[
    item(Action::Format(Format::Link), icons::LINK, "Link", shortcut!(command "K")),
    item(Action::Format(Format::Quote), icons::QUOTES, "Quote", shortcut!(command shift ".")),
];
const MENTION: Item = item(Action::Mention, icons::AT, "Mention someone", "@");
const LISTS: &[Item] = &[
    item(
        Action::Format(Format::BulletList),
        icons::LIST_BULLETS,
        "Bulleted list",
        shortcut!(command shift "8"),
    ),
    item(
        Action::Format(Format::NumberedList),
        icons::LIST_NUMBERS,
        "Numbered list",
        shortcut!(command shift "7"),
    ),
    item(
        Action::Format(Format::TaskList),
        icons::LIST_CHECKS,
        "Task list",
        shortcut!(command shift "9"),
    ),
];
const SUGGESTION: &[Item] =
    &[item(Action::Suggestion, icons::PLUS_MINUS, "Insert suggestion", shortcut!(command "G"))];

/// What the toolbar offers in this editor.
pub struct Features {
    pub can_mention: bool,
    pub can_suggest: bool,
}

/// Formats the caret is inside, shown as pressed buttons.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ActiveFormats(Vec<Format>);

impl ActiveFormats {
    pub fn insert(&mut self, format: Format) {
        if !self.0.contains(&format) {
            self.0.push(format);
        }
    }

    pub fn contains(&self, format: Format) -> bool {
        self.0.contains(&format)
    }

    fn is_active(&self, action: Action) -> bool {
        matches!(action, Action::Format(format) if self.contains(format))
    }
}

fn groups(features: &Features) -> Vec<Vec<&'static Item>> {
    let mut groups: Vec<Vec<&Item>> = vec![TEXT_STYLE.iter().collect(), CODE.iter().collect()];
    let mut link_group: Vec<&Item> = LINK_QUOTE.iter().collect();
    if features.can_mention {
        link_group.push(&MENTION);
    }
    groups.push(link_group);
    groups.push(LISTS.iter().collect());
    if features.can_suggest {
        groups.push(SUGGESTION.iter().collect());
    }
    groups
}

/// Draws the formatting buttons right-aligned into the rest of the row; returns the clicked action.
pub fn show(
    ui: &mut Ui,
    active: &ActiveFormats,
    features: &Features,
    is_enabled: bool,
) -> Option<Action> {
    let groups = groups(features);
    let widths: Vec<f32> = groups.iter().map(|group| group_width(group.len())).collect();
    let shown = visible_groups(&widths, ui.available_width());
    let mut clicked = None;
    ui.add_enabled_ui(is_enabled, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = BUTTON_GAP;
            if shown < groups.len() {
                if let Some(action) = overflow(ui, &groups[shown..], active) {
                    clicked = Some(action);
                }
                divider(ui);
            }
            for (index, group) in groups[..shown].iter().enumerate().rev() {
                for item in group.iter().rev() {
                    if button(ui, item, active.is_active(item.action)).clicked() {
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

fn button(ui: &mut Ui, item: &Item, is_active: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(BUTTON_SIZE, BUTTON_SIZE), Sense::click());
    let tooltip = format!("{} ({})", item.label, item.shortcut);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), is_active, item.label)
    });
    let response = response.on_hover_text(tooltip);
    if ui.is_rect_visible(rect) {
        let rest = if is_active { palette.selection } else { egui::Color32::TRANSPARENT };
        let fill = if ui.is_enabled() {
            row::state_fill(ui, &response, rest)
        } else {
            egui::Color32::TRANSPARENT
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

fn overflow(ui: &mut Ui, groups: &[Vec<&'static Item>], active: &ActiveFormats) -> Option<Action> {
    let more = Item {
        action: Action::Mention,
        icon: icons::DOTS_THREE,
        label: "More formatting",
        shortcut: "menu",
    };
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(BUTTON_SIZE, BUTTON_SIZE), Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), more.label));
    let response = response.on_hover_text(more.label);
    let fill = row::state_fill(ui, &response, egui::Color32::TRANSPARENT);
    ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        more.icon,
        icons::rest_font(&palette, icons::SIZE),
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
                let is_active = active.is_active(item.action);
                let label = format!("{}   {}", item.icon, item.label);
                let entry = egui::Button::selectable(is_active, label)
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

/// What the composer's body shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The rich editor: the comment as it will look.
    Rich,
    /// The Markdown source as typed.
    Markdown,
    Preview,
}

const MODES: [(Mode, &str); 3] =
    [(Mode::Rich, "Rich text"), (Mode::Markdown, "Markdown"), (Mode::Preview, "Preview")];

/// Compact mode switch: a sunken track with the selected segment raised above it. Each segment
/// is a tab, focusable and switched with Enter or Space. Rich text shows only where `has_rich`.
pub fn mode_switch(ui: &mut Ui, id: egui::Id, mode: &mut Mode, has_rich: bool) -> bool {
    let palette = theme::palette(ui.ctx());
    let options: Vec<(Mode, &str)> =
        MODES.into_iter().filter(|(option, _)| has_rich || *option != Mode::Rich).collect();
    let font = theme::fonts::semibold_font(style::BODY_SIZE);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, label)| {
            ui.painter().layout_no_wrap((*label).to_owned(), font.clone(), palette.text).size().x
                + 2.0 * style::SPACE_M
        })
        .collect();
    let track_size = vec2(widths.iter().sum::<f32>() + 4.0, BUTTON_SIZE);
    let (track, _) = ui.allocate_exact_size(track_size, Sense::hover());
    ui.painter().rect_filled(track, style::RADIUS_CONTROL, palette.surface_subtle);
    let mut changed = false;
    let mut x = track.left() + 2.0;
    let mut selected_rect = None;
    let mut segments = Vec::new();
    for ((option, label), width) in options.iter().zip(&widths) {
        let rect = Rect::from_min_size(pos2(x, track.top() + 2.0), vec2(*width, BUTTON_SIZE - 4.0));
        x += width;
        let response = ui.interact(rect, id.with(("editor_mode", *label)), Sense::click());
        let is_selected = *mode == *option;
        response
            .widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, is_selected, *label));
        if is_selected {
            selected_rect = Some(rect);
        }
        if response.clicked() && !is_selected {
            *mode = *option;
            changed = true;
        }
        segments.push((rect, *label, is_selected, response));
    }
    if let Some(target) = selected_rect {
        let id = id.with("editor_mode_thumb");
        let left = motion::glide(ui.ctx(), id.with("left"), target.left(), motion::MEDIUM);
        let right = motion::glide(ui.ctx(), id.with("right"), target.right(), motion::MEDIUM);
        let thumb = Rect::from_min_max(pos2(left, target.top()), pos2(right, target.bottom()));
        ui.painter().rect(
            thumb,
            style::RADIUS_CONTROL - 1,
            palette.surface,
            Stroke::new(1.0, palette.border_strong),
            StrokeKind::Inside,
        );
    }
    for (rect, label, is_selected, response) in &segments {
        let hover = row::hover_amount(ui, response);
        let color = if *is_selected {
            palette.text
        } else {
            motion::mix(palette.text_secondary, palette.text, hover)
        };
        let font =
            if *is_selected { font.clone() } else { egui::FontId::proportional(style::BODY_SIZE) };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, *label, font, color);
        focus::ring(ui, response, *rect);
    }
    changed
}
