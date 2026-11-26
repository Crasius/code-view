//! The parts of the surface that need nothing from the system, shared by the systems without an
//! own implementation of them ([`super::macos`], [`super::linux`] and [`super::other`]).

#[cfg(not(target_os = "linux"))]
use std::path::PathBuf;
use std::time::Duration;

use egui::FontDefinitions;
use review_store::WindowPlacement;

/// Child processes open no window here: nothing to hide.
pub fn hide_console(_command: &mut tokio::process::Command) {}

/// [`hide_console`] for a process run from a thread of its own.
pub fn hide_std_console(_command: &mut std::process::Command) {}

/// `~/Downloads` (Linux reads the XDG user folders instead).
#[cfg(not(target_os = "linux"))]
pub fn downloads_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Downloads"))
}

/// The window chrome draws with no font of its own.
pub fn register_chrome_fonts(_fonts: &mut FontDefinitions) {}

/// No placement: the window opens where the system puts it (centered at the default size).
pub enum PlacedWindow {}

impl PlacedWindow {
    pub fn of(_creation: &eframe::CreationContext<'_>) -> Option<Self> {
        None
    }

    pub fn restore(&self, _saved: Option<WindowPlacement>) {
        match *self {}
    }

    pub fn read(&self) -> Option<WindowPlacement> {
        match *self {}
    }
}

/// Sleeps until a deadline. Without a way to wait on the input queue, input that arrives meanwhile
/// is handled right after the held frame (at most one frame period later).
#[derive(Default)]
pub struct FrameHold;

impl FrameHold {
    #[expect(
        clippy::disallowed_methods,
        reason = "the frame rate limit's wait: holding the UI thread until the next frame is its job"
    )]
    #[expect(
        clippy::unused_self,
        reason = "the signature of the Windows FrameHold, whose wait reuses its timer"
    )]
    pub fn wait(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Dialogs here need no owner window: they come to the front on their own.
#[derive(Debug, Clone, Copy)]
pub struct DialogOwner;

pub fn dialog_owner() -> DialogOwner {
    DialogOwner
}
