//! Removing a worktree a checkout made: on the user's word (`git worktree remove`, which refuses a
//! tree with changes unless the user confirmed that too), or by itself once its pull request is
//! done elsewhere, and then only when nothing of the user's would go with it.

use std::ffi::OsString;
use std::path::Path;

use super::CheckoutError;
use super::repo::LocalRepo;

/// What a removal did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    Removed,
    /// The worktree has changes (porcelain lines, untracked files too); nothing was removed.
    Changed(Vec<String>),
}

/// What a removal does with changes in the worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Changes {
    /// A worktree with changes stays, and the removal reports them.
    Keep,
    /// The user confirmed a second time: the changes go with the worktree.
    Discard,
}

/// The branch the checkout made for itself.
#[derive(Debug, Clone, Copy)]
pub struct OwnBranch<'a> {
    pub name: &'a str,
    /// The commit the checkout made the branch at; the branch goes only while it still points here.
    pub tip: &'a str,
}

/// Removes the worktree at `path` of the clone `clone`; `changes` says what happens to changes in
/// it. The branch stays unless it was the checkout's own and still points at its tip: then nothing
/// of the user's is on it, and it goes too.
pub async fn remove_worktree(
    clone: &Path,
    path: &Path,
    changes: Changes,
    own_branch: Option<OwnBranch<'_>>,
) -> Result<Removal, CheckoutError> {
    if path.exists() {
        let state = LocalRepo::new(path).state().await?;
        if !state.changes.is_empty() && changes == Changes::Keep {
            return Ok(Removal::Changed(state.changes));
        }
    }
    let repo = LocalRepo::new(clone);
    let mut args: Vec<OsString> = vec!["worktree".into(), "remove".into()];
    if changes == Changes::Discard {
        args.push("--force".into());
    }
    args.push(path.as_os_str().to_owned());
    if path.exists() {
        repo.git().run(args).await?;
    } else {
        repo.git().run(["worktree", "prune"]).await?;
    }
    if let Some(own) = own_branch
        && repo.branch_tip(own.name).await?.as_deref() == Some(own.tip)
    {
        let branch_ref = format!("refs/heads/{}", own.name);
        repo.git().run(["update-ref", "-d", branch_ref.as_str(), own.tip]).await?;
    }
    Ok(Removal::Removed)
}

/// A finished pull request's worktree, to remove by itself.
#[derive(Debug, Clone, Copy)]
pub struct FinishedTree<'a> {
    pub clone: &'a Path,
    pub path: &'a Path,
    /// The local branch, and whether the checkout created it.
    pub branch: &'a str,
    pub is_own_branch: bool,
    /// The pull request's last head the checkout knew; empty when it never knew one.
    pub head: &'a str,
}

/// What an automatic removal did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finished {
    /// The worktree is gone; `branch_kept` names why git kept the checkout's own branch.
    Removed { branch_kept: Option<String> },
    /// Nothing was removed, for this reason.
    Kept(Keep),
}

/// Why a finished pull request's worktree stays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keep {
    /// Tracked or untracked files differ (porcelain lines).
    Changed(Vec<String>),
    /// Commits on the branch that the pull request's head does not have.
    LocalCommits(Vec<String>),
    /// The checkout never knew the pull request's head, so it cannot tell.
    UnknownHead,
}

/// Removes the worktree of a pull request merged or closed elsewhere, but only when its files are
/// all committed (untracked ones too) and its commits are all in the pull request's head: then
/// nothing is lost. `git worktree remove` without `--force`, then the checkout's own branch with
/// `git branch -d`, which keeps a branch git does not see merged.
pub async fn remove_finished(tree: FinishedTree<'_>) -> Result<Finished, CheckoutError> {
    if !tree.path.exists() {
        LocalRepo::new(tree.clone).git().run(["worktree", "prune"]).await?;
        return Ok(Finished::Removed { branch_kept: delete_branch(&tree).await? });
    }
    let worktree = LocalRepo::new(tree.path);
    let state = worktree.state().await?;
    if !state.changes.is_empty() {
        return Ok(Finished::Kept(Keep::Changed(state.changes)));
    }
    if tree.head.is_empty() || worktree.resolve(tree.head).await?.is_none() {
        return Ok(Finished::Kept(Keep::UnknownHead));
    }
    let local = worktree.commits_missing_from("HEAD", tree.head).await?;
    if !local.is_empty() {
        return Ok(Finished::Kept(Keep::LocalCommits(local)));
    }
    let repo = LocalRepo::new(tree.clone);
    let path = tree.path.as_os_str().to_owned();
    repo.git().run([OsString::from("worktree"), "remove".into(), path]).await?;
    Ok(Finished::Removed { branch_kept: delete_branch(&tree).await? })
}

/// `git branch -d` of the checkout's own branch; git's reason when it keeps it.
async fn delete_branch(tree: &FinishedTree<'_>) -> Result<Option<String>, CheckoutError> {
    let repo = LocalRepo::new(tree.clone);
    if !tree.is_own_branch || repo.branch_tip(tree.branch).await?.is_none() {
        return Ok(None);
    }
    match repo.git().run(["branch", "-d", tree.branch]).await {
        Ok(_) => Ok(None),
        Err(error) => Ok(Some(error.to_string())),
    }
}
