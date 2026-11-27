//! Brings the main window back where it was closed: position, size and maximized state.
//!
//! The system does not remember this for an app, so [`WindowKeeper`] keeps it in the app settings
//! ([`review_store::WindowPlacement`]); [`platform::PlacedWindow`] reads it from the window and
//! puts it back (on Windows in physical pixels of the virtual screen, which stay unambiguous across
//! monitors with different DPI). eframe starts the window hidden until the first frame is painted;
//! the saved rect is applied in that gap, so the window appears in place. Saves happen on close
//! and a moment after the last move or resize, never per frame and never while idle.

use review_store::WindowPlacement;

use cv_base::platform;
use cv_services::Services;

/// Seconds without further moves or resizes before the placement is written.
const SAVE_DELAY: f64 = 0.5;
/// What the window looked like last frame, as egui reports it; only compared for changes.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Seen {
    outer: Option<egui::Rect>,
    is_maximized: Option<bool>,
    is_minimized: Option<bool>,
}

/// Restores the main window's placement at start-up and saves it when it changes.
pub struct WindowKeeper {
    /// `None` without a window the platform can place (tests, systems without placement).
    window: Option<platform::PlacedWindow>,
    seen: Option<Seen>,
    /// Input time of the last move or resize not saved yet.
    changed_at: Option<f64>,
}

impl WindowKeeper {
    /// Puts the (still hidden) window of `creation` where `saved` says, or centers it.
    pub fn restore(creation: &eframe::CreationContext<'_>, saved: Option<WindowPlacement>) -> Self {
        let window = platform::PlacedWindow::of(creation);
        if let Some(window) = &window {
            window.restore(saved);
        }
        Self { window, seen: None, changed_at: None }
    }

    /// Once per frame: notes moves and resizes, and saves the placement when the window closes or
    /// has rested for `SAVE_DELAY`.
    pub fn track(&mut self, ctx: &egui::Context, services: &mut Services) {
        let (seen, is_closing, now) = ctx.input(|input| {
            let viewport = input.viewport();
            let seen = Seen {
                outer: viewport.outer_rect,
                is_maximized: viewport.maximized,
                is_minimized: viewport.minimized,
            };
            (seen, viewport.close_requested(), input.time)
        });
        // The first frame is the baseline: the restored placement, nothing to save yet.
        if self.seen.replace(seen).is_some_and(|previous| previous != seen) {
            self.changed_at = Some(now);
        }
        match self.changed_at {
            _ if is_closing => self.save(services),
            Some(changed_at) if now - changed_at >= SAVE_DELAY => self.save(services),
            Some(changed_at) => ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                SAVE_DELAY - (now - changed_at),
            )),
            None => {}
        }
    }

    fn save(&mut self, services: &mut Services) {
        self.changed_at = None;
        let Some(placement) = self.window.as_ref().and_then(platform::PlacedWindow::read) else {
            return;
        };
        if services.settings.window != Some(placement) {
            services.settings.window = Some(placement);
            services.save_settings();
        }
    }
}
