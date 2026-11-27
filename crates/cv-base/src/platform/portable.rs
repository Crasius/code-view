//! The parts of the surface that need nothing from the system, shared by the systems without an
//! own implementation of them ([`super::macos`], [`super::linux`] and [`super::other`]). The
//! system-drawn title bar, the missing Save dialog and `xdg-open` serve Linux and the other
//! systems.

#[cfg(not(target_os = "macos"))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(not(target_os = "macos"))]
use std::process::Command;
use std::time::Duration;

use egui::FontDefinitions;
#[cfg(not(target_os = "macos"))]
use egui::{Rangef, Rect, Ui};
use review_store::WindowPlacement;

#[cfg(not(target_os = "macos"))]
use super::SaveRequest;

/// Child processes open no window here: nothing to hide.
pub fn hide_console(_command: &mut tokio::process::Command) {}

/// [`hide_console`] for a process run from a thread of its own.
pub fn hide_std_console(_command: &mut std::process::Command) {}

/// Starts `command` and leaves a thread that waits for it, so the finished child is reaped instead
/// of staying a zombie until the app exits.
pub(crate) fn spawn_reaped(command: &mut std::process::Command) -> std::io::Result<()> {
    let mut child = command.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Opens `path` in the desktop's file manager; fails when `xdg-open` does not start.
#[cfg(not(target_os = "macos"))]
pub fn open_folder(path: &Path) -> std::io::Result<()> {
    spawn_reaped(Command::new("xdg-open").arg(path))
}

/// The system draws the title bar.
#[cfg(not(target_os = "macos"))]
pub fn integrate_title_bar(_creation: &eframe::CreationContext<'_>) -> bool {
    false
}

/// The window's own parts of an integrated title bar: none, the system draws them.
#[cfg(not(target_os = "macos"))]
pub struct Caption {
    bar: Rect,
}

#[cfg(not(target_os = "macos"))]
impl Caption {
    pub fn show(_ui: &mut Ui, bar: Rect, _has_icon: bool) -> Self {
        Self { bar }
    }

    pub fn free(&self) -> Rangef {
        self.bar.x_range()
    }

    #[expect(
        clippy::unused_self,
        reason = "the same signature as Windows and macOS, where the bar publishes its caption"
    )]
    pub fn publish(self, _ctx: &egui::Context, _holes: &[Rect]) {}
}

/// winit reports no refresh rate the frame limit could use under every compositor.
#[cfg(not(target_os = "macos"))]
pub fn monitor_refresh_hz(_frame: &eframe::Frame) -> Option<u32> {
    None
}

/// No dialog here ([`super::HAS_SAVE_DIALOG`] keeps the buttons away).
#[cfg(not(target_os = "macos"))]
pub fn ask_save_path(_owner: DialogOwner, _request: &SaveRequest<'_>) -> Option<PathBuf> {
    None
}

/// `~/Downloads` (Linux reads the XDG user folders instead).
#[cfg(not(target_os = "linux"))]
pub fn downloads_dir() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join("Downloads"))
}

/// The window chrome draws with no font of its own.
pub(crate) fn register_chrome_fonts(_fonts: &mut FontDefinitions) {}

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
