use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use serde::Serialize;
use serde::de::DeserializeOwned;

const TEMP_EXTENSION: &str = "tmp";

/// Gives every write its own temporary file within the process.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Writes `value` as pretty JSON next to `path` and renames it into place.
pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    write_bytes(path, &bytes)
}

/// Writes `bytes` to a temporary file next to `path` and renames it into place. Each write has
/// its own temporary file, so saves of one file at the same time each land whole, the last one
/// winning; a shared one would take a mix of them, and every rename after the first would fail.
pub fn write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path.file_name().map_or_else(Default::default, |name| name.to_string_lossy());
    let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let temp_path =
        path.with_file_name(format!("{name}.{}-{unique}.{TEMP_EXTENSION}", std::process::id()));
    std::fs::write(&temp_path, bytes)?;
    std::fs::rename(&temp_path, path).inspect_err(|_| {
        // Nothing reads the temporary file; a failed rename must not leave it behind.
        let _gone_or_locked = std::fs::remove_file(&temp_path);
    })
}

/// Reads JSON from `path`; a missing file yields `None`.
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let value = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Some(value))
}

/// Opening a pull request counts as use: its local state ages from the last visit, not the last write.
pub(crate) fn mark_used(path: &Path) {
    if let Ok(handle) = std::fs::File::options().write(true).open(path) {
        // A file that cannot take the time ages from its last write, which retention allows for.
        let _ages_from_last_write = handle.set_modified(SystemTime::now());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::{read_json, write_json};
    use crate::test_support::TestDir;

    /// Two tasks save the same file at once (a mirror's stamp from two pull request pages, say):
    /// every save succeeds and the file holds one of them whole, never a mix of both.
    #[test]
    fn saves_of_one_file_at_the_same_time_each_land_whole() {
        let dir = TestDir::new("atomic-concurrent");
        let path = Arc::new(dir.path().join("stamp.json"));
        let start = Arc::new(Barrier::new(4));
        let writers: Vec<_> = (0..4_usize)
            .map(|writer| {
                let (path, start) = (path.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    // Lengths differ per writer, so a mix of two writes does not parse.
                    let value = vec![writer; 200 * (writer + 1)];
                    (0..50).map(|_| write_json(&path, &value)).collect::<Result<Vec<()>, _>>()
                })
            })
            .collect();
        for writer in writers {
            writer.join().expect("the writer ran").expect("every save succeeds");
        }
        let saved: Vec<usize> = read_json(&path).expect("whole JSON").expect("saved");
        let writer = saved.first().copied().expect("not empty");
        assert_eq!(saved, vec![writer; 200 * (writer + 1)], "one save, whole");
    }
}
