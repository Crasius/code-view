//! The assistant on screen: the card in the Overview's side column and the full-height panel. Both
//! show the same parts top to bottom: the agent and model chip, a blocking state with its fix, the
//! note about new commits, the conversation ([`super::turns`]), what a waiting question waits for,
//! the attached lines and the composer. In the card the conversation fills the side column's room
//! down to its bottom edge until the user drags the flat splitter between conversation and
//! composer; that height is kept in the settings. The composer is plain text: what the user types
//! goes to the agent as written. Every part returns what the user did as an [`Action`], which
//! [`super::actions::act`] carries out after drawing. The card's conversation takes the mouse wheel
//! only once the card is armed ([`super::wheel`]). The title lines' buttons are in
//! [`super::title`]; after a new conversation the empty one offers Undo. The chip is
//! [`super::chip`]; with its option on it sits in the title line instead.

use cv_base::theme::{self, icons, style};
use cv_pr_session::session::PullRequestSession;
use cv_services::Services;
use cv_services::agent_names::{cli_name, install_url};
use cv_widgets::splitter::Splitter;
use cv_widgets::states::{self, Action as StateAction};
use cv_widgets::{buttons, glyph, row};
use egui::{Event, Id, Key, KeyboardShortcut, Modifiers, RichText, TextEdit, Ui};

use super::title::{self, NEW_KEY, TitleClick};
use super::{Action, Assistant, Block, chip, turns};

/// The card's conversation is never shorter than this, however little room the side column has:
/// an answer of about ten lines, or a question and the start of its answer, shows without
/// scrolling, and "Jump to latest" still has room to float. Below, the side column scrolls.
const CARD_MIN_HEIGHT: f32 = 260.0;
/// The card frame's padding below its content (the Overview's card padding).
const CARD_FRAME_BOTTOM: f32 = 16.0;
/// The band between the card's conversation and its composer; the splitter's line runs through
/// its middle, its grab area stays clear of both.
const CARD_SPLIT_BAND: f32 = 2.0 * style::SPACE_S;
/// The panel's composer height before it was first measured.
const COMPOSER_HEIGHT_GUESS: f32 = 110.0;
/// The question being written grows up to this many lines, then scrolls inside the composer.
const INPUT_MAX_LINES: f32 = 8.0;

/// Where the conversation is drawn.
#[derive(Clone, Copy, PartialEq, Hash, Debug)]
enum Room {
    Card,
    Panel,
}

/// The card's content below its title; `title` is what the title line's buttons were clicked for
/// this frame.
/// `column_bottom` is where the side column's visible room ends with the page scrolled to the top,
/// in the same coordinates as `ui` (it scrolls with the card).
pub fn card(
    ui: &mut Ui,
    assistant: &mut Assistant,
    session: &PullRequestSession,
    services: &mut Services,
    title: Option<TitleClick>,
    column_bottom: f32,
) {
    assistant.wheel.begin(ui.ctx());
    let mut action = title.map(TitleClick::action);
    let mut picked = head(ui, assistant, session, services);
    let below_id = ui.scope_id().with("assistant_card_below");
    let below = ui.data(|data| data.get_temp::<f32>(below_id)).unwrap_or(0.0);
    let conversation_top = ui.min_rect().bottom();
    let fill = column_bottom - conversation_top - below - CARD_FRAME_BOTTOM;
    // Dragged taller, the card may fill the side column's whole visible height; the column scrolls
    // to show it.
    let chrome = conversation_top - ui.max_rect().top() + below + CARD_FRAME_BOTTOM;
    let tallest = ui.clip_rect().height() - chrome;
    let range = CARD_MIN_HEIGHT..=tallest.max(fill).max(CARD_MIN_HEIGHT);
    let height =
        services.settings.assistant.card_height.unwrap_or(fill).clamp(*range.start(), *range.end());
    picked = picked.or(conversation(ui, assistant, session, Room::Card, height));
    let conversation_bottom = ui.min_rect().bottom();
    if card_splitter(ui, range, height, fill, &mut services.settings.assistant.card_height) {
        services.save_settings();
    }
    if let Some(picked) = picked.or(composer(ui, assistant)) {
        action = Some(picked);
    }
    let measured = ui.min_rect().bottom() - conversation_bottom;
    if (measured - below).abs() > 0.5 {
        ui.data_mut(|data| data.insert_temp(below_id, measured));
        ui.ctx().request_discard("the assistant card's composer changed height");
    }
    finish(ui, assistant, session, services, action);
}

/// After the card's frame was drawn in `card`: arms or disarms its wheel for the next pass, and an
/// armed card wears the theme's focus stroke. The card is a group in the accessibility tree,
/// selected while armed.
pub fn card_drawn(ui: &Ui, assistant: &mut Assistant, card: egui::Rect) {
    assistant.wheel.end(ui.ctx(), card);
    let is_armed = assistant.wheel.is_armed();
    if is_armed {
        let stroke = ui.visuals().selection.stroke;
        ui.painter().rect_stroke(card, style::RADIUS_CARD, stroke, egui::StrokeKind::Inside);
    }
    ui.interact(card, ui.scope_id().with("assistant_card"), egui::Sense::hover())
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::Group, true, is_armed, CARD_LABEL));
}

/// The card's name in the accessibility tree.
pub const CARD_LABEL: &str = "Assistant card";

/// The flat splitter between the card's conversation and its composer: dragged down, the
/// conversation grows and the composer moves down with it, past the column's visible room up to
/// the end of `range`. A drag or a key step keeps the height in `stored`; the reset forgets it, so
/// the conversation fills its room (`fill`) again. Returns
/// whether to save.
fn card_splitter(
    ui: &mut Ui,
    range: std::ops::RangeInclusive<f32>,
    height: f32,
    fill: f32,
    stored: &mut Option<f32>,
) -> bool {
    let band = ui.allocate_space(egui::vec2(ui.available_width(), CARD_SPLIT_BAND)).1;
    // The height as shown, so a drag in a window too small for the stored one starts where the
    // line is.
    let mut value = height;
    let resize = Splitter::new("assistant_card_splitter", "Resize assistant conversation")
        .default_value(fill)
        .range(range)
        .show_horizontal(ui, band.center().y, band.x_range(), &mut value);
    if resize.is_reset {
        *stored = None;
    } else if resize.response.dragged() || resize.committed {
        *stored = Some(value);
    }
    resize.committed
}

/// The full-height panel in place of the side column.
pub fn panel(
    ui: &mut Ui,
    assistant: &mut Assistant,
    session: &PullRequestSession,
    services: &mut Services,
) {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    let title_line = assistant.title_line(services);
    let frame = style::card_frame(&palette).inner_margin(egui::Margin::same(16));
    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(ui.available_height());
        // The title line is one button tall; its buttons sit at its right end.
        let line = egui::vec2(ui.available_width(), buttons::icon_width());
        ui.allocate_ui_with_layout(line, egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.label(RichText::new(TITLE).font(theme::fonts::semibold_font(style::SECTION_SIZE)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                action = title::panel_action(ui, &title_line).map(TitleClick::action);
            });
        });
        ui.add_space(style::SPACE_S);
        if let Some(picked) = panel_body(ui, assistant, session, services) {
            action = Some(picked);
        }
    });
    let rect = shown.response.rect;
    ui.interact(rect, ui.scope_id().with("assistant_panel"), egui::Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Group, true, "Assistant panel"));
    finish(ui, assistant, session, services, action);
}

/// The panel below its title: the conversation fills the height down to the composer, which is
/// pinned to the bottom edge and grows upward with its lines. The composer's height from the
/// frame before places it; a change lays the frame out again before it shows.
fn panel_body(
    ui: &mut Ui,
    assistant: &mut Assistant,
    session: &PullRequestSession,
    services: &mut Services,
) -> Option<Action> {
    let area = ui.available_rect_before_wrap();
    let id = ui.scope_id().with("assistant_composer_height");
    let known = ui.data(|data| data.get_temp::<f32>(id)).unwrap_or(COMPOSER_HEIGHT_GUESS);
    let composer_top = (area.bottom() - known).max(area.top());
    let conversation_rect = egui::Rect::from_min_max(
        area.min,
        egui::pos2(area.right(), (composer_top - style::SPACE_S).max(area.top())),
    );
    let layout = egui::Layout::top_down(egui::Align::Min);
    let mut top = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("assistant_conversation")
            .max_rect(conversation_rect)
            .layout(layout),
    );
    let mut action = head(&mut top, assistant, session, services);
    let height = top.available_height().max(0.0);
    action = action.or(conversation(&mut top, assistant, session, Room::Panel, height));
    let composer_rect = egui::Rect::from_min_max(egui::pos2(area.left(), composer_top), area.max);
    let mut bottom = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("assistant_composer_area")
            .max_rect(composer_rect)
            .layout(layout),
    );
    action = action.or(composer(&mut bottom, assistant));
    let measured = bottom.min_rect().height();
    if (measured - known).abs() > 0.5 {
        ui.data_mut(|data| data.insert_temp(id, measured));
        ui.ctx().request_discard("the assistant's composer changed height");
    }
    ui.advance_cursor_after_rect(area);
    action
}

/// The card's title.
pub const TITLE: &str = "Ask about this PR";

/// Esc that nobody else took: stops the answer, else collapses the panel. While the assistant has
/// the keys (panel open, card armed), Ctrl+N starts a new conversation. Then the action.
fn finish(
    ui: &Ui,
    assistant: &mut Assistant,
    session: &PullRequestSession,
    services: &mut Services,
    action: Option<Action>,
) {
    let mut action = action;
    let is_free = !ui.ctx().text_edit_focused() && !egui::Popup::is_any_open(ui.ctx());
    if action.is_none() && is_free && (assistant.is_busy() || assistant.is_expanded) {
        let escape = ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        if escape {
            action = Some(if assistant.is_busy() { Action::Stop } else { Action::Collapse });
        }
    }
    let has_keys = assistant.is_expanded || assistant.wheel.is_armed();
    if action.is_none() && has_keys && ui.input_mut(|input| input.consume_shortcut(&NEW_KEY)) {
        action = Some(Action::NewConversation);
    }
    if let Some(action) = action {
        super::actions::act(assistant, action, session, services);
    }
}

/// The menu of the chip in the title line, a blocking state and the note about new commits.
fn head(
    ui: &mut Ui,
    assistant: &mut Assistant,
    session: &PullRequestSession,
    services: &mut Services,
) -> Option<Action> {
    let mut action = chip::title_menu(ui.ctx(), chip::Line::Conversation, assistant, services);
    if let Some(block) = assistant.block.clone() {
        action = action.or(block_state(ui, &block));
        ui.add_space(style::SPACE_S);
    }
    if assistant.has_new_commits(session) {
        let actions = [StateAction::primary(icons::ARROW_CLOCKWISE, "Update the agent")];
        let note = "New commits since this conversation started";
        if states::warning_with_actions(ui, note, &actions).is_some() {
            action = Some(Action::UpdateAgent);
        }
        ui.add_space(style::SPACE_S);
    }
    action
}

/// The conversation, then what a waiting question waits for: `height` tall in the card (its
/// splitter sizes it), at most `height` in the panel. It scrolls and follows a streaming answer to
/// the bottom; scrolled up, "Jump to latest" leads back.
fn conversation(
    ui: &mut Ui,
    assistant: &mut Assistant,
    session: &PullRequestSession,
    room: Room,
    height: f32,
) -> Option<Action> {
    assistant.links_publish(ui.ctx(), session);
    let waiting = assistant.waiting.filter(|_| assistant.block.is_none());
    let action = if assistant.chat.turns.is_empty() {
        empty_conversation(ui, assistant, room, height)
    } else {
        let waiting_room = if waiting.is_some() { WAITING_ROW_HEIGHT } else { 0.0 };
        transcript(ui, assistant, room, (height - waiting_room).max(0.0))
    };
    if let Some(waiting) = waiting {
        ui.add_space(style::SPACE_XS);
        states::loading(ui, waiting);
    }
    action
}

/// The turns, scrolled; "Jump to latest" floats over them while the latest is out of view.
fn transcript(ui: &mut Ui, assistant: &mut Assistant, room: Room, height: f32) -> Option<Action> {
    let mut action = None;
    let jump_id = ui.scope_id().with(("assistant_jump", room));
    let jump = ui.data_mut(|data| data.remove_temp::<bool>(jump_id)).unwrap_or(false);
    // The card's column has no height to hand out: its conversation asks for all of it.
    let shortest = if room == Room::Card { height } else { height.min(160.0) };
    let takes_wheel = room == Room::Panel || assistant.wheel.is_armed();
    let source = egui::scroll_area::ScrollSource { mouse_wheel: takes_wheel, ..Default::default() };
    let output = egui::ScrollArea::vertical()
        .scroll_source(source)
        .id_salt(("assistant_transcript", room))
        .max_height(height)
        .min_scrolled_height(shortest)
        .stick_to_bottom(true)
        .auto_shrink([false, room == Room::Panel])
        .show(ui, |ui| {
            action = turns::show(ui, assistant);
            if jump {
                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
            }
        });
    let below =
        output.content_size.y - output.inner_rect.height() - output.state.clamped_offset().y;
    if below > 1.0 && jump_to_latest(ui, output.inner_rect) {
        ui.data_mut(|data| data.insert_temp(jump_id, true));
    }
    action
}

/// Before the first question: Undo after a new conversation, and in the card the conversation's
/// room, so the composer sits where it will.
fn empty_conversation(
    ui: &mut Ui,
    assistant: &Assistant,
    room: Room,
    height: f32,
) -> Option<Action> {
    let top = ui.cursor().top();
    let action = assistant.previous.as_ref().and_then(|_| undo_note(ui));
    if room == Room::Card {
        let left = (height - (ui.cursor().top() - top)).max(0.0);
        ui.allocate_space(egui::vec2(ui.available_width(), left));
    }
    action
}

/// In the empty conversation after "New conversation": Undo brings the one before back, until the
/// next question goes out.
fn undo_note(ui: &mut Ui) -> Option<Action> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    turns::bubble(ui, "New conversation", |ui| {
        row::with_trailing_block(
            ui,
            |ui| {
                if buttons::secondary_icon(ui, icons::ARROW_COUNTER_CLOCKWISE, "Undo").clicked() {
                    action = Some(Action::UndoNewConversation);
                }
            },
            |ui| {
                let text =
                    RichText::new("Started a new conversation.").color(palette.text_secondary);
                ui.add(egui::Label::new(text).wrap());
            },
        );
    });
    action
}

/// Room a waiting question's line takes below the conversation.
const WAITING_ROW_HEIGHT: f32 = 28.0;

/// "Jump to latest", floating at the bottom center of the scrolled conversation `rect`.
fn jump_to_latest(ui: &mut Ui, rect: egui::Rect) -> bool {
    let label = "Jump to latest";
    let size = egui::vec2(buttons::icon_button_width(ui, label), style::CONTROL_HEIGHT);
    let at = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.bottom() - style::SPACE_S - size.y / 2.0),
        size,
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(at));
    buttons::secondary_icon(&mut child, icons::ARROW_DOWN, label).clicked()
}

/// A blocking state and its fix.
fn block_state(ui: &mut Ui, block: &Block) -> Option<Action> {
    match block {
        Block::NotInstalled(kind) => {
            let install = format!("Install {}", cli_name(*kind));
            let actions = [
                StateAction::primary(icons::ARROW_SQUARE_OUT, &install),
                StateAction::secondary(icons::ARROW_CLOCKWISE, "Check again"),
            ];
            let text = format!("{} is not installed on this computer.", cli_name(*kind));
            match states::warning_with_actions(ui, &text, &actions) {
                Some(0) => {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(install_url(*kind)));
                    None
                }
                Some(_) => Some(Action::CheckAgain),
                None => None,
            }
        }
        Block::NotSignedIn(kind) => {
            let actions = [
                StateAction::primary(icons::TERMINAL_WINDOW, "Sign in in a terminal"),
                StateAction::secondary(icons::ARROW_CLOCKWISE, "Check again"),
            ];
            let text = format!(
                "{} is installed but not signed in. You sign in in the terminal; Code View never \
                 sees the login.",
                cli_name(*kind)
            );
            match states::warning_with_actions(ui, &text, &actions)? {
                0 => Some(Action::SignIn),
                _ => Some(Action::CheckAgain),
            }
        }
        Block::Checkout(message) => {
            let actions = [StateAction::primary(icons::ARROW_CLOCKWISE, "Retry")];
            let text = format!("The checkout for the agent failed: {message}");
            states::error_with_actions(ui, &text, &actions).map(|_| Action::Resume)
        }
        Block::Stopped(message) => {
            let actions = [StateAction::primary(icons::ARROW_CLOCKWISE, "Restart")];
            let text = format!("The agent stopped: {message}");
            states::error_with_actions(ui, &text, &actions).map(|_| Action::Resume)
        }
    }
}

/// The attached lines and the composer.
fn composer(ui: &mut Ui, assistant: &mut Assistant) -> Option<Action> {
    attached(ui, assistant).or(input(ui, assistant))
}

/// The lines the next question is about, with the button that drops them.
fn attached(ui: &mut Ui, assistant: &Assistant) -> Option<Action> {
    let quote = assistant.quote.as_ref()?;
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    let text = format!("{}, {} attached", quote.path, turns::lines(quote));
    turns::bubble(ui, "Attached lines", |ui| {
        row::with_trailing_block(
            ui,
            |ui| {
                if buttons::icon(ui, icons::X, "Remove the attached lines").clicked() {
                    action = Some(Action::DropQuote);
                }
            },
            |ui| {
                let look = |text: &str| RichText::new(text).color(palette.text_secondary);
                glyph::truncated(ui, look(icons::QUOTES), look(&text));
            },
        );
    });
    ui.add_space(style::SPACE_S);
    action
}

/// The question being written, as plain text, with Ask at its right end, or Stop while the agent
/// answers. Enter asks, Shift+Enter starts a new line, Esc stops the answer or collapses the panel.
fn input(ui: &mut Ui, assistant: &mut Assistant) -> Option<Action> {
    let palette = theme::palette(ui.ctx());
    let id = Id::unique(("assistant_composer", &assistant.key));
    let is_busy = assistant.is_busy();
    let has_text = !assistant.composer.trim().is_empty();
    let action = keys(ui, assistant, id);
    let hint = if assistant.chat.turns.is_empty() {
        "Ask anything about this pull request"
    } else {
        "Follow up…"
    };
    let is_stopping = assistant.is_stopping;
    let grab_focus = std::mem::take(&mut assistant.focus_composer);
    let composer = &mut assistant.composer;
    let margin = egui::Margin::same(style::SPACE_S as i8);
    let frame = egui::Frame::new().fill(palette.surface).corner_radius(style::RADIUS_CARD);
    let shown = frame.inner_margin(margin).show(ui, |ui| {
        row::with_trailing_block(
            ui,
            |ui| {
                if is_busy {
                    let stop = buttons::secondary_busy(ui, icons::SQUARE, "Stop", is_stopping);
                    let stop = stop.on_hover_text("Stop the answer (Esc)");
                    (stop.clicked() && !is_stopping).then_some(Pressed::Stop)
                } else {
                    let ask = ui.add_enabled_ui(has_text, |ui| {
                        buttons::primary_busy(ui, icons::PAPER_PLANE_RIGHT, "Ask", false)
                    });
                    let ask =
                        ask.inner.on_hover_text("Ask (Enter). Shift+Enter starts a new line.");
                    ask.clicked().then_some(Pressed::Ask)
                }
            },
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("assistant_composer_scroll")
                    .max_height(
                        INPUT_MAX_LINES * ui.text_style_height(&egui::TextStyle::Body) + 8.0,
                    )
                    .auto_shrink([false, true])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let edit = TextEdit::multiline(composer)
                            .id(id)
                            .hint_text(hint)
                            .desired_rows(1)
                            .desired_width(f32::INFINITY)
                            .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(4)))
                            .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter));
                        let response = ui.add(edit);
                        if grab_focus {
                            response.request_focus();
                        }
                    });
            },
        )
        .0
    });
    let border = if ui.memory(|memory| memory.has_focus(id)) {
        egui::Stroke::new(2.0, palette.accent)
    } else {
        egui::Stroke::new(1.0, palette.border_strong)
    };
    let rect = shown.response.rect;
    ui.painter().rect_stroke(rect, style::RADIUS_CARD, border, egui::StrokeKind::Inside);
    ui.interact(rect, id.with("frame"), egui::Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Group, true, "Question composer"));
    action.or(shown.inner.map(|pressed| match pressed {
        Pressed::Ask => Action::Ask(assistant.composer.clone()),
        Pressed::Stop => Action::Stop,
    }))
}

/// The composer's button that was clicked.
enum Pressed {
    Ask,
    Stop,
}

/// What Enter and Esc in the focused input `id` ask for: the question, or stopping the answer or
/// collapsing the panel. Esc also leaves the input.
fn keys(ui: &Ui, assistant: &Assistant, id: Id) -> Option<Action> {
    if !ui.memory(|memory| memory.has_focus(id)) {
        return None;
    }
    let (send, escape) = take_keys(ui);
    if escape {
        ui.memory_mut(|memory| memory.surrender_focus(id));
        return if assistant.is_busy() {
            Some(Action::Stop)
        } else {
            assistant.is_expanded.then_some(Action::Collapse)
        };
    }
    let can_ask = !assistant.is_busy() && !assistant.composer.trim().is_empty();
    (send && can_ask).then(|| Action::Ask(assistant.composer.clone()))
}

/// Takes Enter (alone or with Ctrl or Cmd, not with Shift) and Esc out of this frame's input before
/// the text edit sees them: whether to ask, whether to stop or leave.
fn take_keys(ui: &Ui) -> (bool, bool) {
    let (mut send, mut escape) = (false, false);
    ui.input_mut(|input| {
        input.events.retain(|event| match event {
            Event::Key { key: Key::Enter, pressed: true, modifiers, .. }
                if !modifiers.shift && !modifiers.alt =>
            {
                send = true;
                false
            }
            Event::Key { key: Key::Escape, pressed: true, .. } => {
                escape = true;
                false
            }
            _ => true,
        });
    });
    (send, escape)
}
