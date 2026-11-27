//! The agent's checkout: a detached working tree of the pull request's head commit, made from the
//! page's git mirror under the cache folder (`agent-checkouts/<repository hash>-<number>`), so the
//! agent can read and grep the whole repository as of the pull request. A checkout already at the
//! head is reused as it is. At most [`MAX_CHECKOUTS`] stay (our own budget): the least recently
//! used go through `git worktree remove` on their mirror.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::SystemTime;

use cv_base::platform;
use review_git::GitMirror;
use review_store::{AppPaths, agent_checkouts_dir, stable_hash};

/// Checkouts kept; each is a full working tree of its repository.
pub const MAX_CHECKOUTS: usize = 5;

/// The checkout the agent reads, and its commit.
pub struct Checkout {
    pub dir: PathBuf,
    pub head: String,
}

/// The checkout folder of pull request `number` of `repository_key`.
pub fn dir(paths: &AppPaths, repository_key: &str, number: u64) -> PathBuf {
    agent_checkouts_dir(paths).join(format!("{}-{number}", stable_hash(repository_key)))
}

/// Puts the checkout at `dest` on `head` and drops the least recently used others. Returns the
/// checkout, or git's message.
pub async fn prepare(git: Arc<GitMirror>, head: String, dest: PathBuf) -> Result<Checkout, String> {
    if detached_head(&dest).as_deref() != Some(head.as_str()) {
        git.checkout_detached(&head, &dest).await.map_err(|error| error.to_string())?;
    }
    mark_used(&dest);
    if let Some(root) = dest.parent() {
        for old in least_recent(&used_checkouts(root), &dest, MAX_CHECKOUTS) {
            remove(&old).await;
        }
    }
    Ok(Checkout { dir: dest, head })
}

/// The commit a detached checkout is at, read from its git files without starting git.
fn detached_head(checkout: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir_of(checkout)?.join("HEAD")).ok()?;
    let head = head.trim();
    let is_commit = head.len() >= 40 && head.bytes().all(|byte| byte.is_ascii_hexdigit());
    is_commit.then(|| head.to_owned())
}

/// The worktree's own folder in the mirror (`<mirror>/worktrees/<name>`), from its `.git` file.
fn git_dir_of(checkout: &Path) -> Option<PathBuf> {
    let link = std::fs::read_to_string(checkout.join(".git")).ok()?;
    let dir = link.trim().strip_prefix("gitdir:")?.trim();
    let dir = Path::new(dir);
    Some(if dir.is_absolute() { dir.to_path_buf() } else { checkout.join(dir) })
}

/// Using a checkout stamps its `.git` file, which orders the checkouts by last use.
fn mark_used(checkout: &Path) {
    if let Ok(file) = std::fs::File::options().write(true).open(checkout.join(".git")) {
        // A checkout without the stamp only goes earlier when the budget is full.
        let _unstamped = file.set_modified(SystemTime::now());
    }
}

fn used_checkouts(root: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .map(|path| {
            let used = path
                .join(".git")
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (path, used)
        })
        .collect()
}

/// The checkouts to remove so that `budget` stay, `keep` among them: the least recently used.
fn least_recent(checkouts: &[(PathBuf, SystemTime)], keep: &Path, budget: usize) -> Vec<PathBuf> {
    let mut others: Vec<&(PathBuf, SystemTime)> =
        checkouts.iter().filter(|(path, _)| path != keep).collect();
    let room = budget.saturating_sub(1);
    if others.len() <= room {
        return Vec::new();
    }
    others.sort_by_key(|(_, used)| *used);
    let excess = others.len() - room;
    others.into_iter().take(excess).map(|(path, _)| path.clone()).collect()
}

/// Removes a checkout through its mirror, so the mirror forgets it too; a checkout whose mirror
/// is gone (retention dropped it) is deleted as a folder.
async fn remove(checkout: &Path) {
    let mirror = git_dir_of(checkout)
        .and_then(|dir| dir.parent().and_then(Path::parent).map(Path::to_path_buf));
    if let Some(mirror) = mirror {
        let mut command = tokio::process::Command::new("git");
        command
            .arg("--git-dir")
            .arg(&mirror)
            .args(["worktree", "remove", "--force"])
            .arg(checkout)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        platform::hide_console(&mut command);
        if command.status().await.is_ok_and(|status| status.success()) {
            return;
        }
    }
    // What is left is only files; the next checkout of its mirror prunes the registration.
    let _left_for_later = std::fs::remove_dir_all(checkout);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn the_least_recently_used_checkouts_go_and_the_current_one_stays() {
        let at = |seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        let checkouts: Vec<(PathBuf, SystemTime)> = [("a", 5), ("b", 1), ("c", 9), ("d", 3)]
            .into_iter()
            .map(|(name, used)| (PathBuf::from(name), at(used)))
            .collect();
        assert_eq!(least_recent(&checkouts, Path::new("b"), 2), [PathBuf::from("d"), "a".into()]);
        assert_eq!(least_recent(&checkouts, Path::new("new"), 4), [PathBuf::from("b")]);
        assert!(least_recent(&checkouts, Path::new("a"), 5).is_empty(), "within the budget");
    }
}
