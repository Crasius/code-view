//! Focus mode: the Files tab's diff or a pipeline job's log takes over the window. The title bar,
//! the navigation rail, the pull request header, the tabs and the banners leave; one slim
//! [`strip`] at the top keeps the view's critical controls, the caption buttons and the window
//! drag area. Hiding the chrome only takes work away: nothing else is laid out or painted.
//!
//! - F11 (macOS: ⌃⌘F, see [`platform::FOCUS_MODE_KEY`]) toggles it while a view that can take
//!   over is on screen, as do the toolbar buttons (focus icon), the strip's exit button and "Toggle
//!   focus mode" in the Ctrl+K search.
//! - Esc leaves it, unless a text field had the keys or a menu was open: those take Esc first,
//!   and so do views that use it themselves (a selection clears before focus mode ends).
//! - Ctrl+K leaves it and opens the search in the title bar.
//! - Leaving the view (another tab, page or pull request) ends it.
//!
//! Views offer themselves each frame ([`offer`]); the shell hides the chrome in a frame when focus
//! mode is on and a view offered itself in the frame before ([`begin_frame`]), so every part of a
//! frame agrees on the layout. A change in the middle of a frame (a click, a pick in the search)
//! is applied at its end, which discards the frame: the next pass lays the new layout out before
//! anything is painted, so no frame shows the old chrome around the new view. The views keep their
//! `Ui` ids in both layouts, so scroll positions and selections stay where they were.
//!
//! Whether focus mode is on is not saved: the app always opens with its chrome.

pub mod strip;

use egui::{Context, Id, Key, Modifiers, Ui};

use cv_base::platform::{self, shortcut};
use cv_base::theme::icons;
use cv_runtime::intents;
use cv_widgets::buttons;

/// A view that can take over the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The Files tab: file tree, diff or preview.
    Files,
    /// A pipeline job's log.
    Log,
}

/// Toggles focus mode ([`platform::FOCUS_MODE_KEY`]), in tooltips.
pub const SHORTCUT: &str = shortcut!(focus_mode);
pub const ENTER_HINT: &str =
    concat!("Focus mode: hide everything but this view (", shortcut!(focus_mode), ")");
pub const EXIT_HINT: &str = concat!("Exit focus mode (", shortcut!(focus_mode), " or Esc)");

#[derive(Debug, Clone, Copy, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent per-frame flags of one small state kept in egui's temp memory"
)]
struct State {
    is_on: bool,
    /// The chrome is hidden in this frame.
    is_shown: bool,
    /// The view that offered itself in the last frame, and in this one so far.
    offered_last: Option<Surface>,
    offered: Option<Surface>,
    toggle_requested: bool,
    /// At the end of the last frame a text field had the keys, or a menu was open: Esc is theirs.
    was_typing: bool,
    had_popup: bool,
    /// The app draws the window's title bar, so the strip takes its caption buttons and drag area.
    is_integrated: bool,
}

fn state_id() -> Id {
    Id::unique("review.focus_mode")
}

fn load(ctx: &Context) -> State {
    ctx.data(|data| data.get_temp(state_id())).unwrap_or_default()
}

fn store(ctx: &Context, state: State) {
    ctx.data_mut(|data| data.insert_temp(state_id(), state));
}

/// Call first in the frame, before any chrome: takes F11 (macOS: ⌃⌘F) and Ctrl+K and decides the frame's
/// layout. Returns true when the chrome stays hidden this frame.
pub fn begin_frame(ctx: &Context, is_integrated: bool) -> bool {
    let mut state = load(ctx);
    state.offered_last = state.offered.take();
    state.is_integrated = is_integrated;
    let can_toggle = state.is_on || state.offered_last.is_some();
    if can_toggle && ctx.input_mut(|input| input.consume_shortcut(&platform::FOCUS_MODE_KEY)) {
        state.is_on = !state.is_on;
    }
    // The search lives in the title bar: Ctrl+K brings the bar back with the search open.
    if state.is_on
        && !ctx.text_edit_focused()
        && ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::K))
    {
        state.is_on = false;
        intents::open_search(ctx);
    }
    state.is_shown = state.is_on && state.offered_last.is_some();
    store(ctx, state);
    state.is_shown
}

/// A view that can take over the window is on screen. Returns true when it has the window to
/// itself this frame and draws its [`strip`] instead of its toolbar.
pub fn offer(ctx: &Context, surface: Surface) -> bool {
    let mut state = load(ctx);
    state.offered = Some(surface);
    store(ctx, state);
    state.is_shown
}

/// The chrome is hidden in this frame.
pub fn is_shown(ctx: &Context) -> bool {
    load(ctx).is_shown
}

/// Focus mode can be switched now: a view that can take over is on screen, or it is on.
pub fn is_available(ctx: &Context) -> bool {
    let state = load(ctx);
    state.is_on || state.offered_last.is_some() || state.offered.is_some()
}

/// Switches focus mode at the end of this frame (a button, the search).
pub fn request_toggle(ctx: &Context) {
    let mut state = load(ctx);
    state.toggle_requested = true;
    store(ctx, state);
}

/// The strip draws the caption buttons and publishes the window's drag area.
fn is_integrated(ctx: &Context) -> bool {
    load(ctx).is_integrated
}

/// Call last in the frame, after every view had its keys: applies requested changes, Esc and a
/// view that left the screen. A change of layout discards the frame, so the new one is laid out
/// before it is painted.
pub fn end_frame(ctx: &Context) {
    let mut state = load(ctx);
    if std::mem::take(&mut state.toggle_requested) && (state.is_on || state.offered.is_some()) {
        state.is_on = !state.is_on;
    }
    let is_escape_free = state.is_shown && !state.was_typing && !state.had_popup;
    if is_escape_free && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
        state.is_on = false;
    }
    if state.offered.is_none() {
        state.is_on = false;
    }
    let will_show = state.is_on && state.offered.is_some();
    if will_show != state.is_shown {
        ctx.request_discard("focus mode changed the layout");
        // Should the pass not be repeated, the next frame comes at once.
        ctx.request_repaint();
    }
    state.was_typing = ctx.text_edit_focused();
    state.had_popup = egui::Popup::is_any_open(ctx);
    store(ctx, state);
}

/// For tests: a view notes its top visible line (row or line index) and how far the view's top
/// edge lies below that line's top, after laying out the frame.
#[cfg(any(test, feature = "test-support"))]
pub fn note_top_line(ctx: &Context, surface: Surface, line: usize, below_top: f32) {
    ctx.data_mut(|data| {
        data.insert_temp(
            Id::unique(("review.focus_mode.top_line", surface as u8)),
            (line, below_top),
        )
    });
}

/// The top visible line `surface` noted last, see [`note_top_line`]; read by the focus mode
/// walkthrough and the pipelines walkthrough's live log.
#[cfg(any(test, feature = "test-support"))]
pub fn top_line(ctx: &Context, surface: Surface) -> Option<(usize, f32)> {
    ctx.data(|data| data.get_temp(Id::unique(("review.focus_mode.top_line", surface as u8))))
}

/// The toolbar button that enters focus mode (the focus icon); in a view's own toolbar.
pub fn enter_button(ui: &mut Ui) {
    if buttons::icon(ui, icons::ARROWS_OUT_SIMPLE, ENTER_HINT).clicked() {
        request_toggle(ui.ctx());
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers, RawInput};

    use super::{Surface, begin_frame, end_frame, is_shown, offer, request_toggle};
    use cv_base::platform::FOCUS_MODE_KEY;

    fn key(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    /// The system's focus mode shortcut (F11, macOS ⌃⌘F).
    fn focus_key() -> Event {
        Event::Key {
            key: FOCUS_MODE_KEY.logical_key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: FOCUS_MODE_KEY.modifiers,
        }
    }

    /// One frame: the shell around a view that offers itself while `shows_view`; returns whether
    /// the frame's last pass hid the chrome and how many passes it took.
    fn frame(
        ctx: &egui::Context,
        events: Vec<Event>,
        shows_view: bool,
        toggle: bool,
    ) -> (bool, usize) {
        let mut passes = 0;
        let mut shown = false;
        let mut output = ctx.run_ui(RawInput { events, ..RawInput::default() }, |ui| {
            passes += 1;
            shown = begin_frame(ui.ctx(), false);
            if shows_view {
                offer(ui.ctx(), Surface::Files);
            }
            if toggle && passes == 1 {
                request_toggle(ui.ctx());
            }
            end_frame(ui.ctx());
        });
        // The fonts' first upload; no renderer takes it here.
        output.textures_delta.clear();
        (shown, passes)
    }

    #[test]
    fn f11_and_esc_switch_only_while_a_view_can_take_over() {
        let ctx = egui::Context::default();
        assert!(!frame(&ctx, vec![focus_key()], false, false).0, "nothing to focus on");
        frame(&ctx, Vec::new(), true, false);
        assert_eq!(
            frame(&ctx, vec![focus_key()], true, false),
            (true, 1),
            "the shortcut hides the chrome in the same pass"
        );
        assert!(frame(&ctx, Vec::new(), true, false).0, "and it stays hidden");
        assert_eq!(
            frame(&ctx, vec![key(Key::Escape)], true, false),
            (false, 2),
            "Esc ends it; the frame is laid out again"
        );
        assert!(!is_shown(&ctx));
        assert!(!frame(&ctx, Vec::new(), true, false).0, "the chrome stays back");
    }

    #[test]
    fn a_button_switches_at_the_end_of_the_frame_and_leaving_the_view_ends_it() {
        let ctx = egui::Context::default();
        frame(&ctx, Vec::new(), true, false);
        assert_eq!(
            frame(&ctx, Vec::new(), true, true),
            (true, 2),
            "the request repeats the pass with the new layout"
        );
        assert!(is_shown(&ctx));
        assert!(frame(&ctx, Vec::new(), true, false).0);
        frame(&ctx, Vec::new(), false, false);
        assert!(!frame(&ctx, Vec::new(), true, false).0, "another page ended focus mode");
    }
}
