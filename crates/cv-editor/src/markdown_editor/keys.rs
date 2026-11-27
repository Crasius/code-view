//! The keys the editor handles itself (formatting shortcuts, submit, list continuation,
//! nesting, cancel) and the paste smart paste rewrites, taken out of the input before the text
//! edit sees them.

use egui::{Event, Key, Modifiers, Ui};

use super::formatting::{Format, Indent};
use super::toolbar::{Action, Features};

/// Keys and pastes the editor handles itself, taken out of the input before the text edit sees them.
#[derive(Default)]
pub(super) struct Requests {
    pub action: Option<Action>,
    pub paste: Option<String>,
    pub enter: bool,
    pub tab: Option<Indent>,
    pub submit: bool,
    pub cancel: bool,
}

/// Takes the editor's keys and the first paste out of this frame's input. While the mention
/// popup is open, Enter, Tab and Esc stay with it.
pub(super) fn take_requests(ui: &Ui, is_mention_open: bool, features: &Features) -> Requests {
    let mut requests = Requests::default();
    ui.input_mut(|input| {
        input.events.retain(|event| match event {
            Event::Key { key, physical_key, pressed: true, modifiers, .. } => {
                if let Some(action) = shortcut(*key, *physical_key, *modifiers, features) {
                    requests.action.get_or_insert(action);
                    return false;
                }
                if *key == Key::Enter && modifiers.command {
                    requests.submit = true;
                    return false;
                }
                if is_mention_open {
                    return true;
                }
                if *key == Key::Enter && modifiers.is_none() && !requests.enter {
                    requests.enter = true;
                    return false;
                }
                if *key == Key::Tab
                    && !modifiers.command
                    && !modifiers.alt
                    && requests.tab.is_none()
                {
                    requests.tab = Some(if modifiers.shift { Indent::Out } else { Indent::In });
                    return false;
                }
                if *key == Key::Escape && modifiers.is_none() {
                    requests.cancel = true;
                    return false;
                }
                true
            }
            Event::Paste(pasted) if requests.paste.is_none() => {
                requests.paste = Some(pasted.clone());
                false
            }
            _ => true,
        });
    });
    requests
}

/// The editor's shortcuts. Shifted digits and punctuation also match by physical key, so
/// Ctrl+Shift+7 works on layouts where Shift+7 types `/`.
fn shortcut(
    key: Key,
    physical_key: Option<Key>,
    modifiers: Modifiers,
    features: &Features,
) -> Option<Action> {
    if !modifiers.command || modifiers.alt {
        return None;
    }
    let is = |wanted: Key| key == wanted || physical_key == Some(wanted);
    if modifiers.shift {
        let format = if key == Key::X {
            Format::Strikethrough
        } else if key == Key::E {
            Format::CodeBlock
        } else if key == Key::H {
            Format::Heading
        } else if is(Key::Period) || key == Key::Colon {
            Format::Quote
        } else if is(Key::Num7) || key == Key::Slash {
            Format::NumberedList
        } else if is(Key::Num8) || key == Key::OpenBracket {
            Format::BulletList
        } else if is(Key::Num9) || key == Key::CloseBracket {
            Format::TaskList
        } else {
            return None;
        };
        return Some(Action::Format(format));
    }
    let format = match key {
        Key::B => Format::Bold,
        Key::I => Format::Italic,
        Key::E => Format::InlineCode,
        Key::K => Format::Link,
        Key::G if features.can_suggest => return Some(Action::Suggestion),
        _ => return None,
    };
    Some(Action::Format(format))
}
