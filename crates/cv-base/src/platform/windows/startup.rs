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
#![expect(unsafe_code, reason = "one Win32 query of the process's own startup information")]

use windows_sys::Win32::System::Threading::{GetStartupInfoW, STARTF_USESHOWWINDOW, STARTUPINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SW_SHOWMINNOACTIVE, SW_SHOWNA, SW_SHOWNOACTIVATE,
};

/// The show command the launcher asked for, `None` when it left the choice to the app.
pub fn requested_show_command() -> Option<u16> {
    let mut info = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    // SAFETY: fills the local struct, which carries its size and lives for the call.
    unsafe { GetStartupInfoW(&raw mut info) };
    (info.dwFlags & STARTF_USESHOWWINDOW != 0).then_some(info.wShowWindow)
}

/// The first window comes to the front unless the launcher asked for a show without activation.
pub fn activates_first_window(show_command: Option<u16>) -> bool {
    let no_activation =
        [SW_SHOWNOACTIVATE, SW_SHOWMINNOACTIVE, SW_SHOWNA].map(|command| command as u16);
    !show_command.is_some_and(|command| no_activation.contains(&command))
}

#[cfg(test)]
mod tests {
    use super::activates_first_window;

    #[test]
    fn a_background_start_opens_the_window_without_activation() {
        // SW_SHOWNOACTIVATE (start-background.ps1), SW_SHOWMINNOACTIVE (its -Show 7), SW_SHOWNA.
        for command in [4, 7, 8] {
            assert!(!activates_first_window(Some(command)), "show command {command}");
        }
    }

    #[test]
    fn a_normal_start_brings_the_window_to_the_front() {
        // No STARTF_USESHOWWINDOW, SW_SHOWNORMAL (Explorer), SW_SHOWMAXIMIZED, SW_SHOWDEFAULT.
        assert!(activates_first_window(None));
        for command in [1, 3, 10] {
            assert!(activates_first_window(Some(command)), "show command {command}");
        }
    }
}
