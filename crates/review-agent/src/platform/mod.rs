//! What starting an agent CLI needs from the operating system: which file on `PATH` is the
//! program, the `PATH` saved for new processes, and no console window for a child of a GUI
//! process on Windows. Windows reads the saved `PATH` from the registry, so a CLI installed after
//! the app started is found without signing out. Elsewhere (macOS included) a child process opens
//! no window, a program is any executable file and the process's own `PATH` is the one to search,
//! so [`other`] leaves the command as it is.

#[cfg(not(windows))]
mod other;
#[cfg(windows)]
mod windows;

#[cfg(not(windows))]
pub(crate) use other::{executable_in, hide_console, saved_paths};
#[cfg(windows)]
pub(crate) use windows::{executable_in, hide_console, saved_paths};
