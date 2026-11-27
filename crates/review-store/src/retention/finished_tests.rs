//! A finished pull request (merged, closed, source branch gone) takes its local data with it:
//! logs, viewed marks and mirror refs, the whole mirror when no live pull request is left. Its
//! open page (with its drafts) keeps everything until it closes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::leases::Leases;
use super::mirrors::NothingInUse;
use super::refs::{self, test_repo};
use super::sweep::{CacheLimits, FinishedPullRequest};
use super::sweep_tests::{Cache, InUse};
use super::{MirrorUse, Retention, Schedule, mirror_stamp};
use crate::flat_layout::pull_request_file;
use crate::test_support::{wait_until, write_file};
use crate::{AppPaths, stable_hash};

const MERGED: &str = "gh-1|cli/cli|1";
const OPEN: &str = "gh-1|cli/cli|2";

fn finished(key: &str) -> FinishedPullRequest {
    FinishedPullRequest { connection_id: "gh-1".into(), key: key.into() }
}

fn log(paths: &AppPaths, key: &str, log_id: &str) -> PathBuf {
    write_file(&paths.pull_request_log_file("gh-1", key, log_id), b"log")
}

fn viewed(paths: &AppPaths, key: &str) -> PathBuf {
    write_file(&pull_request_file(&paths.viewed_dir(), "gh-1", key), b"{}")
}

fn thread_marks(paths: &AppPaths, key: &str) -> PathBuf {
    write_file(&pull_request_file(&paths.threads_dir(), "gh-1", key), b"{}")
}

/// A mirror with real refs, stamped with the pull requests that fetched them.
fn mirror(paths: &AppPaths, name: &str, pull_requests: &[(&str, &[&str])]) -> PathBuf {
    let git_dir = paths.mirrors_dir().join(name);
    let all: Vec<&str> =
        pull_requests.iter().flat_map(|(_, names)| names.iter().copied()).collect();
    test_repo::bare_with_refs(&git_dir, &all);
    for (key, names) in pull_requests {
        let local_refs: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
        let usage = MirrorUse { connection_id: "gh-1", pull_request: key, local_refs: &local_refs };
        mirror_stamp::record_use(&git_dir, &usage, SystemTime::now()).unwrap();
    }
    git_dir
}

fn review_refs(git_dir: &Path) -> Vec<String> {
    refs::list(git_dir).unwrap()
}

#[test]
fn a_merged_pull_request_loses_its_logs_marks_and_own_refs() {
    let cache = Cache::new("finished-refs");
    let merged_log = log(&cache.paths, MERGED, "job-1");
    let open_log = log(&cache.paths, OPEN, "job-1");
    let merged_viewed = viewed(&cache.paths, MERGED);
    let open_viewed = viewed(&cache.paths, OPEN);
    let merged_threads = thread_marks(&cache.paths, MERGED);
    let open_threads = thread_marks(&cache.paths, OPEN);
    let git_dir = mirror(
        &cache.paths,
        "github.com_cli_cli.git",
        &[
            (MERGED, &["refs/review/pull/1/head", "refs/review/heads/main"]),
            (OPEN, &["refs/review/pull/2/head", "refs/review/heads/main"]),
        ],
    );

    let sweeper = cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP);
    assert!(sweeper.forget_pull_request(&finished(MERGED)));
    assert!(!merged_log.exists());
    assert!(!merged_viewed.exists());
    assert!(!merged_threads.exists());
    assert!(
        open_log.exists() && open_viewed.exists() && open_threads.exists(),
        "the other pull request keeps its data"
    );
    assert!(git_dir.exists(), "another pull request still uses the mirror");
    assert_eq!(
        review_refs(&git_dir),
        ["refs/review/heads/main", "refs/review/pull/2/head"],
        "the shared target ref stays"
    );
    assert!(!mirror_stamp::read(&git_dir).unwrap().has_pull_request(MERGED));
}

#[test]
fn the_last_live_pull_request_takes_the_whole_mirror() {
    let cache = Cache::new("finished-whole");
    let git_dir = mirror(
        &cache.paths,
        "github.com_cli_cli.git",
        &[(MERGED, &["refs/review/pull/1/head", "refs/review/heads/main"])],
    );

    let sweeper = cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP);
    assert!(sweeper.forget_pull_request(&finished(MERGED)));
    assert!(!git_dir.exists(), "dropping refs alone would free nothing");
}

#[test]
fn refs_another_pull_request_fetched_before_tracking_keep_the_mirror() {
    let cache = Cache::new("finished-untracked");
    let git_dir =
        mirror(&cache.paths, "github.com_cli_cli.git", &[(MERGED, &["refs/review/pull/1/head"])]);
    test_repo::bare_with_refs(&git_dir, &["refs/review/pull/9/head"]);

    let sweeper = cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP);
    assert!(sweeper.forget_pull_request(&finished(MERGED)));
    assert_eq!(review_refs(&git_dir), ["refs/review/pull/9/head"]);
}

#[test]
fn a_mirror_in_use_keeps_its_folder_now_and_goes_at_the_next_sweep() {
    let cache = Cache::new("finished-in-use");
    cache.connections(&["gh-1"]);
    let git_dir =
        mirror(&cache.paths, "github.com_cli_cli.git", &[(MERGED, &["refs/review/pull/1/head"])]);

    let held = cache.sweeper(Arc::new(InUse(vec!["github.com_cli_cli.git"])), CacheLimits::APP);
    assert!(held.forget_pull_request(&finished(MERGED)));
    assert!(git_dir.exists());
    assert!(review_refs(&git_dir).is_empty(), "the refs go at once");

    cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP).sweep(SystemTime::now());
    assert!(!git_dir.exists(), "a mirror without live pull requests goes at the next sweep");
}

#[test]
fn a_log_on_screen_makes_the_cleanup_wait() {
    let cache = Cache::new("finished-leased");
    let shown = log(&cache.paths, MERGED, "job-1");
    let leases = Leases::default();
    let lease = leases.lease(&shown);
    let sweeper =
        super::sweep::Sweeper::new(cache.paths, leases, Arc::new(NothingInUse), CacheLimits::APP);

    assert!(!sweeper.forget_pull_request(&finished(MERGED)), "tried again later");
    assert!(shown.exists());
    drop(lease);
    assert!(sweeper.forget_pull_request(&finished(MERGED)));
    assert!(!shown.exists());
}

#[test]
fn a_pull_request_without_local_data_is_done_at_once() {
    let cache = Cache::new("finished-nothing");
    let other = log(&cache.paths, OPEN, "job-1");
    let sweeper = cache.sweeper(Arc::new(NothingInUse), CacheLimits::APP);
    assert!(sweeper.forget_pull_request(&finished(MERGED)));
    assert!(other.exists());
}

#[test]
fn the_open_pull_request_keeps_its_data_until_its_page_closes() {
    let cache = Cache::new("finished-open-page");
    let merged_log = log(&cache.paths, MERGED, "job-1");
    let merged_viewed = viewed(&cache.paths, MERGED);
    let schedule = Schedule {
        first: Duration::from_secs(3600),
        every: Duration::from_secs(3600),
        retry: Duration::from_millis(20),
    };
    let retention =
        Retention::start(cache.paths, Arc::new(NothingInUse), Arc::new(|| {}), schedule);

    let page = retention.hold_pull_request(MERGED);
    retention.pull_request_finished("gh-1", MERGED);
    // The thread takes requests in order: once this measurement is in, the finished pull request
    // was handled (and kept, its page being open).
    retention.measure();
    wait_until("the usage measured", || retention.status().usage.is_some());
    assert!(
        merged_log.exists() && merged_viewed.exists(),
        "nothing goes while the page (and its drafts) is open"
    );

    drop(page);
    wait_until("the retry cleans up once the page closed", || {
        !merged_log.exists() && !merged_viewed.exists()
    });
}

#[test]
fn logs_of_a_pull_request_share_a_prefix() {
    let paths = AppPaths::new(PathBuf::from("config"), PathBuf::from("cache"));
    let file = paths.pull_request_log_file("gh-1", MERGED, "job-1");
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with(&format!("{}-", stable_hash(MERGED))));
    assert_eq!(file.parent().unwrap(), paths.connection_logs_dir("gh-1"));
}
