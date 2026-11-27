//! The repositories the crate's tests build with git: a temp folder per test, an origin that
//! serves blob-less clones over `file://`, and the files and commits in it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use review_core::GitRemote;

/// A temporary folder for one test, removed when the test ends.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    /// `review-git-<name>-<process id>` in the temp folder; `name` is unique among the tests.
    pub(crate) fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("review-git-{name}-{}", std::process::id()));
        // A folder an earlier run with the same process id left behind; usually there is none.
        let _stale_folder_may_be_missing = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the scratch folder");
        Self(path)
    }

    /// An empty repository in `origin/` on `main` that serves blob-less clones and fetches by id.
    pub(crate) fn origin(&self) -> PathBuf {
        let origin = self.0.join("origin");
        std::fs::create_dir_all(&origin).expect("create the origin folder");
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["config", "uploadpack.allowFilter", "true"]);
        git(&origin, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
        origin
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a folder left in the temp directory harms nothing.
        let _cleanup_is_best_effort = std::fs::remove_dir_all(&self.0);
    }
}

/// `origin` as the remote a mirror fetches from.
pub(crate) fn file_remote(origin: &Path) -> GitRemote {
    GitRemote {
        clone_url: format!("file:///{}", origin.display().to_string().replace('\\', "/")),
        authorization_header: None,
    }
}

/// Runs git in `dir` as a test author, with no line-ending conversion; returns trimmed stdout.
#[expect(
    clippy::disallowed_methods,
    reason = "test driver: builds the fixture repositories with git before the code under test runs"
)]
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.org"])
        .args(["-c", "core.autocrlf=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

pub(crate) fn write(dir: &Path, path: &str, content: impl AsRef<[u8]>) {
    let full = dir.join(path);
    std::fs::create_dir_all(full.parent().expect("a file path has a folder")).expect("mkdir");
    std::fs::write(full, content).expect("write the file");
}

/// `{prefix}line 1\n` up to `{prefix}line {count}\n`.
pub(crate) fn numbered_lines(prefix: &str, count: u32) -> String {
    (1..=count).fold(String::new(), |mut text, line| {
        writeln!(text, "{prefix}line {line}").expect("writing to a String cannot fail");
        text
    })
}
