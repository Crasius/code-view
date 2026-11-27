//! Everything the app does differently per operating system, behind one surface. Feature code
//! calls `platform::…` and never checks the system itself; each system's file implements the same
//! functions and types:
//!
//! - Start-up: [`prepare_environment`] (first in `main`; it also drops an agent session's markers,
//!   `session`) and [`viewport`] (the window's style).
//! - Processes: [`hide_console`] (and [`hide_std_console`]) for command-line tools, [`AZURE_CLI`]
//!   (the Azure CLI's executable), [`downloads_dir`], [`open_folder`], [`open_terminal`] (the
//!   user's terminal running a command, for the assistant's CLIs), [`open_shell`] and
//!   [`TERMINALS`] (a terminal in a checkout's folder), [`EXAMPLE_CLONE`] (a clone's folder for
//!   examples).
//! - Text, for cv-base's theme only: `font_sources` (the system's UI, code, emoji and symbol
//!   fonts) and `register_chrome_fonts` (fonts the window chrome draws with).
//! - Motion, for cv-base's theme only: `prefers_reduced_motion`.
//! - The window: [`integrate_title_bar`] and [`Caption`] (the app's bar as the window's title bar),
//!   [`PlacedWindow`] (restoring the window's placement), [`FrameHold`] (the frame rate limit's
//!   wait), [`monitor_refresh_hz`], [`is_window_active`] (whether refreshes and animations run).
//! - Dialogs: [`HAS_SAVE_DIALOG`], [`dialog_owner`], [`ask_save_path`], [`ask_folder`].
//! - Keys: [`shortcut!`] (a shortcut's label), [`FOCUS_MODE_KEY`], [`DIFF_FIND_KEY`],
//!   [`BACK_SHORTCUT`] and [`FORWARD_SHORTCUT`], all in `keys`.
//! - Gestures: [`page_swipe`], the trackpad's swipe between pages.
//! - Pointer: [`pointer`](mod@pointer), the pointing hand over everything clickable (every system).
//! - Graphics: [`gl`], the OpenGL driver's strings (the same on every system).
//! - Memory: [`heap`], the global allocator that counts the heap's live bytes (every system).
//!
//! `windows`, `macos` and `linux` are the supported systems; `other` keeps the app
//! building and running elsewhere (the BSDs) with the system's own title bar and no integration.
//! `portable` holds the parts the systems other than Windows share, `window_drag` the egui drag
//! area of a title bar the system cannot hit-test and [`long_press_moves_window`] for widgets on
//! the bar (the breadcrumbs) that move the window when held.

pub mod gl;
pub mod heap;
mod keys;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod other;
pub mod pointer;
#[cfg(not(windows))]
mod portable;
mod session;
mod window_drag;
#[cfg(windows)]
mod windows;

use std::path::PathBuf;

pub use crate::shortcut;
pub use keys::{BACK_SHORTCUT, DIFF_FIND_KEY, FOCUS_MODE_KEY, FORWARD_SHORTCUT, consume_plain};
#[cfg(target_os = "linux")]
use linux as system;
#[cfg(target_os = "macos")]
use macos as system;
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
use other as system;
#[cfg(windows)]
use windows as system;

#[cfg(target_os = "macos")]
pub use macos::page_swipe;
pub use system::{
    AZURE_CLI, Caption, EXAMPLE_CLONE, FrameHold, HAS_SAVE_DIALOG, PlacedWindow, TERMINALS,
    ask_folder, ask_save_path, dialog_owner, downloads_dir, hide_console, hide_std_console,
    integrate_title_bar, monitor_refresh_hz, open_folder, open_shell, open_terminal, viewport,
};
pub(crate) use system::{font_sources, prefers_reduced_motion, register_chrome_fonts};
#[cfg(any(test, feature = "test-support"))]
pub use window_drag::LONG_PRESS;
pub(crate) use window_drag::is_drag_area as is_window_drag_area;
pub use window_drag::long_press_moves_window;
/// The Windows title bar's state for the tests of the integrated bar.
#[cfg(all(windows, any(test, feature = "test-support")))]
pub use windows::{hit_test, published_caption};

/// Drops the markers of a coding agent session the app was started from (`session`), then
/// prepares what the system needs. Call first in `main`, before any other thread runs.
pub fn prepare_environment() {
    session::forget_parent_session();
    system::prepare_environment();
}

/// The window is focused, not minimized and not covered. Unknown counts as active (integrations
/// without the info). Automatic refreshes and animations pause while it is not.
pub fn is_window_active(viewport: &egui::ViewportInfo) -> bool {
    viewport.focused.unwrap_or(true) && viewport.visible().unwrap_or(true)
}

/// The trackpad's two-finger swipe between pages, where the system has it on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSwipe {
    /// Content moves with the fingers ("natural scrolling"), so scroll deltas follow the fingers.
    pub is_natural: bool,
}

/// Only macOS turns a two-finger swipe into a page step; elsewhere the travel just scrolls.
#[cfg(not(target_os = "macos"))]
pub fn page_swipe() -> Option<PageSwipe> {
    None
}

/// One font face on disk: the file and the face's index in it (a `.ttc` collection holds several).
#[derive(Debug, Clone)]
pub struct FontFace {
    pub path: PathBuf,
    pub index: u32,
}

/// A variable font for the UI text: one file serves the regular and the semibold weight.
#[derive(Debug, Clone)]
pub struct VariableUiFont {
    pub face: FontFace,
    /// The `opsz` axis value for text, for fonts whose default instance is tuned for display sizes.
    pub optical_size: Option<f32>,
}

/// The system fonts the app's text uses; in each list the first face that can be read wins.
#[derive(Debug, Clone, Default)]
pub struct FontSources {
    /// Tried first; without it the static `ui` and `semibold` faces are used.
    pub variable_ui: Option<VariableUiFont>,
    pub ui: Vec<FontFace>,
    pub semibold: Vec<FontFace>,
    pub code: Vec<FontFace>,
    pub emoji: Vec<FontFace>,
    pub symbol: Vec<FontFace>,
}

/// What a Save As dialog asks for.
pub struct SaveRequest<'a> {
    pub title: &'a str,
    /// Suggested name, extension included.
    pub file_name: &'a str,
    /// The file type's name in the type list ("Log files").
    pub kind: &'a str,
    /// Without the dot.
    pub extension: &'a str,
}
