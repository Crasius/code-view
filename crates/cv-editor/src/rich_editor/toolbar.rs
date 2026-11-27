//! The formatting toolbar: buttons in groups, drawn by `markdown_editor::toolbar_layout` as the
//! Markdown composer's own, each showing whether its format is on at the caret. A button's
//! accessible name is its label; the tooltip adds the shortcut.

use cv_base::platform::shortcut;
use cv_base::theme::icons;
use egui::Ui;

use super::State;
use super::edit::structure::ListStyle;
use super::edit::{self as edit_ops};
use super::input::{Command, link_at};
use super::model::{Container, Kind, ListKind, Mark};
use crate::markdown_editor::toolbar_layout::{self, item};

type Item = toolbar_layout::Item<Action>;

#[cfg(not(target_os = "macos"))]
const HEADING_SHORTCUTS: [&str; 3] = ["Ctrl+Alt+1", "Ctrl+Alt+2", "Ctrl+Alt+3"];
#[cfg(target_os = "macos")]
const HEADING_SHORTCUTS: [&str; 3] = ["⌥⌘1", "⌥⌘2", "⌥⌘3"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
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
    pub(crate) fn command(self) -> Command {
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
    item(Action::Quote, icons::QUOTES, "Quote", shortcut!(command shift ".")),
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
pub(crate) struct Active {
    marks: Vec<Mark>,
    has_link: bool,
    heading: Option<u8>,
    list: Option<ListStyle>,
    is_quote: bool,
    is_code_block: bool,
}

impl Active {
    pub(crate) fn of(state: &State) -> Self {
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
pub(crate) fn show(
    ui: &mut Ui,
    active: &Active,
    can_mention: bool,
    is_enabled: bool,
) -> Option<Command> {
    toolbar_layout::show(ui, &groups(can_mention), |action| active.is_active(action), is_enabled)
        .map(Action::command)
}
