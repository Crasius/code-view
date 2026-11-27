//! One pass over the cache folder: file system work only, never the network. Every deletion is a
//! rename into the trash; the trash thread does the slow part.
//!
//! A pull request that was merged or closed, or whose source branch is gone, takes its local data
//! with it right away ([`Sweeper::forget_pull_request`]): its pipeline logs, its viewed marks, its
//! thread read marks and drafts (nothing can be posted to it any more) and its refs in the mirror.
//! A mirror left without any live pull request goes whole (dropping refs alone frees nothing: the
//! objects stay in the packs); otherwise only the refs go and the size and age limits reclaim the
//! rest. The caller keeps the open pull request out (its page holds its drafts); see `worker`.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use serde::Deserialize;

use super::leases::Leases;
use super::mirror_stamp::{self, MirrorStamp};
use super::mirrors::{self, MirrorGuard};
use super::policy::{self, Entry, Outcome};
use super::trash::Trash;
use super::{
    IMAGE_LIMITS, LOG_LIMITS, Limits, MIRROR_LIMITS, THREAD_LIMITS, VIEWED_LIMITS, files, refs,
};
use crate::flat_layout::{FlatLayout, pull_request_file};
use crate::{AppPaths, COLD_CACHE_BUDGET_BYTES, ColdCache, atomic_file, stable_hash};

pub struct Sweeper {
    paths: AppPaths,
    trash: Trash,
    leases: Leases,
    guard: Arc<dyn MirrorGuard>,
    limits: CacheLimits,
}

/// The limits per category; the app uses [`CacheLimits::APP`].
#[derive(Debug, Clone, Copy)]
pub struct CacheLimits {
    pub logs: Limits,
    pub mirrors: Limits,
    pub viewed: Limits,
    pub threads: Limits,
    pub images: Limits,
}

impl CacheLimits {
    pub const APP: Self = Self {
        logs: LOG_LIMITS,
        mirrors: MIRROR_LIMITS,
        viewed: VIEWED_LIMITS,
        threads: THREAD_LIMITS,
        images: IMAGE_LIMITS,
    };
}

/// A pull request by connection and key (`connection|repository|number`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FinishedPullRequest {
    pub connection_id: String,
    pub key: String,
}

impl Sweeper {
    pub fn new(
        paths: AppPaths,
        leases: Leases,
        guard: Arc<dyn MirrorGuard>,
        limits: CacheLimits,
    ) -> Self {
        let trash = Trash::new(paths.trash_dir());
        Self { paths, trash, leases, guard, limits }
    }

    pub fn trash(&self) -> &Trash {
        &self.trash
    }

    /// The scheduled pass: data of removed connections, then the age and size limits.
    pub fn sweep(&self, now: SystemTime) -> Outcome {
        let mut outcome = self.forget_removed();
        outcome.add(policy::apply(
            files::scan(&self.paths.logs_dir()),
            now,
            self.limits.logs,
            &mut |entry| self.retire_file(entry),
        ));
        let mirrors = mirrors::scan(&self.paths.mirrors_dir())
            .into_iter()
            .map(|mirror| mirror.entry)
            .collect();
        outcome.add(policy::apply(mirrors, now, self.limits.mirrors, &mut |entry| {
            self.retire_mirror(entry)
        }));
        outcome.add(policy::apply(
            self.marks(&self.paths.viewed_dir()),
            now,
            self.limits.viewed,
            &mut |entry| self.retire_file(entry),
        ));
        outcome.add(policy::apply(
            self.marks(&self.paths.threads_dir()),
            now,
            self.limits.threads,
            &mut |entry| self.retire_file(entry),
        ));
        outcome.add(policy::apply(
            files::scan(&self.paths.images_dir()),
            now,
            self.limits.images,
            &mut |entry| self.retire_file(entry),
        ));
        ColdCache::new(&self.paths, COLD_CACHE_BUDGET_BYTES).enforce_budget();
        self.finish();
        outcome
    }

    /// Deletes logs, viewed state, thread read marks and drafts, images and mirrors of connections
    /// that are no longer configured.
    /// Does nothing while the connection list cannot be read: unsure means keep.
    pub fn forget_removed_connections(&self) -> Outcome {
        let outcome = self.forget_removed();
        self.finish();
        outcome
    }

    fn forget_removed(&self) -> Outcome {
        let mut outcome = Outcome::default();
        let Some(known) = connection_ids(&self.paths.connections_file()) else {
            return outcome;
        };
        let known_folders: HashSet<String> = known.iter().map(|id| stable_hash(id)).collect();
        let dirs = [
            self.paths.logs_dir(),
            self.paths.viewed_dir(),
            self.paths.threads_dir(),
            self.paths.images_dir(),
        ];
        for dir in dirs {
            let orphans = files::scan(&dir).into_iter().filter(|entry| {
                // Files from before the per-connection layout belong to no known folder; they age out.
                let folder = entry.path.parent().filter(|parent| *parent != dir.as_path());
                folder
                    .and_then(Path::file_name)
                    .is_some_and(|name| !known_folders.contains(&*name.to_string_lossy()))
            });
            outcome.add(policy::retire_all(orphans, &mut |entry| self.retire_file(entry)));
        }
        for dir in [self.paths.viewed_dir(), self.paths.threads_dir()] {
            if FlatLayout::new(&self.paths, dir.clone()).release_removed(&known) {
                let flat = self
                    .marks(&dir)
                    .into_iter()
                    .filter(|entry| entry.path.parent() == Some(dir.as_path()));
                outcome.add(policy::retire_all(flat, &mut |entry| self.retire_file(entry)));
            }
        }
        for mirror in mirrors::scan(&self.paths.mirrors_dir()) {
            let Some(mut stamp) = mirror.stamp else { continue };
            let was_dropped = stamp.retain_connections(|id| known.contains(id));
            // No connection, or no live pull request left (all finished or of removed connections).
            if stamp.connections().is_empty() || !stamp.has_pull_requests() {
                outcome.add(policy::retire_all([mirror.entry], &mut |entry| {
                    self.retire_mirror(entry)
                }));
            } else if was_dropped {
                // A stamp that could not be written keeps the dropped ids; the next sweep retries.
                let _retried_next_sweep = mirror_stamp::write(&mirror.entry.path, &stamp);
            }
        }
        outcome
    }

    /// Settings → Storage → Clear cache: every mirror, log and image not in use. Viewed marks,
    /// thread read marks and drafts stay; they are the reviewer's work, not a download.
    pub fn clear(&self) -> Outcome {
        let mut retire_file = |entry: &Entry| self.retire_file(entry);
        let mut outcome = policy::retire_all(files::scan(&self.paths.logs_dir()), &mut retire_file);
        outcome.add(policy::retire_all(files::scan(&self.paths.images_dir()), &mut retire_file));
        outcome
            .add(policy::retire_all(files::scan(&self.paths.cold_cache_dir()), &mut retire_file));
        let mirrors =
            mirrors::scan(&self.paths.mirrors_dir()).into_iter().map(|mirror| mirror.entry);
        outcome.add(policy::retire_all(mirrors, &mut |entry| self.retire_mirror(entry)));
        self.finish();
        outcome
    }

    /// Deletes what `pull_request` left (see the module doc). False when something is still in use
    /// and needs another try.
    pub fn forget_pull_request(&self, pull_request: &FinishedPullRequest) -> bool {
        let mut is_done = self.forget_logs(pull_request);
        let (connection_id, key) = (&pull_request.connection_id, &pull_request.key);
        let viewed_dir = self.paths.viewed_dir();
        is_done &= self.forget_file(&pull_request_file(&viewed_dir, connection_id, key));
        let threads_dir = self.paths.threads_dir();
        is_done &= self.forget_file(&pull_request_file(&threads_dir, connection_id, key));
        // A file from before the per-connection layout that its pull request never adopted.
        for dir in [viewed_dir, threads_dir] {
            is_done &= self.forget_file(&FlatLayout::new(&self.paths, dir).flat_file(key));
        }
        for (git_dir, stamp) in mirrors::stamped(&self.paths.mirrors_dir()) {
            if stamp.has_pull_request(&pull_request.key) {
                is_done &= self.forget_in_mirror(&git_dir, stamp, &pull_request.key);
            }
        }
        self.finish();
        is_done
    }

    /// The viewed or thread marks in `dir`, without the owners of its flat files: those never age.
    fn marks(&self, dir: &Path) -> Vec<Entry> {
        let layout = FlatLayout::new(&self.paths, dir.to_owned());
        files::scan(dir).into_iter().filter(|entry| !layout.is_owners_file(&entry.path)).collect()
    }

    fn forget_logs(&self, pull_request: &FinishedPullRequest) -> bool {
        let prefix = format!("{}-", stable_hash(&pull_request.key));
        let folder = self.paths.connection_logs_dir(&pull_request.connection_id);
        let entries = files::scan(&folder);
        let own = entries.iter().filter(|entry| {
            entry.path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
        });
        // Every log is tried, even after one is on screen.
        let mut is_done = true;
        for entry in own {
            is_done &= self.retire_file(entry);
        }
        is_done
    }

    fn forget_file(&self, path: &Path) -> bool {
        if !path.exists() {
            return true;
        }
        self.retire_file(&Entry {
            path: path.to_owned(),
            bytes: 0,
            last_used: SystemTime::UNIX_EPOCH,
        })
    }

    fn forget_in_mirror(&self, git_dir: &Path, mut stamp: MirrorStamp, key: &str) -> bool {
        let own = stamp.take_pull_request(key);
        let Ok(existing) = refs::list(git_dir) else {
            return false;
        };
        let to_drop: Vec<String> = own.into_iter().filter(|name| existing.contains(name)).collect();
        let is_last =
            !stamp.has_pull_requests() && existing.iter().all(|name| to_drop.contains(name));
        if is_last && mirrors::retire(self.guard.as_ref(), &self.trash, git_dir) {
            return true;
        }
        // Other pull requests still use the mirror (or something holds it right now: then the
        // next sweep takes the mirror, which no pull request uses any more).
        refs::delete(git_dir, &to_drop).is_ok() && mirror_stamp::write(git_dir, &stamp).is_ok()
    }

    fn retire_file(&self, entry: &Entry) -> bool {
        !self.leases.is_leased(&entry.path) && self.trash.move_in(&entry.path).is_ok()
    }

    fn retire_mirror(&self, entry: &Entry) -> bool {
        mirrors::retire(self.guard.as_ref(), &self.trash, &entry.path)
    }

    fn finish(&self) {
        files::remove_empty_folders(&self.paths.logs_dir());
        files::remove_empty_folders(&self.paths.viewed_dir());
        files::remove_empty_folders(&self.paths.threads_dir());
        files::remove_empty_folders(&self.paths.images_dir());
        self.trash.purge_in_background();
    }
}

#[derive(Deserialize)]
struct StoredIds {
    connections: Vec<StoredId>,
}

#[derive(Deserialize)]
struct StoredId {
    id: String,
}

/// Ids from the connections file without decrypting any token; `None` when it is absent or unreadable.
pub(crate) fn connection_ids(file: &Path) -> Option<HashSet<String>> {
    let stored: StoredIds = atomic_file::read_json(file).ok().flatten()?;
    Some(stored.connections.into_iter().map(|connection| connection.id).collect())
}
