//! Folders of single files (pipeline logs, viewed state): one folder per connection, plus files
//! from before that layout directly in the category folder.

use std::path::Path;
use std::time::SystemTime;

use super::policy::Entry;

/// Every file in `dir` and in its connection folders, with size and last change.
pub(crate) fn scan(dir: &Path) -> Vec<Entry> {
    let mut entries = Vec::new();
    let Ok(children) = std::fs::read_dir(dir) else {
        return entries;
    };
    for child in children.flatten() {
        let Ok(file_type) = child.file_type() else { continue };
        if file_type.is_dir() {
            if let Ok(files) = std::fs::read_dir(child.path()) {
                entries.extend(files.flatten().filter_map(|file| file_entry(&file.path())));
            }
        } else if file_type.is_file() {
            entries.extend(file_entry(&child.path()));
        }
    }
    entries
}

fn file_entry(path: &Path) -> Option<Entry> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let last_used = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    Some(Entry { path: path.to_owned(), bytes: metadata.len(), last_used })
}

/// Removes connection folders that became empty; folders with files stay.
pub(crate) fn remove_empty_folders(dir: &Path) {
    let Ok(children) = std::fs::read_dir(dir) else { return };
    for child in children.flatten() {
        if child.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            // Fails, harmlessly, when the folder still holds a file.
            let _non_empty_folder_stays = std::fs::remove_dir(child.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDir;

    #[test]
    fn finds_flat_and_per_connection_files() {
        let dir = TestDir::new("files-scan");
        std::fs::write(dir.path().join("legacy.log"), b"12").unwrap();
        std::fs::create_dir_all(dir.path().join("conn")).unwrap();
        std::fs::write(dir.path().join("conn").join("new.log"), b"1234").unwrap();
        std::fs::create_dir_all(dir.path().join("empty")).unwrap();

        let mut sizes: Vec<u64> = scan(dir.path()).iter().map(|entry| entry.bytes).collect();
        sizes.sort_unstable();
        assert_eq!(sizes, [2, 4]);
        remove_empty_folders(dir.path());
        assert!(!dir.path().join("empty").exists());
        assert!(dir.path().join("conn").exists());
    }
}
