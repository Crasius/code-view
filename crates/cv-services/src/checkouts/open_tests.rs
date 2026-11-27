//! `Checkouts::open` on a records file an earlier build wrote: its log lines come back redacted.

use std::path::PathBuf;

use cv_runtime::route::PullRequestKey;
use cv_testkit::ScratchDir;
use review_store::AppPaths;
use review_store::checkouts::{CheckoutKind, CheckoutRecord, CheckoutRecords, OpenWith};

use super::Checkouts;

const TOKEN: &str = "ghp_secret123";

/// The two kinds of lines that carry git's message, as builds before the redaction at storage
/// wrote them, between lines without a credential.
const RAW_LOG: [&str; 4] = [
    "2026-11-20 09:00:01  Checked out feature/x in 3.1 s",
    "2026-11-21 10:00:02  Not updated: git refused: fatal: unable to access https://ci:ghp_secret123@github.com/acme/app",
    "2026-11-22 11:00:03  Updated to abc1234 (fast-forward)",
    "2026-11-23 12:00:04  Pull request done; worktree kept: git refused: fatal: unable to access 'https://ci:ghp_secret123@github.com/acme/app/': 403",
];

const CLEAN_LOG: [&str; 4] = [
    "2026-11-20 09:00:01  Checked out feature/x in 3.1 s",
    "2026-11-21 10:00:02  Not updated: git refused: fatal: unable to access https://github.com/acme/app",
    "2026-11-22 11:00:03  Updated to abc1234 (fast-forward)",
    "2026-11-23 12:00:04  Pull request done; worktree kept: git refused: fatal: unable to access 'https://github.com/acme/app/': 403",
];

fn key() -> PullRequestKey {
    PullRequestKey { connection_id: "gh".into(), repository_key: "acme/app".into(), number: 7 }
}

/// A records file with one record whose log is `RAW_LOG`.
fn write_raw_records(paths: &AppPaths) {
    let mut records = CheckoutRecords::load(paths.checkouts_file()).expect("no file yet");
    let record = CheckoutRecord {
        connection_id: "gh".into(),
        repository_key: "acme/app".into(),
        number: 7,
        clone: PathBuf::from("/dev/app"),
        path: PathBuf::from("/dev/app-wt/pr-7-x"),
        kind: CheckoutKind::Worktree,
        branch: "feature/x".into(),
        is_own_branch: true,
        pull_ref: "refs/codeview/pull/7/head".into(),
        head: "abc1234".into(),
        last_opened: OpenWith::Nothing,
        took_ms: 3_100,
        merged_here: false,
        log: RAW_LOG.map(str::to_owned).to_vec(),
    };
    records.put(record).expect("saved");
}

/// Regression: before the lines were redacted where they are built, the Log panel hid a token in
/// them only on screen; a file of that time showed it raw once the panel stopped redacting.
#[test]
fn an_old_records_file_opens_with_its_log_redacted_and_is_saved_so() {
    let dir = ScratchDir::new("checkouts-open-redacts");
    let paths = dir.app_paths();
    write_raw_records(&paths);

    let (checkouts, error) = Checkouts::open(&paths);

    assert_eq!(error, None);
    let record = checkouts.record(&key()).expect("the record is loaded");
    assert_eq!(record.log, CLEAN_LOG, "only the credentials go, every line stays in order");
    let saved = std::fs::read_to_string(paths.checkouts_file()).expect("the file is there");
    assert!(!saved.contains(TOKEN), "the file keeps no token: {saved}");
    let reloaded = CheckoutRecords::load(paths.checkouts_file()).expect("loads");
    let reloaded = &reloaded.all().first().expect("one record").log;
    assert_eq!(reloaded, &CLEAN_LOG, "the file holds the redacted lines");
}

/// A redacted file that cannot be saved still opens clean and says once that the save failed.
/// Windows refuses to replace a read-only file; elsewhere a rename replaces it all the same.
#[cfg(windows)]
#[test]
fn a_failed_save_of_the_redacted_file_is_reported_and_the_log_stays_clean() {
    let dir = ScratchDir::new("checkouts-open-save-fails");
    let paths = dir.app_paths();
    write_raw_records(&paths);
    let file = paths.checkouts_file();
    let set_readonly = |readonly: bool| {
        let mut permissions = std::fs::metadata(&file).expect("the file is there").permissions();
        permissions.set_readonly(readonly);
        std::fs::set_permissions(&file, permissions).expect("permissions set");
    };
    set_readonly(true);

    let (checkouts, error) = Checkouts::open(&paths);
    let saved = std::fs::read_to_string(&file).expect("the file is there");
    set_readonly(false);

    let error = error.expect("the failed save is reported");
    assert!(error.starts_with("Could not save the list of local checkouts: "), "{error}");
    let record = checkouts.record(&key()).expect("the record is loaded");
    assert_eq!(record.log, CLEAN_LOG, "the records in memory are clean all the same");
    assert!(saved.contains(TOKEN), "the read-only file kept its old content");
}
