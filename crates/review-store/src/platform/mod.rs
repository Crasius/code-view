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

#[cfg(any(test, not(any(windows, target_os = "macos"))))]
use std::ffi::OsString;
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

/// The XDG base directories, for `linux` and `other`:`$XDG_CONFIG_HOME` (`~/.config`) and
/// `$XDG_CACHE_HOME` (`~/.cache`).
#[cfg(not(any(windows, target_os = "macos")))]
fn xdg_data_roots() -> DataRoots {
    let home = std::env::home_dir().unwrap_or_else(std::env::temp_dir);
    DataRoots {
        config: xdg_dir(std::env::var_os("XDG_CONFIG_HOME"), home.join(".config")),
        cache: xdg_dir(std::env::var_os("XDG_CACHE_HOME"), home.join(".cache")),
    }
}

/// The folder an XDG variable holds, or `fallback` while it is unset, empty or relative: the XDG
/// specification calls a relative path invalid and says to ignore it.
#[cfg(any(test, not(any(windows, target_os = "macos"))))]
fn xdg_dir(value: Option<OsString>, fallback: PathBuf) -> PathBuf {
    value.map(PathBuf::from).filter(|path| path.is_absolute()).unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::xdg_dir;

    #[test]
    fn an_xdg_folder_counts_only_when_it_is_absolute() {
        let fallback = PathBuf::from("home").join(".config");
        assert_eq!(xdg_dir(None, fallback.clone()), fallback, "unset");
        assert_eq!(xdg_dir(Some(OsString::new()), fallback.clone()), fallback, "empty");
        let relative = Some(OsString::from("relative/config"));
        assert_eq!(xdg_dir(relative, fallback.clone()), fallback, "relative");
        let absolute = std::env::temp_dir().join("xdg-config");
        assert_eq!(xdg_dir(Some(absolute.clone().into_os_string()), fallback), absolute);
    }
}
