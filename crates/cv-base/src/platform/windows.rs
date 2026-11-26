//! Windows: the integrated title bar ([`caption`]), window placement ([`placement`]), activation at
//! start ([`startup`]), Segoe and Cascadia fonts from the Windows font folder, the client-area
//! animation setting, a waitable timer for the frame rate limit, the common Save As and folder
//! dialogs, Explorer and the terminals.
#![expect(
    unsafe_code,
    reason = "Win32 calls; each unsafe block makes one call and names what keeps it sound"
)]

mod app_mark;
mod caption;
mod caption_buttons;
pub mod hit_test;
mod placement;
mod startup;

use std::path::{Path, PathBuf};
use std::time::Duration;

use egui::{FontDefinitions, ViewportBuilder};
use review_store::checkouts::TerminalChoice;

#[cfg(any(test, feature = "test-support"))]
pub use self::caption::published as published_caption;
pub use self::caption::{Caption, integrate as integrate_title_bar};
pub use self::placement::PlacedWindow;
use super::{FontFace, FontSources, SaveRequest, VariableUiFont};

/// `az` is a batch file on Windows; only the `.cmd` name starts it without a shell.
pub const AZURE_CLI: &str = "az.cmd";
pub const HAS_SAVE_DIALOG: bool = true;
/// The terminals Settings › Local checkouts offers, with their names.
pub const TERMINALS: &[(TerminalChoice, &str)] = &[
    (TerminalChoice::System, "Windows Terminal"),
    (TerminalChoice::PowerShell, "PowerShell"),
    (TerminalChoice::GitBash, "Git Bash"),
];
/// Keeps console programs from flashing a console window in the GUI build.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Segoe UI Variable (Windows 11): optical size tuned for text.
const TEXT_OPTICAL_SIZE: f32 = 10.5;

/// The environment as started: GUI apps see the user's full `PATH` here.
pub fn prepare_environment() {}

/// The window keeps its native frame; [`integrate_title_bar`] takes the caption off once it exists.
/// It opens without taking the foreground when the launcher asked for that ([`startup`]).
pub fn viewport(builder: ViewportBuilder) -> ViewportBuilder {
    builder.with_active(startup::activates_first_window(startup::requested_show_command()))
}

/// Starts `command` without a console window.
pub fn hide_console(command: &mut tokio::process::Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

/// [`hide_console`] for a process run from a thread of its own.
pub fn hide_std_console(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// The user's Downloads folder.
pub fn downloads_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|profile| PathBuf::from(profile).join("Downloads"))
}

/// Shows `path` in Explorer; fails when Explorer does not start.
pub fn open_folder(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("explorer").arg(path).spawn().map(drop)
}

/// Opens a terminal in `dir` running `program` with `args`: Windows Terminal where installed, else
/// a console window. Both run it through `cmd /k`, which finds `.cmd` shims (npm installs) and
/// keeps the window open when it ends.
pub fn open_terminal(dir: &Path, program: &str, args: &[String]) -> std::io::Result<()> {
    let terminal = std::process::Command::new("wt.exe")
        .arg("-d")
        .arg(dir)
        .args(["cmd", "/k", program])
        .args(args)
        .spawn();
    match terminal {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::process::Command::new("cmd")
                .args(["/c", "start", "", "cmd", "/k", program])
                .args(args)
                .current_dir(dir)
                .spawn()
                .map(drop)
        }
        Err(error) => Err(error),
    }
}

/// Segoe UI Variable (static Segoe UI on Windows 10), Cascadia Mono (Consolas before Windows 11),
/// Segoe UI Emoji and Segoe UI Symbol.
pub fn font_sources() -> FontSources {
    let faces = |files: &[&str]| files.iter().map(|file| system_font(file)).collect::<Vec<_>>();
    FontSources {
        variable_ui: Some(VariableUiFont {
            face: system_font("SegUIVar.ttf"),
            optical_size: Some(TEXT_OPTICAL_SIZE),
        }),
        ui: faces(&["segoeui.ttf"]),
        semibold: faces(&["seguisb.ttf", "segoeuib.ttf"]),
        code: faces(&["CascadiaMono.ttf", "consola.ttf"]),
        emoji: faces(&["seguiemj.ttf"]),
        symbol: faces(&["seguisym.ttf"]),
    }
}

/// The caption buttons' glyph font.
pub fn register_chrome_fonts(fonts: &mut FontDefinitions) {
    caption_buttons::register_glyph_font(fonts);
}

/// `file` in the Windows font folder.
fn system_font(file: &str) -> FontFace {
    let windows =
        std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    FontFace { path: windows.join("Fonts").join(file), index: 0 }
}

/// "Animate controls and elements inside windows" is off (Settings → Accessibility → Visual
/// effects → Animation effects).
pub fn prefers_reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };
    let mut animations_enabled: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL into the pointed-to i32 that lives for the call.
    let is_read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&raw mut animations_enabled).cast(),
            0,
        )
    } != 0;
    is_read && animations_enabled == 0
}

/// Sleeps until a deadline, or until input arrives in this thread's queue.
#[derive(Default)]
pub struct FrameHold {
    /// High-resolution waitable timer, created on first use; `None` if it could not be.
    timer: Option<std::os::windows::io::OwnedHandle>,
    is_created: bool,
}

impl FrameHold {
    /// Keyboard, mouse, pen and touch messages; posted messages (repaint requests) and raw input
    /// (mouse movement anywhere on screen) don't end the hold.
    const WAKE_MASK: u32 = {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            QS_KEY, QS_MOUSEBUTTON, QS_MOUSEMOVE, QS_POINTER, QS_TOUCH,
        };
        QS_KEY | QS_MOUSEMOVE | QS_MOUSEBUTTON | QS_POINTER | QS_TOUCH
    };

    pub fn wait(&mut self, duration: Duration) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Threading::INFINITE;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx,
        };

        if !self.is_created {
            self.timer = create_high_resolution_timer();
            self.is_created = true;
        }
        let timer = self
            .timer
            .as_ref()
            .map(AsRawHandle::as_raw_handle)
            .filter(|timer| set_timer(*timer, duration));
        // MWMO_INPUTAVAILABLE also returns at once for input already queued but not yet handled.
        if let Some(timer) = timer {
            // SAFETY: one handle, a live waitable timer owned by `self` for the call.
            unsafe {
                MsgWaitForMultipleObjectsEx(
                    1,
                    &raw const timer,
                    INFINITE,
                    Self::WAKE_MASK,
                    MWMO_INPUTAVAILABLE,
                )
            };
        } else {
            // Without a timer the wait has the system tick's granularity, still no spinning.
            let milliseconds = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX).max(1);
            // SAFETY: no handles; the call only waits for the timeout or input.
            unsafe {
                MsgWaitForMultipleObjectsEx(
                    0,
                    std::ptr::null(),
                    milliseconds,
                    Self::WAKE_MASK,
                    MWMO_INPUTAVAILABLE,
                )
            };
        }
    }
}

fn create_high_resolution_timer() -> Option<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::FromRawHandle as _;
    use windows_sys::Win32::System::Threading::{
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, TIMER_ALL_ACCESS,
    };
    // SAFETY: no attributes and no name; the returned handle is owned from here on.
    let handle = unsafe {
        CreateWaitableTimerExW(
            std::ptr::null(),
            std::ptr::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS,
        )
    };
    // SAFETY: a non-null handle from CreateWaitableTimerExW is valid and owned by nobody else.
    (!handle.is_null())
        .then(|| unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle) })
}

/// Arms the timer to fire once after `duration` (relative due time in 100 ns units).
fn set_timer(timer: std::os::windows::io::RawHandle, duration: Duration) -> bool {
    use windows_sys::Win32::System::Threading::SetWaitableTimer;
    let intervals = i64::try_from(duration.as_nanos() / 100).unwrap_or(i64::MAX).max(1);
    let due = -intervals;
    // SAFETY: `timer` is a live waitable timer handle; no completion routine.
    unsafe { SetWaitableTimer(timer, &raw const due, 0, None, std::ptr::null(), 0) != 0 }
}

/// Refresh rate of the monitor the window is on, as the display settings report it.
pub fn monitor_refresh_hz(frame: &eframe::Frame) -> Option<u32> {
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use windows_sys::Win32::Graphics::Gdi::{
        DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW,
        MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromWindow,
    };
    let RawWindowHandle::Win32(handle) = frame.window_handle().ok()?.as_raw() else {
        return None;
    };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let mut mode =
        DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    // SAFETY: the handle is this app's live window.
    let monitor = unsafe { MonitorFromWindow(handle.hwnd.get() as _, MONITOR_DEFAULTTONEAREST) };
    // SAFETY: `info` carries its size as the API requires and lives for the call.
    let has_info = unsafe { GetMonitorInfoW(monitor, (&raw mut info).cast::<MONITORINFO>()) } != 0;
    // SAFETY: `szDevice` is the NUL-terminated name GetMonitorInfoW wrote; `mode` carries its size.
    let is_read = has_info
        && unsafe {
            EnumDisplaySettingsW(info.szDevice.as_ptr(), ENUM_CURRENT_SETTINGS, &raw mut mode)
        } != 0;
    // 0 and 1 mean "the hardware default", not a rate.
    (is_read && mode.dmDisplayFrequency > 1).then_some(mode.dmDisplayFrequency)
}

/// The window a modal dialog belongs to.
#[derive(Debug, Clone, Copy)]
pub struct DialogOwner(isize);

/// The window that has the keyboard focus on this thread; call on the UI thread.
pub fn dialog_owner() -> DialogOwner {
    // SAFETY: no arguments; returns a window handle or null.
    DialogOwner(unsafe {
        windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow() as isize
    })
}

/// The common Save As dialog, modal to `owner`; `None` when cancelled. Blocks until the user
/// decides, so it runs on a worker thread.
pub fn ask_save_path(owner: DialogOwner, request: &SaveRequest<'_>) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;

    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetSaveFileNameW, OFN_EXPLORER, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
        OPENFILENAMEW,
    };

    const MAX_PATH_CHARS: usize = 32_768;
    let wide = |text: &str| text.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let filter: Vec<u16> = format!(
        "{} (*.{extension})\0*.{extension}\0All files\0*.*\0\0",
        request.kind,
        extension = request.extension
    )
    .encode_utf16()
    .collect();
    let extension = wide(request.extension);
    let title = wide(request.title);
    let mut path = vec![0u16; MAX_PATH_CHARS];
    for (slot, unit) in
        path.iter_mut().zip(request.file_name.encode_utf16().take(MAX_PATH_CHARS - 1))
    {
        *slot = unit;
    }
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner.0 as _,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: path.as_mut_ptr(),
        nMaxFile: MAX_PATH_CHARS as u32,
        lpstrTitle: title.as_ptr(),
        lpstrDefExt: extension.as_ptr(),
        Flags: OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    // COM is initialised (and released) on this thread around the dialog.
    // SAFETY: no reserved pointer; the apartment model is a valid flag.
    let com = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    // SAFETY: every pointer in `dialog` points into buffers that outlive the call, and `path`
    // holds `nMaxFile` units.
    let is_chosen = unsafe { GetSaveFileNameW(&raw mut dialog) } != 0;
    if com >= 0 {
        // SAFETY: pairs the successful CoInitializeEx above, on the same thread.
        unsafe { CoUninitialize() };
    }
    let length = path.iter().position(|&unit| unit == 0).unwrap_or(path.len());
    is_chosen.then(|| PathBuf::from(std::ffi::OsString::from_wide(&path[..length])))
}

/// Opens `terminal` with a shell in `dir`: Windows Terminal (a console window where it is not
/// installed), PowerShell, or Git Bash from Git for Windows.
pub fn open_shell(dir: &Path, terminal: TerminalChoice) -> std::io::Result<()> {
    use std::process::Command;
    let started = match terminal {
        TerminalChoice::System => Command::new("wt.exe").arg("-d").arg(dir).spawn(),
        TerminalChoice::PowerShell => {
            Command::new("wt.exe").arg("-d").arg(dir).arg("powershell.exe").spawn()
        }
        TerminalChoice::GitBash => {
            let program_files = std::env::var_os("ProgramFiles")
                .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from);
            let mut cd = std::ffi::OsString::from("--cd=");
            cd.push(dir);
            let git_bash = program_files.join("Git").join("git-bash.exe");
            return Command::new(git_bash).arg(cd).spawn().map(drop);
        }
    };
    match started {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let shell =
                if terminal == TerminalChoice::PowerShell { "powershell.exe" } else { "cmd.exe" };
            Command::new("cmd").args(["/c", "start", "", shell]).current_dir(dir).spawn().map(drop)
        }
        Err(error) => Err(error),
    }
}

/// The folder picker, modal to `owner`, with `title` above the tree; `None` when cancelled.
/// Blocks until the user decides, so it runs on a worker thread.
pub fn ask_folder(owner: DialogOwner, title: &str) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;

    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    };
    use windows_sys::Win32::UI::Shell::{
        BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, SHBrowseForFolderW,
        SHGetPathFromIDListW,
    };

    const MAX_PATH_CHARS: usize = 32_768;
    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let mut name = vec![0u16; 260];
    let dialog = BROWSEINFOW {
        hwndOwner: owner.0 as _,
        pszDisplayName: name.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        ..Default::default()
    };
    // The new dialog style needs COM in an apartment on this thread, released after the dialog.
    // SAFETY: no reserved pointer; the apartment model is a valid flag.
    let com = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    // SAFETY: `dialog` points into `title` and `name` (260 units, MAX_PATH), which outlive the call.
    let list = unsafe { SHBrowseForFolderW(&raw const dialog) };
    let mut path = vec![0u16; MAX_PATH_CHARS];
    // SAFETY: `list` is the item list the dialog returned; `path` has room for any path.
    let is_chosen =
        !list.is_null() && unsafe { SHGetPathFromIDListW(list, path.as_mut_ptr()) } != 0;
    if !list.is_null() {
        // SAFETY: the dialog's item list comes from the COM allocator and is freed once, here.
        unsafe { CoTaskMemFree(list.cast_const().cast()) };
    }
    if com >= 0 {
        // SAFETY: pairs the successful CoInitializeEx above, on the same thread.
        unsafe { CoUninitialize() };
    }
    let length = path.iter().position(|&unit| unit == 0).unwrap_or(path.len());
    is_chosen.then(|| PathBuf::from(std::ffi::OsString::from_wide(&path[..length])))
}
