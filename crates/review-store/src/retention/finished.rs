//! A pull request that was merged or closed, or whose source branch is gone, takes its local data
//! with it right away: its pipeline logs, its viewed marks, its thread read marks and drafts
//! (nothing can be posted to it any more) and its refs in the mirror. A mirror
//! left without any live pull request goes whole (dropping refs alone frees nothing: the objects
//! stay in the packs); otherwise only the refs go and the size and age limits reclaim the rest.
//!
//! The deletion itself is `Sweeper::forget_pull_request` in `sweep`, next to the other passes that
//! retire files and mirrors. The caller keeps the open pull request out (its page holds its
//! drafts); see `worker`.

use std::path::{Path, PathBuf};

use super::mirror_stamp::{self, MirrorStamp};

/// A pull request by connection and key (`connection|repository|number`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FinishedPullRequest {
    pub connection_id: String,
    pub key: String,
}

/// Mirrors with their stamps, without measuring them: this runs for every finished pull request.
pub(super) fn stamped_mirrors(mirrors_dir: &Path) -> Vec<(PathBuf, MirrorStamp)> {
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
