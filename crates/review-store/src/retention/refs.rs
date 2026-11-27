//! The review refs of a mirror, read and deleted with the local git CLI. Offline: these commands
//! never talk to a remote. Dropping refs frees no space by itself (the objects stay in the packs);
//! it lets a mirror with no live pull request left go whole.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::platform;

const REVIEW_REFS: &str = "refs/review/";

/// Every ref under `refs/review/`, loose or packed.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the retention worker thread, which may wait on git"
)]
pub(crate) fn list(git_dir: &Path) -> io::Result<Vec<String>> {
    let output =
        git(git_dir).args(["for-each-ref", "--format=%(refname)", REVIEW_REFS]).output()?;
    if !output.status.success() {
        return Err(failure("for-each-ref", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .filter(|name| !name.is_empty())
        .collect())
}

/// Deletes `names` in one transaction: all go or none (another git command holding a lock).
pub(crate) fn delete(git_dir: &Path, names: &[String]) -> io::Result<()> {
    let names: Vec<&String> = names.iter().filter(|name| name.starts_with(REVIEW_REFS)).collect();
    if names.is_empty() {
        return Ok(());
    }
    let mut child = git(git_dir).args(["update-ref", "--stdin"]).stdin(Stdio::piped()).spawn()?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| io::Error::other("git stdin missing"))?;
        for name in names {
            writeln!(stdin, "delete {name}")?;
        }
    }
    let output = child.wait_with_output()?;
    if output.status.success() { Ok(()) } else { Err(failure("update-ref", &output.stderr)) }
}

fn git(git_dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("--git-dir")
        .arg(git_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1");
    platform::hide_console(&mut command);
    command
}

fn failure(what: &str, stderr: &[u8]) -> io::Error {
    io::Error::other(format!("git {what} failed: {}", String::from_utf8_lossy(stderr).trim()))
}

#[cfg(test)]
pub(crate) mod test_repo {
    use std::path::Path;
    use std::process::Command;

    /// A bare repository with one commit and `refs` pointing at it, like a mirror after a fetch.
    pub fn bare_with_refs(git_dir: &Path, refs: &[&str]) {
        std::fs::create_dir_all(git_dir).unwrap();
        run(git_dir, &["init", "--bare", "--quiet"]);
        let tree = run(git_dir, &["hash-object", "-t", "tree", "-w", "--stdin"]);
        let commit = run(
            git_dir,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.org",
                "commit-tree",
                &tree,
                "-m",
                "c",
            ],
        );
        for name in refs {
            run(git_dir, &["update-ref", name, &commit]);
        }
    }

    #[expect(clippy::disallowed_methods, reason = "test fixture setup runs git to completion")]
    fn run(git_dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(git_dir)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDir;

    #[test]
    fn deletes_only_the_named_review_refs() {
        let dir = TestDir::new("refs");
        let git_dir = dir.path().join("m.git");
        test_repo::bare_with_refs(
            &git_dir,
            &["refs/review/pull/1/head", "refs/review/pull/2/head", "refs/heads/keep"],
        );

        delete(&git_dir, &["refs/review/pull/1/head".to_owned(), "refs/heads/keep".to_owned()])
            .unwrap();
        assert_eq!(list(&git_dir).unwrap(), ["refs/review/pull/2/head"]);
    }
}
