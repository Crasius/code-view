//! How much the cache folder holds, per category, for Settings → Storage.

use std::path::Path;

use crate::AppPaths;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StorageUsage {
    pub mirrors: u64,
    pub logs: u64,
    /// The disk tier of the memory caches ([`crate::ColdCache`]).
    pub cold: u64,
    /// Viewed state and anything else in the cache folder. The trash is not counted: it is
    /// already being deleted.
    pub other: u64,
}

impl StorageUsage {
    pub fn total(&self) -> u64 {
        self.mirrors + self.logs + self.cold + self.other
    }
}

pub fn measure(paths: &AppPaths) -> StorageUsage {
    let mut usage = StorageUsage::default();
    let Ok(children) = std::fs::read_dir(paths.cache_dir()) else {
        return usage;
    };
    let (mirrors, logs, trash) = (paths.mirrors_dir(), paths.logs_dir(), paths.trash_dir());
    let cold = paths.cold_cache_dir();
    for child in children.flatten() {
        let path = child.path();
        if path == trash {
            continue;
        }
        let bytes = tree_size(&path);
        if path == mirrors {
            usage.mirrors += bytes;
        } else if path == logs {
            usage.logs += bytes;
        } else if path == cold {
            usage.cold += bytes;
        } else {
            usage.other += bytes;
        }
    }
    usage
}

/// Bytes of all files under `path`, not following links.
pub fn tree_size(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_file() {
        return metadata.len();
    }
    if !metadata.is_dir() {
        return 0;
    }
    let Ok(children) = std::fs::read_dir(path) else {
        return 0;
    };
    children.flatten().map(|child| tree_size(&child.path())).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TestDir, write_file};

    #[test]
    fn sizes_per_category_without_the_trash() {
        let dir = TestDir::new("usage");
        let paths = dir.paths();
        let write = |path: std::path::PathBuf, bytes: usize| write_file(&path, &vec![0u8; bytes]);
        write(paths.mirrors_dir().join("a.git").join("objects").join("pack").join("p.pack"), 300);
        write(paths.logs_dir().join("c").join("l.log"), 20);
        write(paths.viewed_dir().join("c").join("v.json"), 4);
        write(paths.trash_dir().join("1-0-old.git"), 1000);
        write(paths.cold_cache_dir().join("page").join("p.bin"), 7);

        assert_eq!(measure(&paths), StorageUsage { mirrors: 300, logs: 20, cold: 7, other: 4 });
    }
}
