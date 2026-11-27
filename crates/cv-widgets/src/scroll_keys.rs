//! The keys that scroll a view by hand-kept offsets: ↑/↓ a step, Page Up/Down a screen,
//! Home/End to the ends. The views that paint their own rows (the rendered diff, the folder
//! summary) read them here; each keeps its own focus guard, its step and its clamp.

use egui::{InputState, Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollKey {
    Down,
    Up,
    PageDown,
    PageUp,
    Home,
    End,
}

impl ScrollKey {
    const KEYS: [(Key, Self); 6] = [
        (Key::ArrowDown, Self::Down),
        (Key::ArrowUp, Self::Up),
        (Key::PageDown, Self::PageDown),
        (Key::PageUp, Self::PageUp),
        (Key::Home, Self::Home),
        (Key::End, Self::End),
    ];

    /// Consumes every plain scroll key pressed this frame; the first in the enum's order wins.
    /// Plain is without Shift and Alt (Shift+↑/↓ extend a selection, and egui would match them
    /// as ↑/↓); `Modifiers::NONE` already refuses Ctrl and Cmd.
    pub fn consume(input: &mut InputState) -> Option<Self> {
        if input.modifiers.shift || input.modifiers.alt {
            return None;
        }
        Self::KEYS
            .map(|(key, scroll)| input.consume_key(Modifiers::NONE, key).then_some(scroll))
            .into_iter()
            .flatten()
            .next()
    }

    /// Where the view's top goes from `top`; `end` is where End leads. Not clamped.
    pub fn target(self, top: f32, step: f32, page: f32, end: f32) -> f32 {
        match self {
            Self::Down => top + step,
            Self::Up => top - step,
            Self::PageDown => top + page,
            Self::PageUp => top - page,
            Self::Home => 0.0,
            Self::End => end,
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers, RawInput};

    use super::ScrollKey;

    /// What [`ScrollKey::consume`] makes of `keys` pressed in one frame, and the keys it left.
    fn consume(keys: &[Key], modifiers: Modifiers) -> (Option<ScrollKey>, usize) {
        let ctx = egui::Context::default();
        let presses = keys.iter().map(|&key| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        let events = std::iter::once(Event::ModifiersChanged(modifiers)).chain(presses).collect();
        let input = RawInput { events, ..RawInput::default() };
        let mut seen = (None, 0);
        let mut output = ctx.run_ui(input, |ui| {
            seen = ui.input_mut(|input| {
                let scroll = ScrollKey::consume(input);
                let left = input.events.iter().filter(|event| matches!(event, Event::Key { .. }));
                (scroll, left.count())
            });
        });
        output.textures_delta.clear();
        seen
    }

    #[test]
    fn plain_keys_are_consumed_and_the_first_in_order_wins() {
        assert_eq!(
            consume(&[Key::PageDown, Key::ArrowDown], Modifiers::NONE),
            (Some(ScrollKey::Down), 0)
        );
        assert_eq!(consume(&[Key::End], Modifiers::NONE), (Some(ScrollKey::End), 0));
        assert_eq!(
            consume(&[Key::ArrowDown], Modifiers::SHIFT),
            (None, 1),
            "Shift+↓ is left alone"
        );
        assert_eq!(consume(&[Key::ArrowDown], Modifiers::ALT), (None, 1));
        assert_eq!(consume(&[Key::Home], Modifiers::COMMAND), (None, 1), "Ctrl+Home is not Home");
    }

    #[test]
    fn scroll_keys_move_by_a_step_a_page_or_to_the_ends() {
        let target = |key: ScrollKey| key.target(100.0, 60.0, 500.0, 4_000.0);
        assert_eq!(target(ScrollKey::Down), 160.0);
        assert_eq!(target(ScrollKey::Up), 40.0);
        assert_eq!(target(ScrollKey::PageDown), 600.0);
        assert_eq!(target(ScrollKey::PageUp), -400.0, "the caller clamps");
        assert_eq!(target(ScrollKey::Home), 0.0);
        assert_eq!(target(ScrollKey::End), 4_000.0);
    }
}
