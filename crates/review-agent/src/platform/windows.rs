#![expect(unsafe_code, reason = "the saved PATH is read from the registry through Win32 calls")]

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use tokio::process::Command;
use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The extensions `CreateProcess` starts directly (`.cmd` and `.bat` through `cmd.exe`). An npm
/// install puts `codex.cmd` next to `codex.ps1` and a POSIX `codex` script; only the first runs
/// without PowerShell or a POSIX shell.
const RUNNABLE: [&str; 4] = [".exe", ".com", ".cmd", ".bat"];
/// `PATHEXT` when the variable is not set, as `cmd.exe` assumes it.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";
/// Where Windows keeps the system's and the user's `PATH`, in the order it joins them for a new
/// sign-in.
const SAVED_PATHS: [(HKEY, &str); 2] = [
    (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
    (HKEY_CURRENT_USER, "Environment"),
];
/// A `%VARIABLE%` may expand to more than the size asked for first; one more try covers it.
const READ_TRIES: usize = 3;

/// Starts `command` without a console window.
pub(crate) fn hide_console(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

/// `name` in `dir` with the first `PATHEXT` extension that exists and starts directly.
pub(crate) fn executable_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_PATHEXT.to_owned());
    with_extension_in(dir, name, &pathext)
}

/// The system's and the user's `PATH` as the registry holds them now, `%VARIABLES%` expanded.
/// A process gets the `PATH` of whoever started it (Explorer, a browser, a terminal), which can
/// predate the install of a CLI; these hold what a terminal opened now would see.
pub(crate) fn saved_paths() -> Vec<OsString> {
    SAVED_PATHS.iter().filter_map(|(hive, key)| string_value(*hive, key, "Path")).collect()
}

fn with_extension_in(dir: &Path, name: &str, pathext: &str) -> Option<PathBuf> {
    pathext
        .split(';')
        .map(str::to_ascii_lowercase)
        .filter(|extension| RUNNABLE.contains(&extension.as_str()))
        .map(|extension| dir.join(format!("{name}{extension}")))
        .find(|candidate| candidate.is_file())
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// The string value `name` of `hive\key`; `None` when it is missing or not a string.
fn string_value(hive: HKEY, key: &str, name: &str) -> Option<OsString> {
    let key = wide(key);
    let name = wide(name);
    let mut data: Vec<u16> = Vec::new();
    for _ in 0..READ_TRIES {
        let mut size = u32::try_from(data.len() * 2).ok()?;
        let buffer = if data.is_empty() { std::ptr::null_mut() } else { data.as_mut_ptr().cast() };
        // SAFETY: `key` and `name` are NUL-terminated and outlive the call. `buffer` is null (the
        // call then writes only the size in bytes to `size`) or points into `data`, which holds
        // `size` bytes and outlives the call, which writes at most that many. RRF_RT_REG_SZ also
        // reads a REG_EXPAND_SZ value, expanded.
        let code = unsafe {
            RegGetValueW(
                hive,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer,
                &raw mut size,
            )
        };
        match code {
            ERROR_SUCCESS if !data.is_empty() => {
                let text = data.split(|unit| *unit == 0).next().unwrap_or_default();
                return Some(OsString::from_wide(text));
            }
            ERROR_SUCCESS | ERROR_MORE_DATA => {
                data = vec![0; usize::try_from(size).ok()? / 2 + 1];
            }
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::with_extension_in;

    #[test]
    fn picks_the_cmd_shim_of_an_npm_install_over_the_powershell_and_posix_scripts() {
        let dir = std::env::temp_dir().join(format!("review-agent-pathext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for file in ["codex", "codex.ps1", "codex.cmd"] {
            std::fs::write(dir.join(file), "").unwrap();
        }
        let pathext = ".COM;.EXE;.BAT;.CMD;.VBS;.JS;.PS1";

        let found = with_extension_in(&dir, "codex", pathext);
        let missing = with_extension_in(&dir, "claude", pathext);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(found, Some(dir.join("codex.cmd")));
        assert_eq!(missing, None);
    }
}
