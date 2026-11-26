//! The formatting toolbar: buttons in groups, drawn as the Markdown composer draws its own
//! (the size, gaps, dividers and fitting of `markdown_editor::toolbar_layout`, the same overflow
//! menu), each showing whether its format is on at the caret. A button's accessible name is its
//! label; the tooltip adds the shortcut.

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, motion, style};
use cv_widgets::{focus, glyph, row};
use egui::{Align2, Color32, Sense, Ui, vec2};

use super::State;
use super::edit::structure::ListStyle;
use super::edit::{self as edit_ops};
use super::input::{Command, link_at};
use super::model::{Container, Kind, ListKind, Mark};
use crate::markdown_editor::toolbar_layout::{
    BUTTON_GAP, BUTTON_SIZE, divider, group_width, visible_groups,
};

#[cfg(not(target_os = "macos"))]
const HEADING_SHORTCUTS: [&str; 3] = ["Ctrl+Alt+1", "Ctrl+Alt+2", "Ctrl+Alt+3"];
#[cfg(target_os = "macos")]
const HEADING_SHORTCUTS: [&str; 3] = ["⌥⌘1", "⌥⌘2", "⌥⌘3"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Mark(Mark),
    Link,
    Heading(u8),
    List(ListStyle),
    Quote,
    CodeBlock,
    Rule,
    Mention,
}

impl Action {
    pub fn command(self) -> Command {
        match self {
            Action::Mark(mark) => Command::Mark(mark),
            Action::Link => Command::EditLink,
            Action::Heading(level) => Command::Heading(level),
            Action::List(style) => Command::List(style),
            Action::Quote => Command::Quote,
            Action::CodeBlock => Command::CodeBlock,
            Action::Rule => Command::Rule,
            Action::Mention => Command::Mention,
        }
    }
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

const MARKS: &[Item] = &[
    item(Action::Mark(Mark::Bold), icons::TEXT_B, "Bold", shortcut!(command "B")),
    item(Action::Mark(Mark::Italic), icons::TEXT_ITALIC, "Italic", shortcut!(command "I")),
    item(Action::Mark(Mark::Underline), icons::TEXT_UNDERLINE, "Underline", shortcut!(command "U")),
    item(
        Action::Mark(Mark::Strike),
        icons::TEXT_STRIKETHROUGH,
        "Strikethrough",
        shortcut!(command shift "X"),
    ),
];
const CODE: &[Item] = &[
    item(Action::Mark(Mark::Code), icons::CODE, "Inline code", shortcut!(command "E")),
    item(Action::CodeBlock, icons::CODE_BLOCK, "Code block", "```"),
];
const LINK_QUOTE: &[Item] = &[
    item(Action::Link, icons::LINK, "Link", shortcut!(command "K")),
    item(Action::Quote, icons::QUOTES, "Quote", shortcut!(command shift "B")),
];
const MENTION: Item = item(Action::Mention, icons::AT, "Mention someone", "@");
const HEADINGS: &[Item] = &[
    item(Action::Heading(1), icons::TEXT_H_ONE, "Heading 1", HEADING_SHORTCUTS[0]),
    item(Action::Heading(2), icons::TEXT_H_TWO, "Heading 2", HEADING_SHORTCUTS[1]),
    item(Action::Heading(3), icons::TEXT_H_THREE, "Heading 3", HEADING_SHORTCUTS[2]),
];
const LISTS: &[Item] = &[
    item(
        Action::List(ListStyle::Bullet),
        icons::LIST_BULLETS,
        "Bulleted list",
        shortcut!(command shift "8"),
    ),
    item(
        Action::List(ListStyle::Ordered),
        icons::LIST_NUMBERS,
        "Numbered list",
        shortcut!(command shift "7"),
    ),
    item(
        Action::List(ListStyle::Task),
        icons::LIST_CHECKS,
        "Task list",
        shortcut!(command shift "9"),
    ),
];
const INSERT: &[Item] = &[item(Action::Rule, icons::MINUS, "Horizontal rule", "---")];
const SCRIPTS: &[Item] = &[
    item(Action::Mark(Mark::Subscript), icons::TEXT_SUBSCRIPT, "Subscript", "<sub>"),
    item(Action::Mark(Mark::Superscript), icons::TEXT_SUPERSCRIPT, "Superscript", "<sup>"),
    item(Action::Mark(Mark::Kbd), icons::KEYBOARD, "Keyboard key", "<kbd>"),
];

/// Which formats are on at the caret or across the selection.
#[derive(Debug, Default)]
pub struct Active {
    marks: Vec<Mark>,
    has_link: bool,
    heading: Option<u8>,
    list: Option<ListStyle>,
    is_quote: bool,
    is_code_block: bool,
}

impl Active {
    pub fn of(state: &State) -> Self {
        let caret = state.doc.clamp(state.selection.head);
        let block = state.doc.block(caret.block);
        let marks = if state.selection.is_empty() {
            let at = state.stored.clone().unwrap_or_else(|| block.content.marks_at(caret.offset));
            Mark::ALL.into_iter().filter(|mark| at.has(*mark)).collect()
        } else {
            Mark::ALL
                .into_iter()
                .filter(|mark| edit_ops::has_mark(&state.doc, state.selection, *mark))
                .collect()
        };
        let list = block.item().map(|(_, container)| match container {
            Container::Item { task: Some(_), .. } => ListStyle::Task,
            Container::Item { list: ListKind::Ordered(_), .. } => ListStyle::Ordered,
            _ => ListStyle::Bullet,
        });
        Self {
            marks,
            has_link: link_at(&state.doc, caret).is_some(),
            heading: match block.kind {
                Kind::Heading(level) => Some(level),
                _ => None,
            },
            list,
            is_quote: block
                .nesting
                .iter()
                .any(|container| matches!(container, Container::Quote(_))),
            is_code_block: matches!(block.kind, Kind::Code(_)),
        }
    }

    fn is_active(&self, action: Action) -> bool {
        match action {
            Action::Mark(mark) => self.marks.contains(&mark),
            Action::Link => self.has_link,
            Action::Heading(level) => self.heading == Some(level),
            Action::List(style) => self.list == Some(style),
            Action::Quote => self.is_quote,
            Action::CodeBlock => self.is_code_block,
            Action::Rule | Action::Mention => false,
        }
    }
}

fn groups(can_mention: bool) -> Vec<Vec<&'static Item>> {
    let mut link_quote: Vec<&Item> = LINK_QUOTE.iter().collect();
    if can_mention {
        link_quote.push(&MENTION);
    }
    vec![
        MARKS.iter().collect(),
        CODE.iter().collect(),
        link_quote,
        HEADINGS.iter().collect(),
        LISTS.iter().collect(),
        INSERT.iter().collect(),
        SCRIPTS.iter().collect(),
    ]
}

/// Draws the buttons right-aligned into the rest of the row; returns the clicked button's command.
pub fn show(ui: &mut Ui, active: &Active, can_mention: bool, is_enabled: bool) -> Option<Command> {
    let groups = groups(can_mention);
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
    clicked.map(Action::command)
}

fn button(ui: &mut Ui, item: &Item, is_active: bool) -> egui::Response {
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

fn overflow(ui: &mut Ui, groups: &[Vec<&'static Item>], active: &Active) -> Option<Action> {
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
                    active.is_active(item.action),
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
