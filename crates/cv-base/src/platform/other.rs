//! Systems without their own integration (the BSDs and others): the system draws the title bar,
//! the window opens where the system puts it, egui's bundled fonts, no Save As dialog, `xdg-open`
//! for folders.

use std::path::{Path, PathBuf};
use std::process::Command;

use egui::ViewportBuilder;
use review_store::checkouts::TerminalChoice;

use super::FontSources;
pub(crate) use super::portable::register_chrome_fonts;
use super::portable::spawn_reaped;
pub use super::portable::{
    Caption, DialogOwner, FrameHold, PlacedWindow, ask_save_path, dialog_owner, downloads_dir,
    hide_console, hide_std_console, integrate_title_bar, monitor_refresh_hz, open_folder,
};

pub const AZURE_CLI: &str = "az";
pub const HAS_SAVE_DIALOG: bool = false;
/// A clone's folder as Settings › Local checkouts shows it in its example.
pub const EXAMPLE_CLONE: &str = "/home/you/src/login-app";
/// The terminals Settings › Local checkouts offers, with their names.
pub const TERMINALS: &[(TerminalChoice, &str)] = &[(TerminalChoice::System, "xterm")];

/// The environment as started.
pub(crate) fn prepare_environment() {}

/// The system's window as it is.
pub fn viewport(builder: ViewportBuilder) -> ViewportBuilder {
    builder
}

/// Opens `xterm` in `dir` running `program` with `args`.
pub fn open_terminal(dir: &Path, program: &str, args: &[String]) -> std::io::Result<()> {
    spawn_reaped(Command::new("xterm").arg("-e").arg(program).args(args).current_dir(dir))
}

/// None: egui's bundled fonts serve the text.
pub(crate) fn font_sources() -> FontSources {
    FontSources::default()
}

/// No system setting the app reads.
pub(crate) fn prefers_reduced_motion() -> bool {
    false
}

/// Opens `xterm` in `dir`.
pub fn open_shell(dir: &Path, _terminal: TerminalChoice) -> std::io::Result<()> {
    spawn_reaped(Command::new("xterm").current_dir(dir))
}

/// No folder dialog here.
pub fn ask_folder(_owner: DialogOwner, _title: &str) -> Option<PathBuf> {
    None
}
