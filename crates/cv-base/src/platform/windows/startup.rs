//! Whether the first window takes the foreground. The launcher says so through `STARTUPINFO`:
//! `scripts/start-background.ps1` asks for `SW_SHOWNOACTIVATE`, Explorer and the Start menu for a
//! normal show.
//!
//! winit ignores that request: eframe creates the window hidden and shows it after the first
//! frame with `ShowWindow(SW_SHOW)` (winit `window_state.rs`, `apply_diff`), and Windows applies
//! the `STARTUPINFO` show command only to a first `ShowWindow(SW_SHOWDEFAULT)`. A process started
//! by the foreground process tree may take the foreground, so `SW_SHOW` activated the window over
//! the agent host that launched it. `with_active(false)` makes winit show it with
//! `SW_SHOWNOACTIVATE` instead.
//!
//! The same answer decides whether the restored maximize of [`super::placement`] may activate the
//! hidden window.
#![expect(unsafe_code, reason = "one Win32 query of the process's own startup information")]

use windows_sys::Win32::System::Threading::{GetStartupInfoW, STARTF_USESHOWWINDOW, STARTUPINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SHOW_WINDOW_CMD, SW_HIDE, SW_MINIMIZE, SW_SHOWMINNOACTIVE, SW_SHOWNA, SW_SHOWNOACTIVATE,
};

/// How the launcher asked the first window to show (`STARTUPINFO.wShowWindow`), `None` when it
/// left the choice to the app.
pub(crate) fn launch_show_command() -> Option<SHOW_WINDOW_CMD> {
    let mut info = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    // SAFETY: fills the local struct, which carries its size and lives for the call.
    unsafe { GetStartupInfoW(&raw mut info) };
    (info.dwFlags & STARTF_USESHOWWINDOW != 0).then(|| SHOW_WINDOW_CMD::from(info.wShowWindow))
}

/// Whether the launcher wants the window active: as for a plain start (Explorer, a shortcut,
/// `Start-Process`) unless it named a show command that leaves the window inactive (a start in
/// the background, `scripts/start-background.ps1`).
pub(crate) fn launch_activates(show: Option<SHOW_WINDOW_CMD>) -> bool {
    !matches!(
        show,
        Some(SW_HIDE | SW_SHOWNOACTIVATE | SW_MINIMIZE | SW_SHOWMINNOACTIVE | SW_SHOWNA)
    )
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SW_HIDE, SW_MINIMIZE, SW_SHOW, SW_SHOWDEFAULT, SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE,
        SW_SHOWNA, SW_SHOWNOACTIVATE, SW_SHOWNORMAL,
    };

    use super::launch_activates;

    #[test]
    fn a_minimized_or_hidden_start_does_not_activate_the_first_window() {
        for show in [None, Some(SW_SHOWNORMAL), Some(SW_SHOW), Some(SW_SHOWDEFAULT)] {
            assert!(launch_activates(show), "{show:?}");
        }
        assert!(launch_activates(Some(SW_SHOWMAXIMIZED)));
        // `scripts/start-background.ps1` starts with SW_SHOWNOACTIVATE, or SW_SHOWMINNOACTIVE.
        for show in [SW_SHOWNOACTIVATE, SW_SHOWMINNOACTIVE, SW_SHOWNA, SW_MINIMIZE, SW_HIDE] {
            assert!(!launch_activates(Some(show)), "{show}");
        }
    }
}
