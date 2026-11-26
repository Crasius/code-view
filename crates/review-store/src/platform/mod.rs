//! Everything the store needs from the operating system, one file per system with the same
//! surface:
//!
//! - `protect_token` / `unprotect_token` / `forget_token`: access tokens at rest. What
//!   `protect_token` returns is what the connections file keeps for the connection `account`;
//!   `forget_token` drops whatever the system keeps outside that file.
//! - `VAULT_SUMMARY` / `check_vault`: where tokens go, in one sentence, and whether one could be
//!   stored now (a [`crate::VaultProblem`] says how to fix it when not).
//! - `enter_background_priority`: the calling housekeeping thread yields to the window and git.
//! - `hide_console`: a git child process opens no console window.
//! - `data_roots`: the folders the app's config and cache folders go in.
//!
//! [`windows`] (DPAPI), [`macos`] (Keychain) and [`linux`] (Secret Service) are the supported
//! systems; [`other`] keeps the crate building elsewhere.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod other;
#[cfg(windows)]
mod windows;

use std::path::PathBuf;

#[cfg(target_os = "linux")]
use linux as system;
#[cfg(target_os = "macos")]
use macos as system;
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
use other as system;
#[cfg(windows)]
use windows as system;

pub(crate) use system::{
    VAULT_SUMMARY, check_vault, data_roots, enter_background_priority, forget_token, hide_console,
    protect_token, unprotect_token,
};

/// Where per-user app data goes: `config` follows the user (roams), `cache` stays on this machine.
pub(crate) struct DataRoots {
    pub config: PathBuf,
    pub cache: PathBuf,
}
