//! What running `git` needs from the operating system. Windows hides the console window a console
//! program would open from a GUI process; elsewhere (macOS included) a child process opens no
//! window, so [`other`] leaves the command as it is.

#[cfg(not(windows))]
mod other;
#[cfg(windows)]
mod windows;

#[cfg(not(windows))]
pub(crate) use other::hide_console;
#[cfg(windows)]
pub(crate) use windows::hide_console;
