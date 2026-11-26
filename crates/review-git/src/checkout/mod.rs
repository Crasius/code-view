//! Local checkouts of a pull request in the user's own clones: a new worktree beside the clone, the
//! clone itself, or a new clone. Every commit and file comes from the app's mirror of the
//! repository ([`supply`]), never from the host directly, and no credential reaches the user's
//! repositories; the mirror downloads what it lacks through its own authorization.
//!
//! Where the user's work could be touched the code refuses instead of forcing: a clone with
//! changed files is not switched, a branch checked out elsewhere gets no second worktree, an
//! update only fast-forwards or follows a force-push of a branch nobody committed to
//! ([`update`]), and a worktree is removed only when asked ([`remove`]). What a failed checkout
//! made is taken away again ([`undo`]), and only that.

mod naming;
mod remotes;
mod remove;
mod repo;
mod run;
mod supply;
mod survey;
#[cfg(test)]
mod tests;
mod update;

use std::path::PathBuf;

use review_core::ProviderError;

pub use self::naming::{fork_branch, slug, worktree_path};
pub use self::remotes::{
    FoundClone, find_clones, matching_remote, repository_id, same_repository, without_credentials,
};
pub use self::remove::{Finished, FinishedTree, Keep, Removal, remove_finished, remove_worktree};
pub use self::repo::{Divergence, LocalRepo, TreeState, Worktree, same_folder};
pub use self::run::{Done, Made, Request, Target, Upstream, check_out, undo};
pub use self::survey::{Survey, source_name, survey};
pub use self::update::{Blocked, UpdateOutcome, UpdateRequest, Updated, update};
use crate::progress::GitEvent;

/// The steps of a checkout, in order; which of them run depends on the [`Target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    /// The clone is checked (or the new clone's folder made).
    FindClone,
    /// The pull request's commits come from the mirror.
    Fetch,
    CreateWorktree,
    /// The branch's files are written.
    CheckOut,
}

/// What a checkout or an update reports while it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A step starts.
    Step(Step),
    Git(GitEvent),
}

/// Why a checkout stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutError {
    /// git failed; its message.
    Git(String),
    /// The clone's tracked files are changed (porcelain lines); it is not switched.
    Dirty(Vec<String>),
    /// The branch is checked out in this working tree already, which git allows once only.
    CheckedOut(PathBuf),
    /// The folder exists and is not empty.
    FolderTaken(PathBuf),
    /// No git working tree in this folder.
    NotARepository(PathBuf),
}

impl std::fmt::Display for CheckoutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Git(message) => formatter.write_str(message),
            Self::Dirty(files) => write!(formatter, "the clone has {} changed files", files.len()),
            Self::CheckedOut(path) => {
                write!(formatter, "the branch is checked out in {} already", path.display())
            }
            Self::FolderTaken(path) => write!(formatter, "{} already exists", path.display()),
            Self::NotARepository(path) => {
                write!(formatter, "{} is not a git working tree", path.display())
            }
        }
    }
}

impl From<ProviderError> for CheckoutError {
    fn from(error: ProviderError) -> Self {
        Self::Git(match error {
            ProviderError::Local(message) => message,
            other => other.to_string(),
        })
    }
}
