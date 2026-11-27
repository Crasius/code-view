//! How each system writes keyboard shortcuts, and the one shortcut that differs per system.
//!
//! Shortcuts themselves match on egui's `Modifiers::COMMAND`, which is Ctrl on Windows and ⌘ on
//! macOS, so the keys need nothing per system; only their labels do. [`shortcut!`] spells a
//! shortcut the system's way as a `&'static str` literal, so it fits `concat!` and constants:
//!
//! | `shortcut!(…)`        | Windows and others | macOS    |
//! |-----------------------|--------------------|----------|
//! | `command "K"`         | Ctrl+K             | ⌘K       |
//! | `command shift "C"`   | Ctrl+Shift+C       | ⇧⌘C      |
//! | `alt "V"`             | Alt+V              | ⌥V       |
//! | `shift "F7"`          | Shift+F7           | ⇧F7      |
//! | `command Enter`       | Ctrl+Enter         | ⌘↩       |
//! | `command click`       | Ctrl+click         | ⌘-click  |
//! | `command wheel`       | Ctrl+wheel         | ⌘-scroll |
//! | `Enter`               | Enter              | ↩        |
//! | `command` (a key cap) | Ctrl               | ⌘        |
//! | `focus_mode`          | F11                | ⌃⌘F      |
//!
//! Focus mode is F11 on Windows; macOS reserves F11 for Show Desktop, and ⌃⌘F is its own
//! full-screen shortcut, which focus mode resembles ([`FOCUS_MODE_KEY`]). Back and forward differ
//! too: Alt+Left and Alt+Right, ⌘[ and ⌘] on macOS ([`BACK_SHORTCUT`], [`FORWARD_SHORTCUT`]).
//! The diff's find bar opens with [`DIFF_FIND_KEY`].
//!
//! [`consume_plain`] takes a key pressed with no modifier at all: egui's
//! `consume_key(Modifiers::NONE, …)` also matches it with Alt or Shift held, so a bare letter
//! would swallow the Alt or Shift shortcut on the same key (N for the next change took Alt+N).

use egui::{Event, InputState, Key, KeyboardShortcut, Modifiers};

/// Consumes the presses of `key` made with no modifier held; with Alt, Shift, Ctrl or ⌘ they stay
/// for the shortcut that has them. True when there was one.
pub fn consume_plain(input: &mut InputState, key: Key) -> bool {
    let before = input.events.len();
    input.events.retain(|event| {
        !matches!(
            event,
            Event::Key { key: pressed, modifiers, pressed: true, .. }
                if *pressed == key && modifiers.is_none()
        )
    });
    input.events.len() != before
}

/// Toggles focus mode.
#[cfg(not(target_os = "macos"))]
pub const FOCUS_MODE_KEY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F11);
/// Toggles focus mode: Control-Command-F, the system's full-screen shortcut.
#[cfg(target_os = "macos")]
pub const FOCUS_MODE_KEY: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers { ctrl: true, mac_cmd: true, ..Modifiers::NONE }, Key::F);

/// Opens the diff's find bar: Ctrl+F, ⌘F on macOS. Settings searches with the same keys while it
/// is open; only one of them is on screen.
pub const DIFF_FIND_KEY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);

/// A shortcut's label as Windows (and Linux) write it: "Ctrl+Shift+C".
#[cfg(not(target_os = "macos"))]
#[macro_export]
macro_rules! shortcut {
    (@key Enter) => { "Enter" };
    (@key click) => { "click" };
    (@key wheel) => { "wheel" };
    (@key $key:literal) => { $key };
    (command) => { "Ctrl" };
    (focus_mode) => { "F11" };
    (command shift $key:tt) => { concat!("Ctrl+Shift+", $crate::shortcut!(@key $key)) };
    (command $key:tt) => { concat!("Ctrl+", $crate::shortcut!(@key $key)) };
    (alt $key:tt) => { concat!("Alt+", $crate::shortcut!(@key $key)) };
    (shift $key:tt) => { concat!("Shift+", $crate::shortcut!(@key $key)) };
    ($key:tt) => { $crate::shortcut!(@key $key) };
}

/// A shortcut's label as macOS writes it: modifier symbols in Apple's order (⌃⌥⇧⌘), no joiner.
#[cfg(target_os = "macos")]
#[macro_export]
macro_rules! shortcut {
    (@key Enter) => { "↩" };
    (@key click) => { "-click" };
    (@key wheel) => { "-scroll" };
    (@key $key:literal) => { $key };
    (command) => { "⌘" };
    (focus_mode) => { "⌃⌘F" };
    (command shift $key:tt) => { concat!("⇧⌘", $crate::shortcut!(@key $key)) };
    (command $key:tt) => { concat!("⌘", $crate::shortcut!(@key $key)) };
    (alt $key:tt) => { concat!("⌥", $crate::shortcut!(@key $key)) };
    (shift $key:tt) => { concat!("⇧", $crate::shortcut!(@key $key)) };
    ($key:tt) => { $crate::shortcut!(@key $key) };
}

/// Goes back through the history, as the system's browsers do: Alt+Left, ⌘[ on macOS.
#[cfg(not(target_os = "macos"))]
pub const BACK_SHORTCUT: &str = shortcut!(alt "Left");
/// Goes back through the history, as the system's browsers do: Alt+Left, ⌘[ on macOS.
#[cfg(target_os = "macos")]
pub const BACK_SHORTCUT: &str = shortcut!(command "[");
/// Goes forward through the history: Alt+Right, ⌘] on macOS.
#[cfg(not(target_os = "macos"))]
pub const FORWARD_SHORTCUT: &str = shortcut!(alt "Right");
/// Goes forward through the history: Alt+Right, ⌘] on macOS.
#[cfg(target_os = "macos")]
pub const FORWARD_SHORTCUT: &str = shortcut!(command "]");

#[cfg(test)]
mod tests {
    use egui::{Event, InputState, Key, KeyboardShortcut, ModifierNames, Modifiers};

    use super::{FOCUS_MODE_KEY, consume_plain};

    fn press(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn a_plain_key_leaves_its_alt_and_shift_shortcuts_alone() {
        let mut input = InputState::default();
        input.events = vec![press(Key::N, Modifiers::ALT), press(Key::F7, Modifiers::SHIFT)];
        assert!(!consume_plain(&mut input, Key::N), "Alt+N is not N");
        assert!(!consume_plain(&mut input, Key::F7), "Shift+F7 is not F7");
        assert_eq!(input.events.len(), 2, "both stay for their own shortcuts");

        input.events.push(press(Key::N, Modifiers::NONE));
        assert!(consume_plain(&mut input, Key::N));
        assert_eq!(input.events.len(), 2, "only the plain N was taken");
    }

    /// Windows and Linux spell a shortcut as modifier names joined with `+`.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn labels_read_as_on_windows() {
        assert_eq!(shortcut!(command "K"), "Ctrl+K");
        assert_eq!(shortcut!(command shift "C"), "Ctrl+Shift+C");
        assert_eq!(shortcut!(alt "↑"), "Alt+↑");
        assert_eq!(shortcut!(shift Enter), "Shift+Enter");
        assert_eq!(shortcut!(command click), "Ctrl+click");
        assert_eq!(concat!("Search (", shortcut!(command "K"), ")"), "Search (Ctrl+K)");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn labels_read_as_on_macos() {
        assert_eq!(shortcut!(command "K"), "⌘K");
        assert_eq!(shortcut!(command shift "C"), "⇧⌘C");
        assert_eq!(shortcut!(alt "V"), "⌥V");
        assert_eq!(shortcut!(command Enter), "⌘↩");
        assert_eq!(shortcut!(command click), "⌘-click");
    }

    /// The focus mode label names the key that toggles it, as egui spells it on the system.
    #[test]
    fn the_focus_mode_label_names_its_key() {
        let is_mac = cfg!(target_os = "macos");
        let names = if is_mac { ModifierNames::SYMBOLS } else { ModifierNames::NAMES };
        assert_eq!(
            KeyboardShortcut::format(&FOCUS_MODE_KEY, &names, is_mac),
            shortcut!(focus_mode)
        );
    }
}
