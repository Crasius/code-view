//! The title line's buttons of card and panel, right to left: expand or collapse, "Continue in
//! terminal", and a new conversation. A new conversation takes one click and asks nothing: the empty
//! conversation offers Undo until the next question goes out. A running answer is stopped by it;
//! Undo brings that conversation back with the answer as far as it came. The agent and model
//! chip ([`super::chip`]) sits left of them.

use cv_base::platform::shortcut;
use cv_base::theme::icons;
use cv_pr_session::assistant_link::OPEN_SHORTCUT;
use cv_services::Services;
use cv_widgets::buttons;
use egui::{Key, KeyboardShortcut, Modifiers, Ui};

use super::chip::{self, Line};
use super::{Action, Assistant};

/// Starts a new conversation while the assistant has the keys: the panel is open, or the card is
/// armed (a click or the focus went into it). The conventional "new" key, free elsewhere in the
/// app.
pub const NEW_SHORTCUT: &str = shortcut!(command "N");
pub const NEW_KEY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);

/// A title-line button clicked this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TitleClick {
    Expand,
    Collapse,
    Terminal,
    NewConversation,
}

impl TitleClick {
    pub(super) fn action(self) -> Action {
        match self {
            Self::Expand => Action::Expand,
            Self::Collapse => Action::Collapse,
            Self::Terminal => Action::ContinueInTerminal,
            Self::NewConversation => Action::NewConversation,
        }
    }
}

/// What a title line shows besides its buttons, read before the card or panel borrows the
/// assistant.
pub struct TitleLine {
    /// Why "Continue in terminal" is disabled ([`Assistant::terminal_block`]).
    terminal_block: Option<&'static str>,
    /// The chip's label.
    chip: String,
}

impl TitleLine {
    pub(super) fn new(assistant: &Assistant, services: &mut Services) -> Self {
        Self { terminal_block: assistant.terminal_block(), chip: chip::label(assistant, services) }
    }
}

/// The card's title-line controls.
pub fn card_action(ui: &mut Ui, line: &TitleLine) -> Option<TitleClick> {
    let tooltip = format!("Expand ({OPEN_SHORTCUT})");
    let expand = buttons::icon(ui, icons::ARROWS_OUT_SIMPLE, &tooltip).clicked();
    let others = terminal_new_and_chip(ui, line);
    if expand { Some(TitleClick::Expand) } else { others }
}

/// The panel's title-line controls, as the card's with collapse in place of expand.
pub(super) fn panel_action(ui: &mut Ui, line: &TitleLine) -> Option<TitleClick> {
    let collapse = buttons::icon(ui, icons::ARROWS_IN_SIMPLE, "Collapse (Esc)").clicked();
    let others = terminal_new_and_chip(ui, line);
    if collapse { Some(TitleClick::Collapse) } else { others }
}

/// "Continue in terminal", then "New conversation" left of it, then the chip.
fn terminal_new_and_chip(ui: &mut Ui, line: &TitleLine) -> Option<TitleClick> {
    let terminal = terminal_button(ui, line.terminal_block);
    let tooltip = format!("New conversation ({NEW_SHORTCUT})");
    let new = buttons::icon(ui, icons::PLUS, &tooltip).clicked();
    chip::in_title(ui, &line.chip, Line::Conversation);
    if terminal { Some(TitleClick::Terminal) } else { new.then_some(TitleClick::NewConversation) }
}

/// "Continue in terminal": the agent's CLI in the pull request's checkout, resuming this
/// conversation's session once there is one. Disabled with the reason `block` names.
fn terminal_button(ui: &mut Ui, block: Option<&'static str>) -> bool {
    let shown = ui.add_enabled_ui(block.is_none(), |ui| {
        buttons::icon(ui, icons::TERMINAL_WINDOW, "Continue in terminal")
    });
    match block {
        Some(why) => shown.inner.on_disabled_hover_text(why).clicked(),
        None => shown.inner.clicked(),
    }
}
