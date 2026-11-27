//! Questions to a clone or worktree of the user's: its branch and changed files, its worktrees,
//! where a branch points and how two commits relate. Nothing here writes.

// RS-DONT-04: parses git's output.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use review_core::{ProviderError, Result};

use crate::command::GitCommand;

/// A clone of the user's, or one of its worktrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRepo {
    dir: PathBuf,
}

/// The branch a working tree is on and what is changed in it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TreeState {
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    /// `git status --porcelain` lines: `M  src/a.rs`, `?? notes.md`.
    pub changes: Vec<String>,
}

impl TreeState {
    /// No tracked file is changed; untracked files may be there.
    pub fn is_clean(&self) -> bool {
        self.changes.iter().all(|line| line.starts_with("??"))
    }

    /// The changed tracked files' lines, untracked ones left out.
    pub fn tracked_changes(&self) -> impl Iterator<Item = &str> + '_ {
        self.changes.iter().map(String::as_str).filter(|line| !line.starts_with("??"))
    }
}

/// One working tree of a clone, the clone's own among them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The branch checked out there; `None` when detached.
    pub branch: Option<String>,
}

impl LocalRepo {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn git(&self) -> GitCommand<'_> {
        GitCommand::in_tree(&self.dir)
    }

    /// The branch checked out here and the changed files.
    pub(crate) async fn state(&self) -> Result<TreeState> {
        let output = self.git().run(["status", "--porcelain=v1", "--branch"]).await?;
        Ok(parse_status(&String::from_utf8_lossy(&output)))
    }

    /// Every working tree of the clone, its own first. git prints each path with its symbolic
    /// links resolved (macOS' `/var` is `/private/var`, a user's `~/dev` may link to another
    /// disk), so this folder's own tree comes back in the spelling this `LocalRepo` was given:
    /// callers compare it with that path by [`same_folder`].
    pub(crate) async fn worktrees(&self) -> Result<Vec<Worktree>> {
        let output = self.git().run(["worktree", "list", "--porcelain"]).await?;
        let mut trees = parse_worktrees(&String::from_utf8_lossy(&output));
        let resolved = tokio::fs::canonicalize(&self.dir).await.ok();
        for tree in &mut trees {
            let is_this_folder = same_folder(&tree.path, &self.dir)
                || match &resolved {
                    Some(resolved) => tokio::fs::canonicalize(&tree.path)
                        .await
                        .is_ok_and(|path| same_folder(&path, resolved)),
                    None => false,
                };
            if is_this_folder {
                tree.path.clone_from(&self.dir);
            }
        }
        Ok(trees)
    }

    /// The working tree `branch` is checked out in, if any.
    pub(crate) async fn checked_out_at(&self, branch: &str) -> Result<Option<PathBuf>> {
        let worktrees = self.worktrees().await?;
        Ok(worktrees
            .into_iter()
            .find(|tree| tree.branch.as_deref() == Some(branch))
            .map(|t| t.path))
    }

    /// The commit `name` (a ref or a commit id) points to, or `None` when there is none. A git
    /// failure, such as a folder that is no repository, is an error.
    pub(crate) async fn resolve(&self, name: &str) -> Result<Option<String>> {
        let spec = format!("{name}^{{commit}}");
        let found =
            self.git().run_or_none(["rev-parse", "--verify", "--quiet", spec.as_str()]).await?;
        Ok(found.map(|output| String::from_utf8_lossy(&output).trim().to_owned()))
    }

    /// The tip of the local branch `branch`.
    pub async fn branch_tip(&self, branch: &str) -> Result<Option<String>> {
        self.resolve(&format!("refs/heads/{branch}")).await
    }

    /// True when the clone has `refs/remotes/<remote>/<branch>`: the branch lives in the
    /// repository that remote points to.
    pub(crate) async fn has_remote_branch(&self, remote: &str, branch: &str) -> Result<bool> {
        Ok(self.resolve(&format!("refs/remotes/{remote}/{branch}")).await?.is_some())
    }

    /// Commits reachable from `from` but not from `to`.
    async fn count(&self, from: &str, to: &str) -> Result<usize> {
        let range = format!("{to}..{from}");
        let output = self.git().run(["rev-list", "--count", range.as_str()]).await?;
        parse_count(&String::from_utf8_lossy(&output))
    }

    /// True when `ancestor` is `descendant` or one of its ancestors.
    pub(crate) async fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        Ok(self.count(ancestor, descendant).await? == 0)
    }

    /// The commits of `head` that `local` lacks.
    pub async fn behind(&self, local: &str, head: &str) -> Result<usize> {
        self.count(head, local).await
    }

    /// `4be19a0  wip: try shorter ttl` for each commit of `tip` that `base` lacks, newest first.
    pub(crate) async fn commits_missing_from(&self, tip: &str, base: &str) -> Result<Vec<String>> {
        let range = format!("{base}..{tip}");
        let args: [&OsStr; 4] =
            ["log".as_ref(), "--no-color".as_ref(), "--format=%h  %s".as_ref(), range.as_ref()];
        let output = self.git().run(args).await?;
        Ok(String::from_utf8_lossy(&output).lines().map(str::to_owned).collect())
    }
}

/// `git rev-list --count`'s one number. Anything else is an error: read as 0, it would make every
/// commit an ancestor and move a branch.
fn parse_count(output: &str) -> Result<usize> {
    let text = output.trim();
    text.parse().map_err(|_| {
        ProviderError::Local(format!("unexpected `git rev-list --count` output: {text}"))
    })
}

/// `git status --porcelain=v1 --branch`: the `## branch...upstream` line, then one per change.
fn parse_status(output: &str) -> TreeState {
    let mut state = TreeState::default();
    for line in output.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            state.branch = branch_of_header(header);
        } else if !line.trim().is_empty() {
            state.changes.push(line.to_owned());
        }
    }
    state
}

/// `main...origin/main [behind 2]` → `main`; `No commits yet on main` → `main`; detached → `None`.
fn branch_of_header(header: &str) -> Option<String> {
    if header.starts_with("HEAD (no branch)") {
        return None;
    }
    let header = header
        .strip_prefix("No commits yet on ")
        .or_else(|| header.strip_prefix("Initial commit on "))
        .unwrap_or(header);
    let name = header.split("...").next().unwrap_or(header);
    let name = name.split(' ').next().unwrap_or(name);
    (!name.is_empty()).then(|| name.to_owned())
}

/// `git worktree list --porcelain`: blocks of `worktree <path>`, `HEAD <id>` and `branch
/// refs/heads/<name>` (or `detached`, `bare`), separated by empty lines.
fn parse_worktrees(output: &str) -> Vec<Worktree> {
    let mut trees = Vec::new();
    let mut current: Option<Worktree> = None;
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            trees.extend(current.take());
            // git writes `/` on every platform; the app shows and compares native paths.
            let path = path.replace('/', std::path::MAIN_SEPARATOR_STR);
            current = Some(Worktree { path: PathBuf::from(path), branch: None });
        } else if let Some(name) = line.strip_prefix("branch ")
            && let Some(tree) = current.as_mut()
        {
            tree.branch = Some(name.strip_prefix("refs/heads/").unwrap_or(name).to_owned());
        }
    }
    trees.extend(current);
    trees
}

/// True when `first` and `second` name the same folder as git and the system print them: `/` or
/// `\`, a trailing separator, and on Windows the case do not matter.
pub fn same_folder(first: &Path, second: &Path) -> bool {
    let normal = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        let text = text.trim_end_matches('/').to_owned();
        if cfg!(windows) { text.to_lowercase() } else { text }
    };
    normal(first) == normal(second)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{LocalRepo, parse_count, parse_status, parse_worktrees, same_folder};
    use crate::test_repo::{Scratch, git, write};

    /// Regression: an unreadable count read as 0, so every commit was an ancestor and a branch
    /// moved to the pull request's head.
    #[test]
    fn a_count_that_is_no_number_is_an_error() {
        assert_eq!(parse_count("3\n").ok(), Some(3));
        assert!(parse_count("").is_err());
        assert!(parse_count("warning: refname is ambiguous").is_err());
    }

    /// Regression: every git failure read as "no such branch", so a broken repository took the
    /// path of a new branch.
    #[tokio::test]
    async fn resolving_in_a_folder_that_is_no_repository_fails() {
        let scratch = Scratch::new("resolve");
        let repository = scratch.0.join("repository");
        std::fs::create_dir_all(&repository).expect("create the repository folder");
        git(&repository, &["init", "-q"]);
        write(&repository, "a.txt", "a\n");
        git(&repository, &["add", "-A"]);
        git(&repository, &["commit", "-q", "-m", "only"]);
        let repo = LocalRepo::new(&repository);
        let head = git(&repository, &["rev-parse", "HEAD"]);
        assert_eq!(repo.branch_tip("main").await.expect("asked"), Some(head));
        assert_eq!(repo.branch_tip("missing").await.expect("asked"), None);

        let broken = scratch.0.join("broken");
        write(&broken, ".git", "gitdir: nowhere\n");
        let resolved = LocalRepo::new(&broken).branch_tip("main").await;
        assert!(resolved.is_err(), "{resolved:?}");
    }

    #[test]
    fn status_names_the_branch_and_every_change() {
        let state = parse_status("## main...origin/main [behind 2]\nM  src/a.rs\n?? notes.md\n");
        assert_eq!(state.branch.as_deref(), Some("main"));
        assert_eq!(state.changes, ["M  src/a.rs", "?? notes.md"]);
        assert!(!state.is_clean());
        assert_eq!(state.tracked_changes().collect::<Vec<_>>(), ["M  src/a.rs"]);

        let untracked = parse_status("## feature/x\n?? scratch.txt\n");
        assert_eq!(untracked.branch.as_deref(), Some("feature/x"));
        assert!(untracked.is_clean(), "untracked files leave the tree clean");
        assert_eq!(parse_status("## HEAD (no branch)\n").branch, None);
        assert_eq!(parse_status("## No commits yet on main\n").branch.as_deref(), Some("main"));
    }

    #[test]
    fn worktrees_list_their_branches() {
        let output = "worktree D:/dev/app\nHEAD 1111\nbranch refs/heads/main\n\n\
            worktree D:/dev/app-wt/pr-12-x\nHEAD 2222\nbranch refs/heads/feature/x\n\n\
            worktree D:/dev/app-wt/detached\nHEAD 3333\ndetached\n";
        let trees = parse_worktrees(output);
        assert_eq!(trees.len(), 3);
        let native = "D:/dev/app-wt/pr-12-x".replace('/', std::path::MAIN_SEPARATOR_STR);
        assert_eq!(trees[1].path, PathBuf::from(native));
        assert_eq!(trees[1].branch.as_deref(), Some("feature/x"));
        assert_eq!(trees[2].branch, None);
    }

    #[test]
    fn folders_compare_as_git_and_the_system_print_them() {
        assert!(same_folder(Path::new("D:/dev/app/"), Path::new(r"D:\dev\app")));
        assert!(!same_folder(Path::new("D:/dev/app"), Path::new("D:/dev/app2")));
    }
}
