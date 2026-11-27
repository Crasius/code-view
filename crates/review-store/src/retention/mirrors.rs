//! Mirrors are deleted whole: `git gc` cannot reclaim the blobs a partial clone fetched on
//! demand (they sit in promisor packs), so trimming one in place frees little.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::mirror_stamp::{self, MirrorStamp};
use super::policy::Entry;
use super::trash::Trash;
use super::usage::tree_size;

/// Who may veto deleting a mirror: the app's mirror registry knows which mirrors an open pull
/// request or a running git command holds.
pub trait MirrorGuard: Send + Sync {
    /// Runs `retire` for the mirror at `git_dir` unless it is in use, with opening that mirror
    /// blocked meanwhile. Returns false when in use, otherwise what `retire` returned.
    fn retire_if_idle(&self, git_dir: &Path, retire: &mut dyn FnMut() -> bool) -> bool;
}

/// For tests: nothing is in use.
#[cfg(test)]
pub(crate) struct NothingInUse;

#[cfg(test)]
impl MirrorGuard for NothingInUse {
    fn retire_if_idle(&self, _git_dir: &Path, retire: &mut dyn FnMut() -> bool) -> bool {
        retire()
    }
}

pub struct ScannedMirror {
    pub entry: Entry,
    pub stamp: Option<MirrorStamp>,
}

/// Every mirror with its size and last use. Mirrors from before stamps existed use the time git
/// last changed their folder (each open rewrites the config there).
pub fn scan(mirrors_dir: &Path) -> Vec<ScannedMirror> {
    let Ok(children) = std::fs::read_dir(mirrors_dir) else {
        return Vec::new();
    };
    children
        .flatten()
        .filter(|child| child.file_type().is_ok_and(|file_type| file_type.is_dir()))
        .map(|child| {
            let path = child.path();
            let stamp = mirror_stamp::read(&path);
            let folder_changed = child
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let last_used = stamp.as_ref().map_or(folder_changed, MirrorStamp::last_used);
            ScannedMirror { entry: Entry { bytes: tree_size(&path), path, last_used }, stamp }
        })
        .collect()
}

/// Mirrors with their stamps, without measuring them: this runs for every finished pull request.
pub(super) fn stamped(mirrors_dir: &Path) -> Vec<(PathBuf, MirrorStamp)> {
    let Ok(children) = std::fs::read_dir(mirrors_dir) else {
        return Vec::new();
    };
    children
        .flatten()
        .filter_map(|child| {
            let path = child.path();
            mirror_stamp::read(&path).map(|stamp| (path, stamp))
        })
        .collect()
}

/// Moves the mirror into the trash unless the app or a git process uses it.
pub fn retire(guard: &dyn MirrorGuard, trash: &Trash, git_dir: &Path) -> bool {
    guard.retire_if_idle(git_dir, &mut || !has_git_locks(git_dir) && trash.move_in(git_dir).is_ok())
}

/// Lock and temporary files a running git command leaves while it writes, possibly from
/// another app instance. (A file git holds open also makes the rename fail on Windows.)
fn has_git_locks(git_dir: &Path) -> bool {
    let has_lock_in = |dir: &Path, is_lock: &dyn Fn(&str) -> bool| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|entry| is_lock(&entry.file_name().to_string_lossy()))
        })
    };
    has_lock_in(git_dir, &is_lock_file)
        || has_lock_in(&git_dir.join("objects").join("pack"), &|name| {
            name.starts_with("tmp_") || name.starts_with(".tmp-")
        })
        || has_ref_lock(&git_dir.join("refs"))
}

fn has_ref_lock(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let is_dir = entry.file_type().is_ok_and(|file_type| file_type.is_dir());
        if is_dir {
            has_ref_lock(&entry.path())
        } else {
            is_lock_file(&entry.file_name().to_string_lossy())
        }
    })
}

/// `index.lock`, `HEAD.lock`, `refs/…/name.lock`: git's lock files, in any case, since Windows
/// and macOS folders ignore it.
fn is_lock_file(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|extension| extension.eq_ignore_ascii_case("lock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDir;

    #[test]
    fn a_mirror_git_is_writing_stays() {
        let dir = TestDir::new("mirror-locks");
        let trash = Trash::new(dir.path().join("trash"));
        let git_dir = dir.path().join("mirrors").join("busy.git");
        std::fs::create_dir_all(git_dir.join("refs").join("review").join("pull")).unwrap();
        std::fs::write(git_dir.join("refs").join("review").join("pull").join("12.lock"), b"")
            .unwrap();

        assert!(!retire(&NothingInUse, &trash, &git_dir));
        assert!(git_dir.exists());
        std::fs::remove_file(git_dir.join("refs").join("review").join("pull").join("12.lock"))
            .unwrap();
        assert!(retire(&NothingInUse, &trash, &git_dir));
        assert!(!git_dir.exists());
    }
}
