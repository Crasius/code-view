//! Sweeps over real temp folders with a fake clock: file times and stamps are set relative to a
//! chosen `now`, and the sweep is told that `now`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use super::leases::Leases;
use super::mirrors::{MirrorGuard, NothingInUse};
use super::sweep::{CacheLimits, Sweeper};
use super::{DAY, Limits, MirrorUse, mirror_stamp};
use crate::flat_layout::{FlatLayout, pull_request_file};
use crate::test_support::{TestDir, wait_until, write_connections, write_file};
use crate::{AppPaths, Draft, ThreadStore, ViewedFiles, stable_hash};

pub(super) fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + DAY * 20_000
}

/// Writes `bytes` zeros to `path`, last changed `days_ago` before [`now`].
fn aged_file(path: PathBuf, bytes: usize, days_ago: u32) -> PathBuf {
    write_file(&path, &vec![0u8; bytes]);
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(now() - DAY * days_ago).unwrap();
    path
}

pub(super) struct Cache {
    _dir: TestDir,
    pub(super) paths: AppPaths,
}

impl Cache {
    pub(super) fn new(name: &str) -> Self {
        let dir = TestDir::new(name);
        let paths = dir.paths();
        Self { _dir: dir, paths }
    }

    pub(super) fn log(&self, connection: &str, name: &str, bytes: usize, days_ago: u32) -> PathBuf {
        aged_file(self.paths.logs_dir().join(stable_hash(connection)).join(name), bytes, days_ago)
    }

    pub(super) fn viewed(&self, connection: &str, name: &str, days_ago: u32) -> PathBuf {
        aged_file(self.paths.viewed_dir().join(stable_hash(connection)).join(name), 2, days_ago)
    }

    /// Saves a draft on `key` of `connection` through the store, as the pull request page does.
    pub(super) fn draft(&self, connection: &str, key: &str) -> PathBuf {
        let mut store = ThreadStore::load(&self.paths, connection, key).unwrap();
        store.set_draft("reply:t1", Draft { text: "Draft".into(), anchor: None });
        store.save().unwrap();
        pull_request_file(&self.paths.threads_dir(), connection, key)
    }

    pub(super) fn mirror(
        &self,
        name: &str,
        pack_bytes: usize,
        days_ago: u32,
        connections: &[&str],
    ) -> PathBuf {
        let git_dir = self.paths.mirrors_dir().join(name);
        aged_file(git_dir.join("objects").join("pack").join("pack-1.pack"), pack_bytes, 0);
        for connection in connections {
            let pull_request = format!("{connection}|repo|1");
            let usage = MirrorUse {
                connection_id: connection,
                pull_request: &pull_request,
                local_refs: &[],
            };
            mirror_stamp::record_use(&git_dir, &usage, now() - DAY * days_ago).unwrap();
        }
        git_dir
    }

    pub(super) fn connections(&self, ids: &[&str]) {
        write_connections(&self.paths, ids);
    }

    pub(super) fn sweeper(&self, guard: Arc<dyn MirrorGuard>, limits: CacheLimits) -> Sweeper {
        Sweeper::new(self.paths.clone(), Leases::default(), guard, limits)
    }
}

/// Refuses the mirrors whose folder name it holds, like the registry for an open pull request.
pub(super) struct InUse(pub(super) Vec<&'static str>);

impl MirrorGuard for InUse {
    fn retire_if_idle(&self, git_dir: &Path, retire: &mut dyn FnMut() -> bool) -> bool {
        let name = git_dir.file_name().unwrap().to_string_lossy();
        !self.0.iter().any(|held| *held == name) && retire()
    }
}

#[test]
fn logs_older_than_a_week_go_and_the_rest_stays() {
    let cache = Cache::new("sweep-logs-age");
    let old = cache.log("gh-1", "old.log", 10, 8);
    let recent = cache.log("gh-1", "recent.log", 10, 6);
    let legacy = aged_file(cache.paths.logs_dir().join("legacy.log"), 10, 9);

    let outcome = cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP).sweep(now());
    assert!(!old.exists());
    assert!(!legacy.exists());
    assert!(recent.exists());
    assert_eq!(outcome.removed, 2);
}

#[test]
fn logs_over_the_size_limit_go_least_recently_used_first() {
    let cache = Cache::new("sweep-logs-size");
    let oldest = cache.log("gh-1", "a.log", 400, 3);
    let middle = cache.log("gh-1", "b.log", 400, 2);
    let newest = cache.log("gh-1", "c.log", 400, 1);
    let limits = CacheLimits {
        logs: Limits { max_age: DAY * 7, max_total: Some(1000) },
        ..CacheLimits::APP
    };

    cache.sweeper(Arc::new(NothingInUse), limits).sweep(now());
    assert!(!oldest.exists());
    assert!(middle.exists());
    assert!(newest.exists());
}

#[test]
fn a_log_on_screen_is_kept() {
    let cache = Cache::new("sweep-logs-lease");
    let shown = cache.log("gh-1", "shown.log", 10, 30);
    let leases = Leases::default();
    let _lease = leases.lease(&shown);

    Sweeper::new(cache.paths, leases, Arc::new(NothingInUse), CacheLimits::APP).sweep(now());
    assert!(shown.exists());
}

#[test]
fn mirrors_unused_for_a_month_go_unless_in_use() {
    let cache = Cache::new("sweep-mirrors-age");
    cache.connections(&["gh-1"]);
    let stale = cache.mirror("stale.git", 10, 31, &["gh-1"]);
    let open = cache.mirror("open.git", 10, 45, &["gh-1"]);
    let recent = cache.mirror("recent.git", 10, 29, &["gh-1"]);

    let outcome = cache.sweeper(Arc::new(InUse(vec!["open.git"])), CacheLimits::APP).sweep(now());
    assert!(!stale.exists());
    assert!(open.exists());
    assert!(recent.exists());
    assert_eq!(outcome.kept_in_use, 1);
}

#[test]
fn mirrors_over_the_size_limit_go_whole_least_recently_used_first() {
    let cache = Cache::new("sweep-mirrors-size");
    cache.connections(&["gh-1"]);
    let oldest = cache.mirror("a.git", 600, 5, &["gh-1"]);
    let held = cache.mirror("b.git", 600, 4, &["gh-1"]);
    let next = cache.mirror("c.git", 600, 3, &["gh-1"]);
    let newest = cache.mirror("d.git", 600, 1, &["gh-1"]);
    let limits = CacheLimits {
        mirrors: Limits { max_age: DAY * 30, max_total: Some(1500) },
        ..CacheLimits::APP
    };

    cache.sweeper(Arc::new(InUse(vec!["b.git"])), limits).sweep(now());
    assert!(!oldest.exists());
    assert!(held.exists(), "the mirror in use stays even though it is older");
    assert!(!next.exists());
    assert!(newest.exists());
}

#[test]
fn a_mirror_with_read_only_packs_is_deleted_completely() {
    let cache = Cache::new("sweep-mirrors-readonly");
    cache.connections(&["gh-1"]);
    let git_dir = cache.mirror("old.git", 10, 40, &["gh-1"]);
    let pack = git_dir.join("objects").join("pack").join("pack-1.pack");
    let mut permissions = std::fs::metadata(&pack).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&pack, permissions).unwrap();

    let sweeper = cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP);
    sweeper.sweep(now());
    assert!(!git_dir.exists());
    // The sweep started the purge thread; a second purge here would race it for the same files
    // (Windows then refuses one of the deletes).
    wait_until("the purge thread deletes the read-only pack", || sweeper.trash().is_empty());
}

#[test]
fn viewed_state_unused_for_ninety_days_goes() {
    let cache = Cache::new("sweep-viewed");
    cache.connections(&["gh-1"]);
    let old = cache.viewed("gh-1", "old.json", 91);
    let kept = cache.viewed("gh-1", "kept.json", 89);

    cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP).sweep(now());
    assert!(!old.exists());
    assert!(kept.exists());
}

#[test]
fn thread_marks_and_drafts_unused_for_ninety_days_go() {
    let cache = Cache::new("sweep-threads");
    cache.connections(&["gh-1"]);
    let threads = cache.paths.threads_dir();
    let old = aged_file(threads.join(stable_hash("gh-1")).join("old.json"), 2, 91);
    let kept = aged_file(threads.join(stable_hash("gh-1")).join("kept.json"), 2, 89);
    let flat = aged_file(threads.join("flat.json"), 2, 91);
    let owners = aged_file(threads.join("owners.json"), 2, 91);

    cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP).sweep(now());
    assert!(!old.exists());
    assert!(!flat.exists(), "files from before the per-connection layout age out too");
    assert!(kept.exists());
    assert!(owners.exists(), "the owners of flat files never age out");
}

/// Regression: drafts and read marks were kept by pull request alone, so a new connection that
/// reused a removed connection's id found the old drafts.
#[test]
fn removing_a_connection_deletes_its_drafts_and_a_new_one_with_its_id_starts_empty() {
    let cache = Cache::new("sweep-connection-drafts");
    cache.connections(&["gh-1", "gh-2"]);
    let removed = cache.draft("gh-1", "gh-1|cli/cli|1");
    let other = cache.draft("gh-2", "gh-2|cli/cli|1");
    cache.connections(&["gh-2"]);

    cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP).forget_removed_connections();
    assert!(!removed.exists());
    assert!(!removed.parent().unwrap().exists(), "the connection folder goes too");
    assert!(other.exists(), "another connection's drafts stay");
    let store = ThreadStore::load(&cache.paths, "gh-2", "gh-2|cli/cli|1").unwrap();
    assert!(store.draft("reply:t1").is_some());

    cache.connections(&["gh-2", "gh-1"]);
    let reused = ThreadStore::load(&cache.paths, "gh-1", "gh-1|cli/cli|1").unwrap();
    assert!(reused.draft("reply:t1").is_none());
    assert!(!reused.has_visited(), "no read marks either");
}

/// Files from before the per-connection layout name no connection: a removed connection leaves
/// the owners, so its unopened flat files never go to a new connection with its id, and they all
/// go once no owner is left.
#[test]
fn flat_marks_and_drafts_of_a_removed_connection_go_to_nobody() {
    let cache = Cache::new("sweep-connection-flat-drafts");
    let flat = |dir: PathBuf, key: &str, body: &str| {
        let path = FlatLayout::new(&cache.paths, dir.clone()).flat_file(key);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    };
    let draft = r#"{"drafts":{"reply:t1":{"text":"Old"}}}"#;
    let marks = r#"{"files":{"src/a.rs":"blob1"}}"#;
    let removed = [
        flat(cache.paths.threads_dir(), "gh-1|cli/cli|1", draft),
        flat(cache.paths.viewed_dir(), "gh-1|cli/cli|1", marks),
    ];
    let kept = [
        flat(cache.paths.threads_dir(), "gh-2|cli/cli|1", draft),
        flat(cache.paths.viewed_dir(), "gh-2|cli/cli|1", marks),
    ];
    let all = || removed.iter().chain(&kept);
    cache.connections(&["gh-2"]);
    let sweeper = cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP);

    sweeper.forget_removed_connections();
    assert!(all().all(|file| file.exists()), "unsure which is whose: all stay while gh-2 owns");
    cache.connections(&["gh-2", "gh-1"]);
    let reused = ThreadStore::load(&cache.paths, "gh-1", "gh-1|cli/cli|1").unwrap();
    assert!(reused.draft("reply:t1").is_none(), "a new gh-1 adopts no draft");
    let reused = ViewedFiles::load(&cache.paths, "gh-1", "gh-1|cli/cli|1").unwrap();
    assert!(!reused.is_viewed("src/a.rs", "blob1"), "nor a viewed mark");
    assert!(removed.iter().all(|file| file.exists()));

    cache.connections(&["gh-1"]);
    sweeper.forget_removed_connections();
    assert!(all().all(|file| !file.exists()), "no owner is left");
    assert!(cache.paths.threads_dir().join("owners.json").exists());
    assert!(cache.paths.viewed_dir().join("owners.json").exists());
}

#[test]
fn removing_a_connection_deletes_its_data_and_keeps_shared_mirrors() {
    let cache = Cache::new("sweep-connection-removed");
    let removed_log = cache.log("gh-1", "a.log", 10, 0);
    let removed_viewed = cache.viewed("gh-1", "a.json", 0);
    let own_mirror = cache.mirror("own.git", 10, 0, &["gh-1"]);
    let shared_mirror = cache.mirror("shared.git", 10, 0, &["gh-1", "gh-2"]);
    let other_log = cache.log("gh-2", "b.log", 10, 0);
    let other_viewed = cache.viewed("gh-2", "b.json", 0);
    cache.connections(&["gh-2"]);

    let outcome =
        cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP).forget_removed_connections();
    assert!(!removed_log.exists());
    assert!(!removed_viewed.exists());
    assert!(!removed_log.parent().unwrap().exists(), "the connection folder goes too");
    assert!(!own_mirror.exists());
    assert!(shared_mirror.exists());
    let stamp = mirror_stamp::read(&shared_mirror).unwrap();
    assert_eq!(stamp.connections().iter().map(String::as_str).collect::<Vec<_>>(), ["gh-2"]);
    assert!(other_log.exists());
    assert!(other_viewed.exists());
    assert_eq!(outcome.removed, 3);
}

#[test]
fn an_unreadable_connection_list_deletes_nothing() {
    let cache = Cache::new("sweep-connections-missing");
    let log = cache.log("gh-1", "a.log", 10, 0);
    let mirror = cache.mirror("own.git", 10, 0, &["gh-1"]);

    cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP).forget_removed_connections();
    assert!(log.exists());
    assert!(mirror.exists());
}

#[test]
fn clear_cache_keeps_viewed_marks_and_mirrors_in_use() {
    let cache = Cache::new("sweep-clear");
    cache.connections(&["gh-1"]);
    let log = cache.log("gh-1", "a.log", 10, 0);
    let viewed = cache.viewed("gh-1", "a.json", 0);
    let idle = cache.mirror("idle.git", 10, 0, &["gh-1"]);
    let open = cache.mirror("open.git", 10, 0, &["gh-1"]);
    let cold = aged_file(cache.paths.cold_cache_dir().join("page").join("p.bin"), 10, 0);

    let outcome = cache.sweeper(Arc::new(InUse(vec!["open.git"])), CacheLimits::APP).clear();
    assert!(!log.exists());
    assert!(!cold.exists(), "the cold tier goes with the cache");
    assert!(!idle.exists());
    assert!(open.exists());
    assert!(viewed.exists());
    assert_eq!(outcome.kept_in_use, 1);
}
