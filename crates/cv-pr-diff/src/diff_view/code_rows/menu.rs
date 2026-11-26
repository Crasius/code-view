//! The context menu of a code row: copy the selection, the line, or its path and number, comment
//! on the selected lines or on the line, and ask the assistant about the selected lines.

use egui::Response;

use review_store::ChatQuote;

use crate::diff_view::COMMENT_SHORTCUT;
use crate::diff_view::selecting::RangeTarget;
use cv_base::platform::shortcut;
use cv_base::theme::icons;
use cv_pr_session::assistant_link::{self as assistant, OPEN_SHORTCUT};
use cv_widgets::glyph;

/// What the menu offers for the row it was opened on.
pub(in crate::diff_view) struct MenuItems<'a> {
    /// What is selected: the lines it touches and its text (to ask the assistant about).
    pub selection: Option<ChatQuote>,
    /// The selected lines to comment on.
    pub target: Option<&'a RangeTarget>,
    /// The row's line: its text and number.
    pub line: Option<(String, u32)>,
    pub path: &'a str,
    /// The platform takes a comment on the row's line.
    pub can_comment: bool,
}

/// What the user picked in the menu that the view carries out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::diff_view) enum MenuChoice {
    CopySelection,
    CommentOnSelection,
    CommentOnLine,
}

/// Whether the menu is open, and what was picked in it this frame.
pub(in crate::diff_view) struct MenuShown {
    pub is_open: bool,
    pub choice: Option<MenuChoice>,
}

/// The context menu of `background` with `items`; copying the line or its place happens here.
pub(in crate::diff_view) fn show(background: &Response, items: &MenuItems<'_>) -> MenuShown {
    let mut choice = None;
    let has_selection = items.selection.is_some();
    let menu = background.context_menu(|ui| {
        if has_selection
            && glyph::button(ui, icons::COPY, "Copy selection")
                .on_hover_text(shortcut!(command "C"))
                .clicked()
        {
            choice = Some(MenuChoice::CopySelection);
            ui.close();
        }
        if let Some(target) = items.target {
            let words = format!("Comment on {}", target.label);
            let label = format!("{}  {words}", icons::CHAT_CIRCLE_TEXT);
            let button = ui.add_enabled(target.blocked.is_none(), egui::Button::new(label));
            glyph::speak_as(&button, &words);
            let button = match target.blocked {
                Some(reason) => button.on_disabled_hover_text(reason),
                None => button.on_hover_text(COMMENT_SHORTCUT),
            };
            if button.clicked() {
                choice = Some(MenuChoice::CommentOnSelection);
                ui.close();
            }
        }
        if let Some(quote) = &items.selection
            && assistant::is_enabled(ui.ctx())
            && glyph::button(ui, icons::ROBOT, "Ask about these lines")
                .on_hover_text(OPEN_SHORTCUT)
                .clicked()
        {
            assistant::request(ui.ctx(), Some(quote.clone()));
            ui.close();
        }
        if has_selection || items.target.is_some() {
            ui.separator();
        }
        if let Some((text, number)) = &items.line {
            if glyph::button(ui, icons::COPY, "Copy line").clicked() {
                ui.ctx().copy_text(text.clone());
                ui.close();
            }
            if glyph::button(ui, icons::LINK, &format!("Copy {}:{number}", items.path)).clicked() {
                ui.ctx().copy_text(format!("{}:{number}", items.path));
                ui.close();
            }
        }
        if items.can_comment && glyph::button(ui, icons::CHAT_CIRCLE_TEXT, "Add comment").clicked()
        {
            choice = Some(MenuChoice::CommentOnLine);
            ui.close();
        }
    });
    MenuShown { is_open: menu.is_some(), choice }
}
