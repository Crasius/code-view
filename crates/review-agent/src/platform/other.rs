use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tokio::process::Command;

/// Child processes open no window here: nothing to hide.
pub(crate) fn hide_console(_command: &mut Command) {}

/// `dir/name` when it is a file someone may execute.
pub(crate) fn executable_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let candidate = dir.join(name);
    let metadata = std::fs::metadata(&candidate).ok()?;
    let is_executable = metadata.is_file() && metadata.permissions().mode() & 0o111 != 0;
    is_executable.then_some(candidate)
}

/// No saved `PATH` to read here: the process's own is the one a shell would see (the app adds
/// the tool folders a macOS GUI app misses at start-up).
pub(crate) fn saved_paths() -> Vec<OsString> {
    Vec::new()
}
