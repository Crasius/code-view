//! Windows: tokens encrypted with DPAPI for the current user (the connections file is useless on
//! another account or machine), housekeeping in the thread background mode, git without a console
//! window, and the roaming and local application data folders.

#![expect(unsafe_code, reason = "DPAPI and the thread priority are Win32 FFI")]

use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::ptr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use super::DataRoots;
use crate::token_vault::VaultProblem;

pub(crate) const VAULT_SUMMARY: &str = "Tokens are stored encrypted for your Windows user (DPAPI). Code View never shows or logs them.";

/// DPAPI needs nothing running or unlocked.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on the other systems, whose stores can be missing"
)]
pub(crate) fn check_vault() -> Result<(), VaultProblem> {
    Ok(())
}

/// The DPAPI blob of `token`, base64; the account plays no part, the blob is bound to the user.
pub(crate) fn protect_token(_account: &str, token: &str) -> io::Result<String> {
    let encrypted = dpapi(token.as_bytes(), Direction::Protect)?;
    Ok(STANDARD.encode(encrypted))
}

pub(crate) fn unprotect_token(_account: &str, stored: &str) -> io::Result<String> {
    let encrypted = STANDARD
        .decode(stored)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let plain = dpapi(&encrypted, Direction::Unprotect)?;
    String::from_utf8(plain).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// The blob lives in the connections file only, so dropping the connection drops the token.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on the other systems, where forgetting talks to a store"
)]
pub(crate) fn forget_token(_account: &str) -> io::Result<()> {
    Ok(())
}

/// Lowest CPU priority and low I/O and memory priority for the calling thread, so deleting
/// gigabytes never competes with the window or with git.
pub(crate) fn enter_background_priority() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
    };
    // SAFETY: no preconditions; it returns a pseudo handle for the calling thread.
    let thread = unsafe { GetCurrentThread() };
    // SAFETY: the pseudo handle of the calling thread is always valid; a failure only keeps the
    // normal priority.
    unsafe {
        SetThreadPriority(thread, THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// Starts `command` without a console window.
pub(crate) fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// `%APPDATA%` roams with the user profile, `%LOCALAPPDATA%` stays on this machine.
pub(crate) fn data_roots() -> DataRoots {
    let home = std::env::home_dir().unwrap_or_else(std::env::temp_dir);
    DataRoots {
        config: std::env::var_os("APPDATA").map_or_else(|| home.join(".config"), PathBuf::from),
        cache: std::env::var_os("LOCALAPPDATA").map_or_else(|| home.join(".cache"), PathBuf::from),
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Protect,
    Unprotect,
}

fn dpapi(input: &[u8], direction: Direction) -> io::Result<Vec<u8>> {
    let length = u32::try_from(input.len()).map_err(io::Error::other)?;
    let input_blob = CRYPT_INTEGER_BLOB { cbData: length, pbData: input.as_ptr().cast_mut() };
    let mut output_blob = CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() };
    let succeeded = match direction {
        // SAFETY: both blobs point to valid memory for the duration of the call; DPAPI allocates
        // the output with LocalAlloc, which is released below with LocalFree.
        Direction::Protect => unsafe {
            CryptProtectData(
                &raw const input_blob,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output_blob,
            )
        },
        // SAFETY: as for `Protect`.
        Direction::Unprotect => unsafe {
            CryptUnprotectData(
                &raw const input_blob,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output_blob,
            )
        },
    };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success DPAPI returns `cbData` readable bytes at `pbData`.
    let output =
        unsafe { std::slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize) }
            .to_vec();
    // SAFETY: `pbData` was allocated by DPAPI with LocalAlloc.
    unsafe { LocalFree(output_blob.pbData.cast()) };
    Ok(output)
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trips_through_dpapi() {
        let protected = super::protect_token("gh-1", "example-value").unwrap();
        assert_ne!(protected, "example-value");
        assert_eq!(super::unprotect_token("gh-1", &protected).unwrap(), "example-value");
    }
}
