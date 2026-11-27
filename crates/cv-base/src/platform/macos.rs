//! macOS: the app's bar as the title bar around the native traffic lights (a full-size content
//! view with a transparent title bar, moved through a [`DragArea`]), San Francisco and SF Mono or
//! Menlo from the system font folder, the Reduce motion setting, the screen's refresh rate, the
//! Save dialog through AppleScript, the Finder through `open` and the trackpad's swipe between
//! pages ([`page_swipe`]).
//!
//! The window opens centered at its default size ([`PlacedWindow`] restores nothing here), and the
//! frame rate limit sleeps ([`FrameHold`]).
#![expect(
    unsafe_code,
    reason = "AppKit's view handle and the start-up PATH; each unsafe block makes one call or read"
)]

mod traffic_lights;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;

use egui::{Rangef, Rect, Ui, ViewportBuilder};
use objc2::rc::Weak;
use objc2_app_kit::{NSView, NSWindow};
use review_store::checkouts::TerminalChoice;

use super::portable::spawn_reaped;
pub use super::portable::{
    DialogOwner, FrameHold, PlacedWindow, dialog_owner, downloads_dir, hide_console,
    hide_std_console, register_chrome_fonts,
};
use super::window_drag::DragArea;
use super::{FontFace, FontSources, PageSwipe, SaveRequest, VariableUiFont};

thread_local! {
    // Do not keep a closed window alive. Native view access stays on the UI thread.
    static CAPTION_WINDOW: RefCell<Option<Weak<NSWindow>>> = const { RefCell::new(None) };
}

/// Found through the `PATH` that [`prepare_environment`] completes.
pub const AZURE_CLI: &str = "az";
pub const HAS_SAVE_DIALOG: bool = true;
/// A clone's folder as Settings › Local checkouts shows it in its example.
pub const EXAMPLE_CLONE: &str = "/home/you/src/login-app";
/// The terminals Settings › Local checkouts offers, with their names.
pub const TERMINALS: &[(TerminalChoice, &str)] = &[(TerminalChoice::System, "Terminal")];
/// Where command-line tools live that an app started from the Finder or the Dock does not see:
/// GUI apps get the minimal `/usr/bin:/bin:/usr/sbin:/sbin` (Homebrew on Apple silicon, Homebrew
/// on Intel, MacPorts).
const TOOL_DIRS: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"];
const FONT_DIR: &str = "/System/Library/Fonts";

/// Adds [`TOOL_DIRS`] to `PATH`, so `gh`, `az` and a Homebrew `git` start from the Dock as they do
/// from a terminal. Call first in `main`, before any other thread runs.
pub fn prepare_environment() {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&current).collect();
    for dir in TOOL_DIRS {
        if !dirs.iter().any(|known| known == Path::new(dir)) {
            dirs.push(PathBuf::from(dir));
        }
    }
    if let Ok(path) = std::env::join_paths(dirs) {
        // SAFETY: called at the start of `main`, while no other thread reads the environment.
        unsafe { std::env::set_var("PATH", path) };
    }
}

/// The content reaches under the title bar, which keeps only the traffic lights: the app's bar is
/// the title bar ([`Caption`]).
pub fn viewport(builder: ViewportBuilder) -> ViewportBuilder {
    builder.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false)
}

/// Reveals `path` in the Finder; fails when `open` does not start.
pub fn open_folder(path: &Path) -> std::io::Result<()> {
    spawn_reaped(Command::new("open").arg(path))
}

/// Opens Terminal in `dir` running `program` with `args`, through AppleScript.
pub fn open_terminal(dir: &Path, program: &str, args: &[String]) -> std::io::Result<()> {
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let mut line = format!("cd {} && {}", quote(&dir.to_string_lossy()), quote(program));
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    // An AppleScript string literal escapes backslashes and double quotes.
    let script_text = line.replace('\\', r"\\").replace('"', "\\\"");
    spawn_reaped(
        Command::new("osascript")
            .args(["-e", &format!("tell application \"Terminal\" to do script \"{script_text}\"")])
            .args(["-e", "tell application \"Terminal\" to activate"]),
    )
}

/// San Francisco (variable; Helvetica where it is missing), SF Mono where installed, else Menlo or
/// Monaco, and Apple Symbols. No emoji face: Apple Color Emoji holds bitmaps only, which egui does
/// not draw; egui's bundled emoji font stays the fallback.
pub fn font_sources() -> FontSources {
    let face = |file: &str, index: u32| FontFace { path: Path::new(FONT_DIR).join(file), index };
    FontSources {
        variable_ui: Some(VariableUiFont { face: face("SFNS.ttf", 0), optical_size: None }),
        ui: vec![face("Helvetica.ttc", 0)],
        semibold: vec![face("Helvetica.ttc", 1)],
        code: vec![face("SFNSMono.ttf", 0), face("Menlo.ttc", 0), face("Monaco.ttf", 0)],
        emoji: Vec::new(),
        symbol: vec![face("Apple Symbols.ttf", 0)],
    }
}

/// "Reduce motion" is on (System Settings → Accessibility → Display).
pub fn prefers_reduced_motion() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// The global default behind System Settings → Trackpad → Natural scrolling (and the mouse's).
const NATURAL_SCROLLING_KEY: &str = "com.apple.swipescrolldirection";

/// "Swipe between pages" is set to scroll left or right with two fingers (System Settings →
/// Trackpad → More Gestures); `None` when it is off or set to three fingers, which reach the app as
/// no event winit forwards. Natural scrolling is on unless the user turned it off, so a missing
/// key reads as on.
pub fn page_swipe() -> Option<PageSwipe> {
    if !objc2_app_kit::NSEvent::isSwipeTrackingFromScrollEventsEnabled() {
        return None;
    }
    let defaults = objc2_foundation::NSUserDefaults::standardUserDefaults();
    let key = objc2_foundation::NSString::from_str(NATURAL_SCROLLING_KEY);
    let is_natural = defaults.objectForKey(&key).is_none() || defaults.boolForKey(&key);
    Some(PageSwipe { is_natural })
}

/// True for an AppKit window: [`viewport`] already gave it the transparent title bar.
pub fn integrate_title_bar(creation: &eframe::CreationContext<'_>) -> bool {
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    if objc2::MainThreadMarker::new().is_none() {
        return false;
    }
    let Ok(handle) = creation.window_handle() else { return false };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { return false };
    // SAFETY: eframe's borrowed AppKit handle refers to a live NSView; the creation context
    // keeps it alive here, and the main-thread check above allows AppKit access.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    let Some(window) = view.window() else { return false };
    CAPTION_WINDOW.with(|slot| *slot.borrow_mut() = Some(Weak::new(&window)));
    true
}

/// The window's own parts of the title bar: the system's traffic lights at the left end (hidden in
/// full screen), and the bar's free parts as the drag area.
pub struct Caption {
    bar: Rect,
    inset: f32,
    drag: DragArea,
}

impl Caption {
    /// Senses the pointer on `bar` ahead of the widgets laid out on it afterwards. There is no app
    /// icon in a macOS title bar, so `has_icon` changes nothing.
    pub fn show(ui: &mut Ui, bar: Rect, _has_icon: bool) -> Self {
        let is_full_screen = ui.input(|input| input.viewport().fullscreen.unwrap_or(false));
        let zoom = ui.ctx().zoom_factor();
        let inset = if is_full_screen {
            0.0
        } else {
            CAPTION_WINDOW
                .with(|slot| {
                    slot.borrow().as_ref().and_then(Weak::load).map(|window| {
                        traffic_lights::align(
                            &window,
                            f64::from(bar.bottom() * zoom),
                            f64::from(bar.center().y * zoom),
                        ) as f32
                            / zoom
                    })
                })
                .unwrap_or(traffic_lights::FALLBACK_INSET as f32 / zoom)
        };
        Self { bar, inset, drag: DragArea::sense(ui, bar) }
    }

    /// The part of the bar the app's content may take: after the traffic lights.
    pub fn free(&self) -> Rangef {
        Rangef::new(self.bar.left() + self.inset, self.bar.right())
    }

    /// Dragging the bar moves the window and a double click zooms it, except over `holes` (the
    /// content's widgets).
    pub fn publish(self, _ctx: &egui::Context, holes: &[Rect]) {
        self.drag.apply(holes);
    }
}

/// The highest refresh rate of the screen with the key window (ProMotion screens report 120).
pub fn monitor_refresh_hz(_frame: &eframe::Frame) -> Option<u32> {
    let main_thread = objc2::MainThreadMarker::new()?;
    let screen = objc2_app_kit::NSScreen::mainScreen(main_thread)?;
    u32::try_from(screen.maximumFramesPerSecond()).ok().filter(|hz| *hz > 0)
}

/// The standard Save dialog, through AppleScript's `choose file name`; `None` when cancelled.
/// Blocks until the user decides, so it runs on a worker thread. The title and name go in as
/// arguments, never as script text.
#[expect(
    clippy::disallowed_methods,
    reason = "the dialog blocks until the user decides; callers run it on a worker thread"
)]
pub fn ask_save_path(_owner: DialogOwner, request: &SaveRequest<'_>) -> Option<PathBuf> {
    const SCRIPT: [&str; 4] = [
        "on run argv",
        "set chosen to choose file name with prompt (item 1 of argv) default name (item 2 of argv)",
        "return POSIX path of chosen",
        "end run",
    ];
    let mut command = Command::new("osascript");
    for line in SCRIPT {
        command.arg("-e").arg(line);
    }
    let output = command.arg(request.title).arg(request.file_name).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let chosen = String::from_utf8(output.stdout).ok()?;
    let mut path = PathBuf::from(chosen.trim_end_matches('\n'));
    if path.as_os_str().is_empty() {
        return None;
    }
    // Like the Windows dialog's default extension: a name typed without one gets it.
    if path.extension().is_none() {
        path.set_extension(request.extension);
    }
    Some(path)
}

/// Opens Terminal in `dir`; macOS has no other terminal to choose.
pub fn open_shell(dir: &Path, _terminal: TerminalChoice) -> std::io::Result<()> {
    spawn_reaped(Command::new("open").args(["-a", "Terminal"]).arg(dir))
}

/// The folder chooser through AppleScript; `None` when cancelled. Blocks, so it runs on a worker
/// thread.
#[expect(
    clippy::disallowed_methods,
    reason = "the dialog blocks until the user decides; callers run it on a worker thread"
)]
pub fn ask_folder(_owner: DialogOwner, title: &str) -> Option<PathBuf> {
    const SCRIPT: [&str; 4] = [
        "on run argv",
        "set chosen to choose folder with prompt (item 1 of argv)",
        "return POSIX path of chosen",
        "end run",
    ];
    let mut command = Command::new("osascript");
    for line in SCRIPT {
        command.arg("-e").arg(line);
    }
    let output = command.arg(title).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let chosen = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(chosen.trim_end_matches('\n'));
    (!path.as_os_str().is_empty()).then_some(path)
}
