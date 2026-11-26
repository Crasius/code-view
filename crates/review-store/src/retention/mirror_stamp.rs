//! A small file inside each mirror's git directory: when the app last used the mirror, which
//! connections did, and which refs each pull request fetched. Git ignores unknown files there.
//! Mirrors are shared by clone URL, so a mirror goes with a removed connection or a finished pull
//! request only when nothing else uses it.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::atomic_file;

const STAMP_FILE: &str = "review-app-usage.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorStamp {
    /// Seconds since the Unix epoch.
    last_used: u64,
    connections: BTreeSet<String>,
    /// Local refs (`refs/review/...`) per pull request key (`connection|repository|number`).
    #[serde(default)]
    pull_requests: BTreeMap<String, BTreeSet<String>>,
}

/// One use of a mirror: by which connection, for which pull request, with which local refs.
pub struct MirrorUse<'a> {
    pub connection_id: &'a str,
    pub pull_request: &'a str,
    pub local_refs: &'a [String],
}

impl MirrorStamp {
    pub fn last_used(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.last_used)
    }

    pub fn connections(&self) -> &BTreeSet<String> {
        &self.connections
    }

    /// Drops connections that no longer exist, with their pull requests; true when any was dropped.
    pub fn retain_connections(&mut self, is_known: impl Fn(&str) -> bool) -> bool {
        let before = self.connections.len();
        self.connections.retain(|id| is_known(id));
        self.pull_requests.retain(|key, _| key.split('|').next().is_some_and(&is_known));
        self.connections.len() != before
    }

    pub fn has_pull_request(&self, key: &str) -> bool {
        self.pull_requests.contains_key(key)
    }

    /// Forgets `key` and returns the refs only it used (refs another pull request shares stay).
    pub fn take_pull_request(&mut self, key: &str) -> Vec<String> {
        let Some(refs) = self.pull_requests.remove(key) else {
            return Vec::new();
        };
        refs.into_iter()
            .filter(|name| !self.pull_requests.values().any(|other| other.contains(name)))
            .collect()
    }

    pub fn has_pull_requests(&self) -> bool {
        !self.pull_requests.is_empty()
    }
}

fn stamp_path(git_dir: &Path) -> PathBuf {
    git_dir.join(STAMP_FILE)
}

/// The stamp of a mirror; `None` for mirrors made before stamps existed or with a damaged stamp.
pub fn read(git_dir: &Path) -> Option<MirrorStamp> {
    atomic_file::read_json(&stamp_path(git_dir)).ok().flatten()
}

pub fn write(git_dir: &Path, stamp: &MirrorStamp) -> io::Result<()> {
    atomic_file::write_json(&stamp_path(git_dir), stamp)
}

/// Marks the mirror as used at `now` by `usage`.
pub fn record_use(git_dir: &Path, usage: &MirrorUse<'_>, now: SystemTime) -> io::Result<()> {
    // Pages of one mirror record at the same time; without the lock, each would write back the
    // stamp it read and drop the other's pull request.
    static RECORDING: Mutex<()> = Mutex::new(());
    let _recording = RECORDING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut stamp = read(git_dir).unwrap_or_default();
    stamp.last_used = now.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    stamp.connections.insert(usage.connection_id.to_owned());
    stamp
        .pull_requests
        .entry(usage.pull_request.to_owned())
        .or_default()
        .extend(usage.local_refs.iter().cloned());
    write(git_dir, &stamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDir;

    fn refs(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn records_last_use_every_connection_and_pull_request() {
        let dir = TestDir::new("stamp");
        let git_dir = dir.path().join("github.com_cli_cli.git");
        let first = UNIX_EPOCH + Duration::from_secs(1_000);
        let first_refs = refs(&["refs/review/pull/1/head", "refs/review/heads/main"]);
        record_use(
            &git_dir,
            &MirrorUse {
                connection_id: "gh-1",
                pull_request: "gh-1|cli/cli|1",
                local_refs: &first_refs,
            },
            first,
        )
        .unwrap();
        let second_refs = refs(&["refs/review/pull/2/head", "refs/review/heads/main"]);
        let later = first + Duration::from_secs(60);
        record_use(
            &git_dir,
            &MirrorUse {
                connection_id: "gh-2",
                pull_request: "gh-2|cli/cli|2",
                local_refs: &second_refs,
            },
            later,
        )
        .unwrap();

        let mut stamp = read(&git_dir).unwrap();
        assert_eq!(stamp.last_used(), later);
        assert_eq!(
            stamp.connections().iter().map(String::as_str).collect::<Vec<_>>(),
            ["gh-1", "gh-2"]
        );
        assert_eq!(
            stamp.take_pull_request("gh-1|cli/cli|1"),
            ["refs/review/pull/1/head"],
            "the shared target ref stays"
        );
        assert_eq!(
            stamp.take_pull_request("gh-2|cli/cli|2"),
            ["refs/review/heads/main", "refs/review/pull/2/head"]
        );
        assert!(!stamp.has_pull_requests());
    }

    /// Pull request pages of one mirror open at the same time: each one's use is kept, none
    /// overwrites another's with the stamp it read before.
    #[test]
    fn uses_recorded_at_the_same_time_are_all_kept() {
        let dir = TestDir::new("stamp-concurrent");
        let git_dir = std::sync::Arc::new(dir.path().join("github.com_cli_cli.git"));
        let start = std::sync::Arc::new(std::sync::Barrier::new(8));
        let pages: Vec<_> = (0..8)
            .map(|number| {
                let (git_dir, start) = (git_dir.clone(), start.clone());
                std::thread::spawn(move || {
                    let key = format!("gh-1|cli/cli|{number}");
                    let local_refs = refs(&[&format!("refs/review/pull/{number}/head")]);
                    let usage = MirrorUse {
                        connection_id: "gh-1",
                        pull_request: &key,
                        local_refs: &local_refs,
                    };
                    start.wait();
                    record_use(&git_dir, &usage, SystemTime::now())
                })
            })
            .collect();
        for page in pages {
            page.join().expect("the page ran").expect("every use is recorded");
        }
        let stamp = read(&git_dir).expect("a whole stamp");
        for number in 0..8 {
            assert!(stamp.has_pull_request(&format!("gh-1|cli/cli|{number}")), "{number} lost");
        }
    }
}
