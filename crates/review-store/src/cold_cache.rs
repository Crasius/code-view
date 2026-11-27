//! The disk tier behind the app's memory caches: results that are expensive to get again (a host
//! answer after a restart, the colors of a large diff) are kept as one file each under
//! `cache/cold/<kind>/`, within one byte budget for all kinds, the least recently used going
//! first.
//!
//! - Each file names its format, the kind's version and its full key; anything else (an older
//!   version, a hash collision, a torn or damaged file) reads as a miss and is deleted.
//! - Writes go to a temporary file renamed into place, so a reader never sees half a file.
//! - Reading marks a file as used, so the budget keeps what is read and drops what is not.
//! - The budget holds after every write and at startup ([`ColdCache::enforce_budget`]).
//!
//! It never holds access tokens, cookies or credentials: callers store host answers and derived
//! results, never requests. Every call touches the disk, so call it off the UI thread.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::atomic_file::{self, TEMP_EXTENSION};
use crate::{AppPaths, stable_hash};

/// The cold tier's bytes on disk, for every kind together: our number, the lists and overviews of
/// about a hundred pull requests and the colors of a few dozen large diffs. Small next to the git
/// mirrors (gigabytes), and a miss only costs what the cache saves.
pub const BUDGET_BYTES: u64 = 64 * 1024 * 1024;
/// The container format; a new layout of the header changes it.
const MAGIC: &[u8; 8] = b"CVCOLD1\n";
const FILE_EXTENSION: &str = "bin";
/// A temporary file older than this was left by a write that did not finish; one in flight is
/// younger.
const STALE_TEMP: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct ColdCache {
    root: PathBuf,
    budget: u64,
}

impl ColdCache {
    /// The cold tier in `paths`' cache folder, kept within `budget` bytes.
    pub fn new(paths: &AppPaths, budget: u64) -> Self {
        Self { root: paths.cold_cache_dir(), budget }
    }

    /// The payload stored for `key` of `kind` at `version`, marked as used; `None` when there is
    /// none or it does not read as written.
    pub fn read(&self, kind: &str, version: u32, key: &str) -> Option<Vec<u8>> {
        let path = self.path(kind, key);
        let bytes = std::fs::read(&path).ok()?;
        let Some((stored, start)) = parse(&bytes, version) else {
            // Another version or a damaged file: it would never read, so it only takes room.
            let _gone_or_locked = std::fs::remove_file(&path);
            return None;
        };
        // Another key whose name hashes alike keeps its file.
        if stored != key.as_bytes() {
            return None;
        }
        atomic_file::mark_used(&path);
        Some(bytes[start..].to_vec())
    }

    /// The payloads of `kind` at `version` with their keys, the most recently used first, until
    /// `max_bytes` are read; none is marked as used. Files of another version or damaged ones are
    /// deleted.
    pub fn read_all(&self, kind: &str, version: u32, max_bytes: u64) -> Vec<(String, Vec<u8>)> {
        let Ok(entries) = std::fs::read_dir(self.root.join(kind)) else {
            return Vec::new();
        };
        let mut files: Vec<(SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == FILE_EXTENSION))
            .map(|entry| {
                let used = entry.metadata().and_then(|metadata| metadata.modified());
                (used.unwrap_or(SystemTime::UNIX_EPOCH), entry.path())
            })
            .collect();
        files.sort_by_key(|(used, _)| std::cmp::Reverse(*used));
        let mut found = Vec::new();
        let mut read: u64 = 0;
        for (_, path) in files {
            if read >= max_bytes {
                break;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            read += bytes.len() as u64;
            let parsed = parse(&bytes, version).and_then(|(key, start)| {
                Some((String::from_utf8(key.to_vec()).ok()?, bytes[start..].to_vec()))
            });
            match parsed {
                Some(entry) => found.push(entry),
                None => {
                    let _gone_or_locked = std::fs::remove_file(&path);
                }
            }
        }
        found
    }

    /// Stores `payload` for `key` of `kind` at `version`, replacing what was there, then drops
    /// the least recently used files beyond the budget.
    pub fn write(&self, kind: &str, version: u32, key: &str, payload: &[u8]) -> io::Result<()> {
        let path = self.path(kind, key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let key_length = u32::try_from(key.len()).map_err(io::Error::other)?;
        let mut bytes = Vec::with_capacity(MAGIC.len() + 8 + key.len() + payload.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&key_length.to_le_bytes());
        bytes.extend_from_slice(key.as_bytes());
        bytes.extend_from_slice(payload);
        atomic_file::write_bytes(&path, &bytes)?;
        self.enforce_budget();
        Ok(())
    }

    /// Deletes the least recently used files until the rest fit the budget, and temporary files
    /// a write left behind.
    pub fn enforce_budget(&self) {
        let mut files = self.files();
        let mut total: u64 = files.iter().map(|file| file.bytes).sum();
        files.sort_by_key(|file| file.used);
        for file in files {
            if total <= self.budget {
                break;
            }
            if std::fs::remove_file(&file.path).is_ok() {
                total -= file.bytes;
            }
        }
    }

    /// Bytes on disk.
    #[cfg(test)]
    fn size(&self) -> u64 {
        self.files().iter().map(|file| file.bytes).sum()
    }

    /// A file name that stays the same across builds; a collision only costs a miss, since the
    /// file holds its full key.
    fn path(&self, kind: &str, key: &str) -> PathBuf {
        self.root.join(kind).join(format!("{}.{FILE_EXTENSION}", stable_hash(key)))
    }

    /// The stored files; temporary files from a write that did not finish are deleted.
    fn files(&self) -> Vec<StoredFile> {
        let Ok(kinds) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut files = Vec::new();
        for kind in kinds.flatten() {
            let Ok(entries) = std::fs::read_dir(kind.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if path.extension().is_some_and(|extension| extension == TEMP_EXTENSION) {
                    let is_stale = metadata
                        .modified()
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > STALE_TEMP);
                    if is_stale {
                        let _gone_or_locked = std::fs::remove_file(&path);
                    }
                    continue;
                }
                let used = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                files.push(StoredFile { path, bytes: metadata.len(), used });
            }
        }
        files
    }
}

struct StoredFile {
    path: PathBuf,
    bytes: u64,
    used: SystemTime,
}

/// The key `bytes` hold and where their payload starts, when they are in this format at
/// `version`.
fn parse(bytes: &[u8], version: u32) -> Option<(&[u8], usize)> {
    let rest = bytes.strip_prefix(MAGIC.as_slice())?;
    let (stored_version, rest) = rest.split_first_chunk::<4>()?;
    let (key_length, rest) = rest.split_first_chunk::<4>()?;
    let key_length = usize::try_from(u32::from_le_bytes(*key_length)).ok()?;
    let key = rest.get(..key_length)?;
    (u32::from_le_bytes(*stored_version) == version).then_some((key, MAGIC.len() + 8 + key_length))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDir;

    fn cache(dir: &TestDir, budget: u64) -> ColdCache {
        ColdCache::new(&dir.paths(), budget)
    }

    #[test]
    fn reads_what_was_written_for_its_key_and_version() {
        let dir = TestDir::new("cold-roundtrip");
        let cold = cache(&dir, 1 << 20);
        cold.write("page", 3, "pr|a|detail", b"payload").unwrap();
        assert_eq!(cold.read("page", 3, "pr|a|detail").as_deref(), Some(&b"payload"[..]));
        assert_eq!(cold.read("page", 3, "pr|b|detail"), None, "another key");
        assert_eq!(cold.read("colors", 3, "pr|a|detail"), None, "another kind");
        cold.write("page", 3, "pr|b|threads", b"more").unwrap();
        let mut all = cold.read_all("page", 3, u64::MAX);
        all.sort();
        assert_eq!(
            all,
            [
                ("pr|a|detail".to_owned(), b"payload".to_vec()),
                ("pr|b|threads".to_owned(), b"more".to_vec())
            ]
        );
        assert_eq!(cold.read("page", 4, "pr|a|detail"), None, "another version is a miss");
        assert_eq!(cold.read("page", 3, "pr|a|detail"), None, "and was deleted");
    }

    #[test]
    fn a_damaged_file_is_a_miss_and_goes() {
        let dir = TestDir::new("cold-damaged");
        let cold = cache(&dir, 1 << 20);
        cold.write("page", 1, "key", b"payload").unwrap();
        let path = cold.path("page", "key");
        std::fs::write(&path, &MAGIC[..5]).unwrap();
        assert_eq!(cold.read("page", 1, "key"), None);
        assert!(!path.exists());
    }

    #[test]
    fn keeps_the_budget_by_dropping_the_least_recently_used() {
        let dir = TestDir::new("cold-budget");
        let cold = cache(&dir, 3_000);
        let block = vec![7_u8; 900];
        let age = |kind: &str, key: &str, seconds: u64| {
            let file = std::fs::File::options().write(true).open(cold.path(kind, key)).unwrap();
            file.set_modified(SystemTime::now() - Duration::from_secs(seconds)).unwrap();
        };
        cold.write("page", 1, "a", &block).unwrap();
        age("page", "a", 30);
        cold.write("page", 1, "b", &block).unwrap();
        age("page", "b", 20);
        cold.write("colors", 1, "c", &block).unwrap();
        age("colors", "c", 10);
        assert!(cold.read("page", 1, "a").is_some(), "reading a makes b the oldest");
        cold.write("page", 1, "d", &block).unwrap();
        assert_eq!(cold.read("page", 1, "b"), None, "the least recently used went");
        assert!(cold.read("page", 1, "a").is_some() && cold.read("colors", 1, "c").is_some());
        assert!(cold.size() <= 3_000, "{}", cold.size());
    }
}
