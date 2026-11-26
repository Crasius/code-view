//! Pointer clicks for the whole-app walkthroughs that background work cannot take away.
//!
//! `egui_kittest`'s `Node::click` reads the node's place once and queues three events that
//! `Harness::step` runs as three frames: the pointer moves there, the button goes down, the button
//! comes up. egui hit-tests a press against the widgets of the frame before it, here the frame of
//! the move, which the caller never looked at. Background work (git reading a thread's code, a
//! host's answer) is taken in by whatever frame comes after it finished. Taken in by the move frame
//! it can move the page: the thread card's code replaces its taller skeleton and the reply box
//! below it moves up 60 points, a rate-limit banner opens above the header and the tabs move down.
//! The press then lands where the widget was a frame earlier, on something else, and the test
//! waits for an effect that never comes. On a busy runner, where git finishes at any moment
//! between two frames, that failed the pipeline.
//!
//! [`Clicks::click_at`] queues only the press and the release, like `Node::click` for the caller's
//! next step: the press comes in the next frame, at the place read from the last one, so the hit
//! test runs against exactly the widgets the caller read. egui takes a button event's position as
//! the pointer's, so the pointer moves there in that frame. Where the release lands does not
//! matter: egui counts a click for the widget that took the press, wherever it moved meanwhile.
//! A real pointer has no such gap: the frame it presses in tests against the frame on screen.

use egui::{Event, Modifiers, PointerButton, Pos2, Rect};
use egui_kittest::Harness;

pub(super) trait Clicks {
    /// A primary click at the centre of `rect`, read from the last frame ([`Self::click_point`]).
    fn click_at(&self, rect: Rect) {
        self.click_point(rect.center(), PointerButton::Primary);
    }

    /// A secondary click at the centre of `rect`, read from the last frame: a context menu.
    fn secondary_click_at(&self, rect: Rect) {
        self.click_point(rect.center(), PointerButton::Secondary);
    }

    /// A click of `button` at `at`, queued for the next step: pressed in its first frame, released
    /// in its second.
    fn click_point(&self, at: Pos2, button: PointerButton);
}

impl<State> Clicks for Harness<'_, State> {
    fn click_point(&self, at: Pos2, button: PointerButton) {
        for pressed in [true, false] {
            self.event(Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
    }
}
