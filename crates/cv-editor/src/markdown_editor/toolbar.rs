//! The editor's header: the Rich text / Markdown / Preview switch and the Markdown formatting
//! toolbar. Buttons are 28 px icon targets in groups (text style, code, link and quote, lists,
//! suggestion) separated by hairlines; groups that do not fit move, from the right, into a
//! "More formatting" menu. `toolbar_layout` draws them, as it draws the rich editor's toolbar.

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, motion, style};
use cv_widgets::{elide, focus, row};
use egui::{Align2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::formatting::Format;
use super::toolbar_layout::{self, BUTTON_SIZE, item};

type Item = toolbar_layout::Item<Action>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Format(Format),
    Suggestion,
    Mention,
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
    toolbar_layout::show(ui, &groups(features), |action| active.is_active(action), is_enabled)
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
pub fn mode_switch(ui: &mut Ui, id: egui::Id, mode: &mut Mode, has_rich: bool) {
    let palette = theme::palette(ui.ctx());
    let options: Vec<(Mode, &str)> =
        MODES.into_iter().filter(|(option, _)| has_rich || *option != Mode::Rich).collect();
    let font = theme::fonts::semibold_font(style::BODY_SIZE);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, label)| elide::text_width(ui, label, &font) + 2.0 * style::SPACE_M)
        .collect();
    let track_size = vec2(widths.iter().sum::<f32>() + 4.0, BUTTON_SIZE);
    let (track, _) = ui.allocate_exact_size(track_size, Sense::hover());
    ui.painter().rect_filled(track, style::RADIUS_CONTROL, palette.surface_subtle);
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
}
