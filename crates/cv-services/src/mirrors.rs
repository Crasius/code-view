use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use review_core::{GitRemote, Result};
use review_git::GitMirror;
use review_store::retention::{self, MirrorGuard, MirrorUse};
use tokio::sync::Mutex;

/// One [`GitMirror`] per clone URL for the whole app, so concurrent pages never race on the same repository.
pub struct MirrorRegistry {
    root: PathBuf,
    mirrors: Mutex<HashMap<String, Arc<GitMirror>>>,
}

impl MirrorRegistry {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root, mirrors: Mutex::new(HashMap::new()) }
    }

    /// The mirror of `remote`, stamped as used now by `usage` (retention keeps it a month from
    /// here, and drops the pull request's refs once it finished).
    pub(crate) async fn open(
        &self,
        remote: &GitRemote,
        usage: &MirrorUse<'_>,
    ) -> Result<Arc<GitMirror>> {
        let mirror = self.open_unstamped(remote).await?;
        Self::record_use(&mirror, usage);
        Ok(mirror)
    }

    /// The mirror of `remote`, created on first use, before any pull request's commits are known.
    /// Whoever holds it keeps retention off it; [`Self::record_use`] stamps it once they are.
    pub async fn open_unstamped(&self, remote: &GitRemote) -> Result<Arc<GitMirror>> {
        let mut mirrors = self.mirrors.lock().await;
        if let Some(mirror) = mirrors.get(&remote.clone_url) {
            return Ok(mirror.clone());
        }
        let mirror = Arc::new(GitMirror::open(&self.root, remote).await?);
        mirrors.insert(remote.clone_url.clone(), mirror.clone());
        Ok(mirror)
    }

    /// Stamps `mirror` as used now by `usage`, as [`Self::open`] does.
    pub fn record_use(mirror: &GitMirror, usage: &MirrorUse<'_>) {
        // A missing stamp only lets the mirror age out by its folder time; never worth failing the page for.
        let _unstamped = retention::record_mirror_use(mirror.git_dir(), usage, SystemTime::now());
    }

    /// The mirror of `remote` when a pull request page made one before; never creates one, and
    /// reading from it keeps no pull request's data alive.
    pub(crate) async fn existing(&self, remote: &GitRemote) -> Result<Option<Arc<GitMirror>>> {
        let mut mirrors = self.mirrors.lock().await;
        if let Some(mirror) = mirrors.get(&remote.clone_url) {
            return Ok(Some(mirror.clone()));
        }
        if !GitMirror::exists(&self.root, remote) {
            return Ok(None);
        }
        let mirror = Arc::new(GitMirror::open(&self.root, remote).await?);
        mirrors.insert(remote.clone_url.clone(), mirror.clone());
        Ok(Some(mirror))
    }
}

impl MirrorGuard for MirrorRegistry {
    /// In use means held outside the registry: by an open pull request's git data or by a task
    /// running git. Holding the lock meanwhile keeps [`Self::open`] from handing the mirror out.
    fn retire_if_idle(&self, git_dir: &Path, retire: &mut dyn FnMut() -> bool) -> bool {
        let mut mirrors = self.mirrors.blocking_lock();
        let held =
            mirrors.iter().find(|(_, mirror)| mirror.git_dir().file_name() == git_dir.file_name());
        let Some((clone_url, mirror)) = held else {
            return retire();
        };
        if Arc::strong_count(mirror) > 1 {
            return false;
        }
        let clone_url = clone_url.clone();
        let is_retired = retire();
        if is_retired {
            // The next open creates the mirror afresh.
            mirrors.remove(&clone_url);
        }
        is_retired
    }
}
