//! Keys, text and IME events to commands. The editor reads this frame's events while it has
//! focus and takes the ones it handles out of the input, so no other widget sees them twice.

use egui::{Event, ImeEvent, Key, Modifiers, Ui};

use super::{Command, Motion};
use crate::rich_editor::edit::structure::ListStyle;
use crate::rich_editor::edit::{self as edit_ops};
use crate::rich_editor::model::{Mark, Pos, Selection};
use crate::rich_editor::{Popup, State};

/// Word jumps and edges use Alt on macOS, Ctrl elsewhere.
fn is_word_modifier(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") { modifiers.alt } else { modifiers.command }
}

/// Takes this frame's events the focused editor handles out of the input.
pub(crate) fn take_commands(ui: &Ui, state: &mut State) -> Vec<Command> {
    let mut commands = Vec::new();
    let is_mention_open = matches!(state.popup, Popup::Mention { .. });
    let mut ime = Vec::new();
    ui.input_mut(|input| {
        input.events.retain(|event| match event {
            Event::Text(text) => {
                commands.push(Command::Insert(text.clone()));
                false
            }
            Event::Copy => {
                commands.push(Command::Copy);
                false
            }
            Event::Cut => {
                commands.push(Command::Cut);
                false
            }
            Event::Paste(text) => {
                commands.push(Command::Paste(text.clone()));
                false
            }
            Event::Ime(event) => {
                ime.push(event.clone());
                false
            }
            Event::Key { key, physical_key, pressed: true, modifiers, .. } => {
                if is_mention_open
                    && matches!(
                        key,
                        Key::ArrowUp | Key::ArrowDown | Key::Enter | Key::Tab | Key::Escape
                    )
                {
                    return true;
                }
                match key_command(*key, *physical_key, *modifiers) {
                    Some(command) => {
                        commands.push(command);
                        false
                    }
                    None => true,
                }
            }
            _ => true,
        });
    });
    for event in ime {
        ime_event(state, event, &mut commands);
    }
    commands
}

/// The command of a key press, `None` for keys the editor leaves to others.
fn key_command(key: Key, physical_key: Option<Key>, modifiers: Modifiers) -> Option<Command> {
    let is = |wanted: Key| key == wanted || physical_key == Some(wanted);
    let extend = modifiers.shift;
    let word = is_word_modifier(modifiers);
    let moving = |motion| Some(Command::Move { motion, extend });
    if modifiers.command && modifiers.alt && !modifiers.shift {
        return match key {
            Key::Num1 => Some(Command::Heading(1)),
            Key::Num2 => Some(Command::Heading(2)),
            Key::Num3 => Some(Command::Heading(3)),
            _ => None,
        };
    }
    match key {
        Key::ArrowLeft if word => moving(Motion::WordLeft),
        Key::ArrowRight if word => moving(Motion::WordRight),
        Key::ArrowLeft if cfg!(target_os = "macos") && modifiers.mac_cmd => {
            moving(Motion::RowStart)
        }
        Key::ArrowRight if cfg!(target_os = "macos") && modifiers.mac_cmd => moving(Motion::RowEnd),
        Key::ArrowUp if cfg!(target_os = "macos") && modifiers.mac_cmd => moving(Motion::DocStart),
        Key::ArrowDown if cfg!(target_os = "macos") && modifiers.mac_cmd => moving(Motion::DocEnd),
        Key::ArrowLeft => moving(Motion::Left),
        Key::ArrowRight => moving(Motion::Right),
        Key::ArrowUp => moving(Motion::Up),
        Key::ArrowDown => moving(Motion::Down),
        Key::Home if modifiers.command => moving(Motion::DocStart),
        Key::End if modifiers.command => moving(Motion::DocEnd),
        Key::Home => moving(Motion::RowStart),
        Key::End => moving(Motion::RowEnd),
        Key::PageUp => moving(Motion::PageUp),
        Key::PageDown => moving(Motion::PageDown),
        Key::Backspace if word => Some(Command::DeleteWord { forward: false }),
        Key::Delete if word => Some(Command::DeleteWord { forward: true }),
        Key::Backspace => Some(Command::Backspace),
        Key::Delete => Some(Command::Delete),
        Key::Enter if modifiers.command => None,
        Key::Enter if modifiers.shift => Some(Command::LineBreak),
        Key::Enter => Some(Command::Enter),
        Key::Tab if !modifiers.command && !modifiers.alt => {
            Some(Command::Indent { outdent: modifiers.shift })
        }
        _ if modifiers.command && modifiers.shift => {
            if is(Key::Num7) || key == Key::Slash {
                Some(Command::List(ListStyle::Ordered))
            } else if is(Key::Num8) || key == Key::OpenBracket {
                Some(Command::List(ListStyle::Bullet))
            } else if is(Key::Num9) || key == Key::CloseBracket {
                Some(Command::List(ListStyle::Task))
            } else if key == Key::X || key == Key::S {
                Some(Command::Mark(Mark::Strike))
            } else if is(Key::Period) || key == Key::B {
                Some(Command::Quote)
            } else if key == Key::Z {
                Some(Command::Redo)
            } else {
                None
            }
        }
        _ if modifiers.command => match key {
            Key::B => Some(Command::Mark(Mark::Bold)),
            Key::I => Some(Command::Mark(Mark::Italic)),
            Key::U => Some(Command::Mark(Mark::Underline)),
            Key::E => Some(Command::Mark(Mark::Code)),
            Key::K => Some(Command::EditLink),
            Key::A => Some(Command::SelectAll),
            Key::Z => Some(Command::Undo),
            Key::Y => Some(Command::Redo),
            _ => None,
        },
        _ => None,
    }
}

/// IME composition: the preedit text sits selected at the caret and is not recorded; the commit
/// replaces it as typed text.
fn ime_event(state: &mut State, event: ImeEvent, commands: &mut Vec<Command>) {
    match event {
        ImeEvent::Preedit { text, .. } => {
            clear_preedit(state);
            if text.is_empty() {
                return;
            }
            let (from, to) = state.selection.ordered();
            let at = edit_ops::delete_range(&mut state.doc, from, to);
            let marks = state.doc.block(at.block).content.marks_at(at.offset);
            let end = edit_ops::insert_text(&mut state.doc, Selection::caret(at), &text, &marks);
            // Typed on a rule, the text went into a new paragraph after it: it starts there.
            let length = text.chars().count();
            let start = Pos::new(end.block, end.offset.saturating_sub(length));
            state.preedit = Some((start, length));
            state.selection = Selection { anchor: start, head: end };
            state.changed();
        }
        ImeEvent::Commit(text) => {
            clear_preedit(state);
            if !text.is_empty() {
                commands.push(Command::Insert(text));
            }
        }
        ImeEvent::DeleteSurrounding { before_chars, after_chars } => {
            clear_preedit(state);
            let caret = state.selection.head;
            let from = Pos::new(caret.block, caret.offset.saturating_sub(before_chars));
            let to = Pos::new(caret.block, caret.offset + after_chars);
            state.selection = Selection { anchor: from, head: state.doc.clamp(to) };
            commands.push(Command::Backspace);
        }
        #[expect(
            deprecated,
            reason = "the enum still has the deprecated variants; egui-winit 0.37 never sends them"
        )]
        ImeEvent::Enabled | ImeEvent::Disabled => {}
    }
}

fn clear_preedit(state: &mut State) {
    if let Some((at, length)) = state.preedit.take() {
        let end = Pos::new(at.block, at.offset + length);
        edit_ops::delete_range(&mut state.doc, at, end);
        state.selection = Selection::caret(at);
        state.changed();
    }
}
