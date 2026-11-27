//! What the crate's tests share: a temporary folder per test with the app's folders in it, the
//! connection list and other files written as the app writes them, a bounded wait for work a
//! background thread does, and the problem of a token store that refuses every token.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::retention::remove_tree;
use crate::{AppPaths, LINUX_HELP_URL, VaultProblem};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A temporary folder for one test, removed when the test ends.
pub struct TestDir(PathBuf);

impl TestDir {
    pub fn new(name: &str) -> Self {
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("review-store-{name}-{}-{unique}", std::process::id()));
        remove_tree(&path).expect("remove what an earlier run with this process id left");
        std::fs::create_dir_all(&path).expect("create the test folder");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The app's config and cache folders inside this folder.
    pub fn paths(&self) -> AppPaths {
        AppPaths::new(self.0.join("config"), self.0.join("cache"))
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        // Best effort: a folder left in the temp directory harms nothing, and a panic in drop
        // would hide the test's own failure.
        let _cleanup_is_best_effort = remove_tree(&self.0);
    }
}

/// Writes the connection list with `ids`, as Settings saves it.
pub fn write_connections(paths: &AppPaths, ids: &[&str]) {
    let list: Vec<String> = ids
        .iter()
        .map(|id| {
            format!(r#"{{"id":"{id}","name":"n","kind":"GitHub","base_url":"https://github.com"}}"#)
        })
        .collect();
    write_file(
        &paths.connections_file(),
        format!(r#"{{"connections":[{}]}}"#, list.join(",")).as_bytes(),
    );
}

/// Writes `bytes` to `path`, creating its folders, and returns the path.
pub fn write_file(path: &Path, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(path.parent().expect("a file in a folder"))
        .expect("create the folders");
    std::fs::write(path, bytes).expect("write the file");
    path.to_owned()
}

/// Polls `condition` until it holds or ten seconds pass; true when it held.
#[expect(
    clippy::disallowed_methods,
    reason = "waits for a background thread that reports to nobody; returns as soon as the \
              condition holds"
)]
pub fn wait_for(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    condition()
}

/// What a Linux session without a keyring answers, for [`crate::TokenVault::Refusing`].
pub fn no_keyring() -> &'static VaultProblem {
    static PROBLEM: LazyLock<VaultProblem> = LazyLock::new(|| VaultProblem {
        title: "No keyring".to_owned(),
        fix: "Start one.".to_owned(),
        commands: Vec::new(),
        help_url: LINUX_HELP_URL,
    });
    &PROBLEM
}
