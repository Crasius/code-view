//! Deleting without waiting: a target is first renamed into `cache\trash` (one atomic rename on
//! the same volume, so it is gone from its place at once), then a low-priority thread deletes the
//! trash. Nothing waits for that thread; whatever an exit interrupts is deleted at the next start.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::platform;

#[derive(Clone)]
pub(crate) struct Trash {
    dir: PathBuf,
    is_purging: Arc<AtomicBool>,
}

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl Trash {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir, is_purging: Arc::new(AtomicBool::new(false)) }
    }

    /// Renames `target` into the trash under a unique name. Fails, leaving `target` in place,
    /// when something holds it open (Windows refuses the rename then).
    pub(crate) fn move_in(&self, target: &Path) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let name = target
            .file_name()
            .map_or_else(|| "item".into(), |name| name.to_string_lossy().into_owned());
        let stamp =
            SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::fs::rename(target, self.dir.join(format!("{stamp}-{sequence}-{name}")))
    }

    pub(crate) fn is_empty(&self) -> bool {
        std::fs::read_dir(&self.dir).map_or(true, |mut entries| entries.next().is_none())
    }

    /// Starts the deleting thread unless it runs already. It ends once the trash is empty.
    pub(crate) fn purge_in_background(&self) {
        if self.is_empty() || self.is_purging.swap(true, Ordering::AcqRel) {
            return;
        }
        let trash = self.clone();
        let started = std::thread::Builder::new().name("trash-purge".into()).spawn(move || {
            platform::enter_background_priority();
            // Items moved in while a pass ran are picked up by the next one. An item that cannot
            // be deleted now (held open) waits for the next sweep or start.
            let mut is_clean = true;
            while is_clean && !trash.is_empty() {
                is_clean = trash.purge().is_ok();
            }
            trash.is_purging.store(false, Ordering::Release);
            // Something moved in between the last check and the flag going down.
            if is_clean {
                trash.purge_in_background();
            }
        });
        if started.is_err() {
            self.is_purging.store(false, Ordering::Release);
        }
    }

    /// Deletes everything in the trash now. Fails on the first item it cannot delete.
    pub(crate) fn purge(&self) -> io::Result<()> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            remove_tree(&entry?.path())?;
        }
        Ok(())
    }
}

/// Removes a file or a directory tree without following links. Git marks pack files read-only
/// (Windows refuses to delete those), so a refused delete clears the attribute and tries again.
/// Whatever is already gone counts as removed.
pub(crate) fn remove_tree(path: &Path) -> io::Result<()> {
    ignore_missing(remove_existing(path))
}

fn remove_existing(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        // A directory link (or junction) is removed as a directory; its target stays.
        return std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path));
    }
    if file_type.is_dir() {
        for entry in std::fs::read_dir(path)? {
            remove_tree(&entry?.path())?;
        }
        return with_writable(path, metadata.permissions(), || std::fs::remove_dir(path));
    }
    with_writable(path, metadata.permissions(), || std::fs::remove_file(path))
}

fn ignore_missing(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn with_writable(
    path: &Path,
    permissions: std::fs::Permissions,
    remove: impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    match remove() {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied && permissions.readonly() => {
            let mut writable = permissions;
            #[expect(
                clippy::permissions_set_readonly_false,
                reason = "clears the Windows read-only attribute git sets on packs"
            )]
            writable.set_readonly(false);
            std::fs::set_permissions(path, writable)?;
            remove()
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TestDir, wait_until};

    #[test]
    fn moved_items_leave_their_place_at_once_and_purge_later() {
        let root = TestDir::new("trash-move");
        let target = root.path().join("mirror.git");
        std::fs::create_dir_all(target.join("objects")).unwrap();
        std::fs::write(target.join("objects").join("pack"), b"data").unwrap();
        let trash = Trash::new(root.path().join("trash"));

        trash.move_in(&target).unwrap();
        assert!(!target.exists());
        assert!(!trash.is_empty());
        trash.purge().unwrap();
        assert!(trash.is_empty());
    }

    #[test]
    fn read_only_pack_files_are_deleted() {
        let root = TestDir::new("trash-readonly");
        let pack_dir = root.path().join("mirror.git").join("objects").join("pack");
        std::fs::create_dir_all(&pack_dir).unwrap();
        let pack = pack_dir.join("pack-1.pack");
        std::fs::write(&pack, b"pack").unwrap();
        let mut permissions = std::fs::metadata(&pack).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&pack, permissions).unwrap();

        remove_tree(&root.path().join("mirror.git")).unwrap();
        assert!(!root.path().join("mirror.git").exists());
    }

    #[test]
    fn leftovers_from_an_interrupted_run_are_resumed() {
        let root = TestDir::new("trash-resume");
        let trash_dir = root.path().join("trash");
        std::fs::create_dir_all(trash_dir.join("1-0-old.git").join("objects")).unwrap();
        std::fs::write(trash_dir.join("1-0-old.git").join("objects").join("left"), b"x").unwrap();
        std::fs::write(trash_dir.join("2-1-old.log"), b"log").unwrap();

        // A new app run: a fresh `Trash` on the same folder.
        let trash = Trash::new(trash_dir);
        trash.purge_in_background();
        wait_until("the leftovers are deleted", || trash.is_empty());
    }
}
