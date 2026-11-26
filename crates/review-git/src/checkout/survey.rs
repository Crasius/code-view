//! What the Check out popover needs to know about a clone before anything runs: its branch and
//! changed files, which local branch the pull request gets, whether that branch is checked out
//! somewhere already, and whether the pull request comes from a fork. Nothing here writes.

use std::path::PathBuf;

use super::naming::fork_branch;
use super::remotes::FoundClone;
use super::repo::{LocalRepo, TreeState};
use super::{CheckoutError, Upstream};

/// A clone of the pull request's repository, seen from the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    pub clone: PathBuf,
    /// The clone's remote that points to the repository.
    pub remote: String,
    /// The clone's own branch and changed files.
    pub state: TreeState,
    /// The local branch the pull request gets: the source branch, or `pr-<n>-<slug>` for a fork.
    pub branch: String,
    /// The source branch is not one of the remote's branches the clone knows: the pull request
    /// comes from a fork, so its branch gets no upstream.
    pub is_fork: bool,
    /// The working tree the branch is checked out in already.
    pub checked_out: Option<PathBuf>,
}

impl Survey {
    /// The upstream a new branch follows: the source branch on the clone's remote, unless a fork.
    pub fn upstream(&self, source_branch: &str) -> Option<Upstream> {
        (!self.is_fork).then(|| Upstream {
            remote: self.remote.clone(),
            branch: source_name(source_branch).to_owned(),
        })
    }
}

/// Looks at `clone` for the pull request `number` from `source_branch`.
pub async fn survey(
    clone: &FoundClone,
    source_branch: &str,
    number: u64,
) -> Result<Survey, CheckoutError> {
    let repo = LocalRepo::new(&clone.dir);
    let state = repo.state().await.map_err(|_| CheckoutError::NotARepository(clone.dir.clone()))?;
    let source = source_name(source_branch);
    let is_fork = !repo.has_remote_branch(&clone.remote, source).await?;
    let branch = if is_fork { fork_branch(number, source) } else { source.to_owned() };
    let checked_out = repo.checked_out_at(&branch).await?;
    Ok(Survey {
        clone: clone.dir.clone(),
        remote: clone.remote.clone(),
        state,
        branch,
        is_fork,
        checked_out,
    })
}

/// `refs/heads/feature/x` (how Azure DevOps names it) → `feature/x`.
pub fn source_name(source_branch: &str) -> &str {
    source_branch.strip_prefix("refs/heads/").unwrap_or(source_branch)
}
