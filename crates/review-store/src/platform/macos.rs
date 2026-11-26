//! macOS: tokens in the login Keychain (one generic password per connection, so Keychain Access
//! shows and removes them), housekeeping at the background quality of service, and the
//! Application Support and Caches folders of the user's Library.

#![expect(unsafe_code, reason = "the thread's quality of service is set through libSystem FFI")]

use std::io;
use std::path::PathBuf;
use std::process::Command;

use security_framework::passwords::{
    PasswordOptions, delete_generic_password, generic_password, set_generic_password,
};

use super::DataRoots;
use crate::token_vault::VaultProblem;

pub(crate) const VAULT_SUMMARY: &str =
    "Tokens are stored in your login Keychain. Code View never shows or logs them.";

/// The Keychain item's service ("Where" in Keychain Access); tests keep to their own items.
const SERVICE: &str = if cfg!(test) { "Code View (tests)" } else { "Code View" };
/// What the connections file keeps for a token that lives in the Keychain.
const IN_KEYCHAIN: &str = "keychain";
/// `errSecItemNotFound`.
const ITEM_NOT_FOUND: i32 = -25300;
/// `QOS_CLASS_BACKGROUND` from `<sys/qos.h>`.
const QOS_CLASS_BACKGROUND: u32 = 0x09;

unsafe extern "C" {
    /// libSystem: sets the calling thread's quality of service class.
    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
}

/// Stores `token` as the Keychain item of the connection `account`; the file keeps a marker.
pub(crate) fn protect_token(account: &str, token: &str) -> io::Result<String> {
    set_generic_password(SERVICE, account, token.as_bytes()).map_err(io::Error::other)?;
    Ok(IN_KEYCHAIN.to_owned())
}

/// The token of `account` from the Keychain. A value that is no marker came from another
/// system's store (a connections file copied from Windows) and cannot be read here.
pub(crate) fn unprotect_token(account: &str, stored: &str) -> io::Result<String> {
    if stored != IN_KEYCHAIN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the token was protected on another system; add it again",
        ));
    }
    let bytes = generic_password(PasswordOptions::new_generic_password(SERVICE, account))
        .map_err(io::Error::other)?;
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Deletes the Keychain item of `account`; one that is not there is fine.
pub(crate) fn forget_token(account: &str) -> io::Result<()> {
    match delete_generic_password(SERVICE, account) {
        Err(error) if error.code() != ITEM_NOT_FOUND => Err(io::Error::other(error)),
        _ => Ok(()),
    }
}

/// The login Keychain is always there; a locked one asks for the password when written.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on the other systems, whose stores can be missing"
)]
pub(crate) fn check_vault() -> Result<(), VaultProblem> {
    Ok(())
}

/// The background `QoS` class: lowest CPU priority, throttled disk I/O, so deleting gigabytes never
/// competes with the window or with git.
pub(crate) fn enter_background_priority() {
    // SAFETY: plain call on the current thread; a failure only keeps the default class.
    unsafe {
        pthread_set_qos_class_self_np(QOS_CLASS_BACKGROUND, 0);
    }
}

/// Child processes open no window here: nothing to hide.
pub(crate) fn hide_console(_command: &mut Command) {}

/// `~/Library/Application Support` for the config, `~/Library/Caches` for the cache.
pub(crate) fn data_roots() -> DataRoots {
    let home = std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from);
    let library = home.join("Library");
    DataRoots { config: library.join("Application Support"), cache: library.join("Caches") }
}

#[cfg(test)]
mod tests {
    /// The login Keychain is the user's own: a test binary that reads an item another build wrote
    /// waits on an access prompt. `cargo test -p review-store -- --ignored` checks it on a Mac.
    #[test]
    #[ignore = "writes to the login Keychain; run with --ignored"]
    fn round_trips_through_the_keychain() {
        let account = format!("round-trip-{}", std::process::id());
        let stored = super::protect_token(&account, "example-value").unwrap();
        assert_ne!(stored, "example-value");
        assert_eq!(super::unprotect_token(&account, &stored).unwrap(), "example-value");
        super::forget_token(&account).unwrap();
        assert!(super::unprotect_token(&account, &stored).is_err(), "the item is gone");
        super::forget_token(&account).unwrap();
    }
}
