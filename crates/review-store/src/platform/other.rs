//! Systems without a supported secret store (the BSDs and others): connections work without a
//! token only. Housekeeping keeps the normal priority, child processes open no window, data
//! follows the XDG defaults.

use std::io;
use std::path::PathBuf;
use std::process::Command;

use super::DataRoots;
use crate::token_vault::{LINUX_HELP_URL, VaultProblem};

pub(crate) const VAULT_SUMMARY: &str =
    "This system has no secret store Code View supports, so tokens cannot be saved.";

/// Refuses: a token is never written to disk unprotected.
pub(crate) fn protect_token(_account: &str, _token: &str) -> io::Result<String> {
    Err(unsupported().into())
}

pub(crate) fn unprotect_token(_account: &str, _stored: &str) -> io::Result<String> {
    Err(unsupported().into())
}

/// Nothing is kept outside the connections file.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on the other systems, where forgetting talks to a store"
)]
pub(crate) fn forget_token(_account: &str) -> io::Result<()> {
    Ok(())
}

pub(crate) fn check_vault() -> Result<(), VaultProblem> {
    Err(unsupported())
}

pub(crate) fn enter_background_priority() {}

pub(crate) fn hide_console(_command: &mut Command) {}

pub(crate) fn data_roots() -> DataRoots {
    let home = std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from);
    DataRoots {
        config: std::env::var_os("XDG_CONFIG_HOME")
            .map_or_else(|| home.join(".config"), PathBuf::from),
        cache: std::env::var_os("XDG_CACHE_HOME")
            .map_or_else(|| home.join(".cache"), PathBuf::from),
    }
}

fn unsupported() -> VaultProblem {
    VaultProblem {
        title: "Tokens cannot be saved on this system".to_owned(),
        fix: "Code View stores tokens in the system's secret store (DPAPI on Windows, the Keychain on macOS, the Secret \
              Service on Linux) and has none for this system. Save the connection without a token to browse public \
              pull requests."
            .to_owned(),
        commands: Vec::new(),
        help_url: LINUX_HELP_URL,
    }
}
