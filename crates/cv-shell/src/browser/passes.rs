//! Pull requests Code View itself just opened in the browser ("Open on GitHub", any
//! `ctx.open_url` of a pull request page). Without this the extension would see that navigation,
//! ask the host, and send the page straight back to the window it came from.
//!
//! The app records every URL a frame opens that [`link_access::locate`] resolves to a pull request
//! ([`Recorder`]), keyed by the pull request rather than the URL, so `/files`, a fragment and the
//! `visualstudio.com` host all match. The host's `check` asks [`contains`] right after
//! [`link_access::locate`] and answers `pass` within [`TTL`], without a read. No URL is changed: a marker in the query or
//! fragment would show up in the browser and in links the user copies from there.
//!
//! The file is `passes.json` in the browser folder, shared by two processes: the app writes it,
//! the host only reads it. Every write goes through a temp file and a rename, so the host never
//! reads half a list, and drops the entries past [`TTL`], so the file stays a handful of lines.
//! Writes of the app run one at a time on the background runtime (never disk on the UI thread),
//! each reading the file it replaces, so a later write never loses an earlier pass. A file that
//! is missing or unreadable is an empty list: the extension hands the page back to Code View.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use review_core::Connection;
use review_store::AppPaths;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use cv_runtime::background::Spawner;
use cv_runtime::route::PullRequestKey;

use crate::links::link_access::{self, Target};

/// How long a pull request the app opened stays in the browser. Our own budget: the browser
/// starts loading the page well within a second of the app's request, and a minute also covers a
/// browser that has to start first.
pub const TTL: Duration = Duration::from_secs(60);

/// The pass list next to the host manifest.
pub fn file(paths: &AppPaths) -> PathBuf {
    paths.browser_dir().join("passes.json")
}

/// One pull request the app opened, and when, in milliseconds since the Unix epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Pass {
    connection: String,
    repository: String,
    number: u64,
    at_ms: u64,
}

impl Pass {
    fn is_for(&self, key: &PullRequestKey) -> bool {
        self.number == key.number
            && self.connection == key.connection_id
            && self.repository == key.repository_key
    }

    /// Recorded within [`TTL`] before `now`; one stamped after `now` (the clock went back) is not.
    fn is_fresh(&self, now: SystemTime) -> bool {
        let now = millis(now);
        self.at_ms <= now && now - self.at_ms < millis_of(TTL)
    }
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, millis_of)
}

fn millis_of(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The list in `path`; empty when it is missing or unreadable. An entry that lacks a field is
/// skipped, not the whole list.
fn load(path: &Path) -> Vec<Pass> {
    let entries = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<Value>>(&bytes).ok())
        .unwrap_or_default();
    entries.into_iter().filter_map(|entry| serde_json::from_value::<Pass>(entry).ok()).collect()
}

/// Adds `key` at `now` to the list in `path`, dropping the entries past [`TTL`] and an older pass
/// of the same pull request.
pub fn record(path: &Path, key: &PullRequestKey, now: SystemTime) -> io::Result<()> {
    let mut passes = load(path);
    passes.retain(|pass| pass.is_fresh(now) && !pass.is_for(key));
    passes.push(Pass {
        connection: key.connection_id.clone(),
        repository: key.repository_key.clone(),
        number: key.number,
        at_ms: millis(now),
    });
    let folder = path.parent().ok_or_else(|| io::Error::other("the pass list has no folder"))?;
    std::fs::create_dir_all(folder)?;
    review_store::write_bytes(path, &serde_json::to_vec(&passes)?)
}

/// The app opened `key` in the browser within [`TTL`] before `now`.
pub fn contains(path: &Path, key: &PullRequestKey, now: SystemTime) -> bool {
    load(path).iter().any(|pass| pass.is_for(key) && pass.is_fresh(now))
}

/// The app's side: hands each pull request a frame opens in the browser to the pass list.
pub struct Recorder {
    path: PathBuf,
    spawner: Spawner,
    /// One write at a time, so each reads the list the one before it wrote.
    writing: Arc<Mutex<()>>,
}

impl Recorder {
    pub fn new(paths: &AppPaths, spawner: Spawner) -> Self {
        Self { path: file(paths), spawner, writing: Arc::default() }
    }

    /// After the frame: the URLs it asked the system to open that name a pull request of
    /// `connections` go to the list on the background runtime. Locating them sends nothing.
    pub fn note_frame(&self, ctx: &egui::Context, connections: &[Connection]) {
        let urls: Vec<String> = ctx.output(|output| {
            output
                .commands
                .iter()
                .filter_map(|command| match command {
                    egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                    _ => None,
                })
                .collect()
        });
        for url in urls {
            if let Target::PullRequest { key, .. } = link_access::locate(&url, connections) {
                self.record(key);
            }
        }
    }

    fn record(&self, key: PullRequestKey) {
        let (path, writing) = (self.path.clone(), self.writing.clone());
        // Kept: a click on "Open on GitHub" right before quitting still lands. A failed write
        // has nobody to tell; the extension then hands the page back.
        self.spawner.spawn_kept(async move {
            let _unreported = tokio::task::spawn_blocking(move || {
                let _one_at_a_time = writing.lock().unwrap_or_else(PoisonError::into_inner);
                record(&path, &key, SystemTime::now())
            })
            .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use review_core::{Connection, ProviderKind};

    use super::{TTL, contains, file, record};
    use cv_runtime::route::PullRequestKey;
    use cv_testkit::{ScratchDir, samples};

    use crate::links::link_access::{self, Target};

    fn connections() -> [Connection; 2] {
        let connection = |id: &str, kind, base_url: &str| Connection {
            organizations: vec!["acme".to_owned()],
            ..samples::connection(id, kind, base_url)
        };
        [
            connection("gh", ProviderKind::GitHub, "https://github.com"),
            connection("ado", ProviderKind::AzureDevOps, "https://dev.azure.com"),
        ]
    }

    fn key_of(url: &str) -> PullRequestKey {
        match link_access::locate(url, &connections()) {
            Target::PullRequest { key, .. } => key,
            other => panic!("{url} is no pull request: {other:?}"),
        }
    }

    fn list(scratch: &ScratchDir) -> std::path::PathBuf {
        file(&scratch.app_paths())
    }

    #[test]
    fn a_recorded_pull_request_matches_every_form_of_its_link() {
        let scratch = ScratchDir::new("passes-forms");
        let path = list(&scratch);
        let now = SystemTime::now();
        record(&path, &key_of("https://github.com/acme/widgets/pull/7"), now).expect("recorded");
        record(&path, &key_of("https://dev.azure.com/acme/app/_git/app/pullrequest/12"), now)
            .expect("recorded");
        for url in [
            "https://github.com/acme/widgets/pull/7/files",
            "https://github.com/acme/widgets/pull/7#discussion_r1",
            "https://acme.visualstudio.com/app/_git/app/pullrequest/12",
        ] {
            assert!(contains(&path, &key_of(url), now), "{url}");
        }
        for url in
            ["https://github.com/acme/widgets/pull/8", "https://github.com/acme/gadgets/pull/7"]
        {
            assert!(!contains(&path, &key_of(url), now), "{url}");
        }
    }

    #[test]
    fn a_pass_expires_after_its_ttl_and_the_next_write_drops_it() {
        let scratch = ScratchDir::new("passes-expiry");
        let path = list(&scratch);
        let old = key_of("https://github.com/acme/widgets/pull/1");
        let start = SystemTime::now();
        record(&path, &old, start).expect("recorded");
        let second = Duration::from_secs(1);
        assert!(contains(&path, &old, start + TTL - second));
        assert!(!contains(&path, &old, start + TTL));
        assert!(!contains(&path, &old, start - second), "stamped after the clock went back");

        let later = start + TTL;
        record(&path, &key_of("https://github.com/acme/widgets/pull/2"), later).expect("recorded");
        let text = std::fs::read_to_string(&path).expect("the list");
        assert_eq!(text.matches("\"number\"").count(), 1, "only the fresh pass is left: {text}");
        let folder = path.parent().expect("folder");
        let files = std::fs::read_dir(folder).expect("the folder").count();
        assert_eq!(files, 1, "only passes.json is left, no temp file");
    }

    #[test]
    fn a_missing_or_corrupt_list_holds_no_pass_and_is_replaced_by_the_next_write() {
        let scratch = ScratchDir::new("passes-corrupt");
        let path = list(&scratch);
        let key = key_of("https://github.com/acme/widgets/pull/7");
        let now = SystemTime::now();
        assert!(!contains(&path, &key, now), "no list yet");
        std::fs::create_dir_all(path.parent().expect("folder")).expect("folder");
        std::fs::write(&path, b"{ not json").expect("corrupt");
        assert!(!contains(&path, &key, now));
        record(&path, &key, now).expect("recorded over the corrupt list");
        assert!(contains(&path, &key, now));
    }
}
