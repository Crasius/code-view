//! The current user's registry (`HKEY_CURRENT_USER`), for the browsers' native messaging keys:
//! each key's default value, a string. Nothing here touches another hive.
#![expect(unsafe_code, reason = "the registry is reached through Win32 calls")]

use std::io;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyW, RegGetValueW, RegSetKeyValueW,
};

use crate::browser::registration::KeyStore;

pub(crate) struct UserKeys;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn checked(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX)))
    }
}

impl KeyStore for UserKeys {
    fn value(&self, key: &str) -> io::Result<Option<String>> {
        let key = wide(key);
        let mut size = 0_u32;
        // SAFETY: `key` is NUL-terminated; a null data pointer asks only for the size in bytes,
        // which the call writes to `size`. A null value name reads the default value.
        let code = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut size,
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        checked(code)?;
        let mut data = vec![0_u16; usize::try_from(size).unwrap_or_default() / 2 + 1];
        let mut size = u32::try_from(data.len() * 2).unwrap_or(u32::MAX);
        // SAFETY: `data` holds `size` bytes and outlives the call, which writes at most that many
        // and the byte count it wrote to `size`.
        let code = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                data.as_mut_ptr().cast(),
                &raw mut size,
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        checked(code)?;
        let text: Vec<u16> = data.into_iter().take_while(|unit| *unit != 0).collect();
        Ok(Some(String::from_utf16_lossy(&text)))
    }

    fn set(&self, key: &str, value: &str) -> io::Result<()> {
        let key = wide(key);
        let value = wide(value);
        let bytes = u32::try_from(value.len() * 2).map_err(io::Error::other)?;
        // SAFETY: `key` and `value` are NUL-terminated and outlive the call; `bytes` is the size
        // of `value` including its NUL, as REG_SZ wants it. The call creates a missing key.
        let code = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                std::ptr::null(),
                REG_SZ,
                value.as_ptr().cast(),
                bytes,
            )
        };
        checked(code)
    }

    fn delete(&self, key: &str) -> io::Result<()> {
        let key = wide(key);
        // SAFETY: `key` is NUL-terminated and outlives the call; it deletes only that key, which
        // has no subkeys.
        let code = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, key.as_ptr()) };
        if code == ERROR_FILE_NOT_FOUND { Ok(()) } else { checked(code) }
    }
}
