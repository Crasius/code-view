//! Systems without their own integration (the BSDs and others): the system draws the title bar,
//! the window opens where the system puts it, egui's bundled fonts, no Save As dialog, `xdg-open`
//! for folders.

use std::path::{Path, PathBuf};

use egui::{Rangef, Rect, Ui, ViewportBuilder};
use review_store::checkouts::TerminalChoice;

pub use super::portable::{
    DialogOwner, FrameHold, PlacedWindow, dialog_owner, downloads_dir, hide_console,
    hide_std_console, register_chrome_fonts,
};
use super::{FontSources, SaveRequest};

pub const AZURE_CLI: &str = "az";
pub const HAS_SAVE_DIALOG: bool = false;
/// The terminals Settings › Local checkouts offers, with their names.
pub const TERMINALS: &[(TerminalChoice, &str)] = &[(TerminalChoice::System, "xterm")];

/// The environment as started.
pub fn prepare_environment() {}

/// The system's window as it is.
pub fn viewport(builder: ViewportBuilder) -> ViewportBuilder {
    builder
}

/// Opens `path` in the desktop's file manager; fails when `xdg-open` does not start.
pub fn open_folder(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(path).spawn().map(drop)
}

/// Opens `xterm` in `dir` running `program` with `args`.
pub fn open_terminal(dir: &Path, program: &str, args: &[String]) -> std::io::Result<()> {
    std::process::Command::new("xterm")
        .arg("-e")
        .arg(program)
        .args(args)
        .current_dir(dir)
        .spawn()
        .map(drop)
}

/// None: egui's bundled fonts serve the text.
pub fn font_sources() -> FontSources {
    FontSources::default()
}

/// No system setting the app reads.
pub fn prefers_reduced_motion() -> bool {
    false
}

/// The system draws the title bar.
pub fn integrate_title_bar(_creation: &eframe::CreationContext<'_>) -> bool {
    false
}

/// The window's own parts of an integrated title bar: none, the system draws them.
pub struct Caption {
    bar: Rect,
}

impl Caption {
    pub fn show(_ui: &mut Ui, bar: Rect, _has_icon: bool) -> Self {
        Self { bar }
    }

    pub fn free(&self) -> Rangef {
        self.bar.x_range()
    }

    pub fn publish(self, _ctx: &egui::Context, _holes: &[Rect]) {}
}

pub fn monitor_refresh_hz(_frame: &eframe::Frame) -> Option<u32> {
    None
}

/// No dialog here ([`HAS_SAVE_DIALOG`] keeps the buttons away).
pub fn ask_save_path(_owner: DialogOwner, _request: &SaveRequest<'_>) -> Option<PathBuf> {
    None
}

/// Opens `xterm` in `dir`.
pub fn open_shell(dir: &Path, _terminal: TerminalChoice) -> std::io::Result<()> {
    std::process::Command::new("xterm").current_dir(dir).spawn().map(drop)
}

/// No folder dialog here.
pub fn ask_folder(_owner: DialogOwner, _title: &str) -> Option<PathBuf> {
    None
}
