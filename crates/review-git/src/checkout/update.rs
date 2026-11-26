//! Brings a checkout up to the pull request's new head, but only where nothing of the user's can
//! get lost:
//!
//! - the branch only lags behind (its tip is an ancestor of the new head): fast-forward;
//! - the branch is still where the pull request last was and the new head is not a descendant
//!   (a force-push): move it to the new head, since the commits it leaves were the pull
//!   request's, not the user's;
//! - where the branch is checked out, only with no tracked file changed: a fast-forward merge or
//!   `reset --keep`, which both refuse to overwrite anything (an untracked file in the way, too);
//!   a branch checked out nowhere just moves.
//!
//! Anything else (commits of the user's on the branch, changed files, git refusing) changes
//! nothing and says why ([`Blocked`]).

use std::path::PathBuf;

use super::repo::LocalRepo;
use super::supply;
use super::{CheckoutError, Event};
use crate::mirror::GitMirror;
use crate::progress::GitEvent;

/// Which checkout to update to which head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRequest {
    /// The user's clone (or the new clone a checkout made).
    pub clone: PathBuf,
    pub branch: String,
    /// The pull request's new head.
    pub head: String,
    /// The head the checkout last had from the pull request; `None` when unknown.
    pub recorded: Option<String>,
    /// The clone's ref for the pull request's head (`Request::pull_ref`).
    pub pull_ref: String,
}

/// An update that went through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Updated {
    /// The branch was at the head already.
    UpToDate,
    FastForwarded {
        from: String,
    },
    /// After a force-push: the branch moved from the pull request's old head.
    Moved {
        from: String,
    },
}

/// Why an update changed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// Tracked files are changed where the branch is checked out (porcelain lines).
    Changed(Vec<String>),
    /// The branch has commits the pull request lacks (`<short id>  <subject>`, newest first).
    LocalCommits(Vec<String>),
    /// git refused (an untracked file in the way, say); its message.
    Refused(String),
    /// The branch is gone from the clone.
    NoBranch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    Updated(Updated),
    Blocked(Blocked),
}

/// Fetches `request.head` from the mirror and moves the branch to it where that is safe.
pub async fn update(
    mirror: &GitMirror,
    request: &UpdateRequest,
    events: &(dyn Fn(Event) + Send + Sync),
) -> Result<UpdateOutcome, CheckoutError> {
    let sink = |event: GitEvent| events(Event::Git(event));
    let repo = LocalRepo::new(&request.clone);
    supply::supply(mirror, &repo, &request.head, &request.pull_ref, &sink).await?;
    let Some(tip) = repo.branch_tip(&request.branch).await? else {
        return Ok(UpdateOutcome::Blocked(Blocked::NoBranch));
    };
    if tip == request.head {
        return Ok(UpdateOutcome::Updated(Updated::UpToDate));
    }
    let is_fast_forward = repo.is_ancestor(&tip, &request.head).await?;
    let is_pull_requests_own = request.recorded.as_deref() == Some(tip.as_str());
    if !is_fast_forward && !is_pull_requests_own {
        let commits = repo.commits_missing_from(&tip, &request.head).await?;
        return Ok(UpdateOutcome::Blocked(Blocked::LocalCommits(commits)));
    }
    let updated = if is_fast_forward {
        Updated::FastForwarded { from: tip.clone() }
    } else {
        Updated::Moved { from: tip.clone() }
    };
    let Some(path) = repo.checked_out_at(&request.branch).await? else {
        let branch_ref = format!("refs/heads/{}", request.branch);
        repo.git()
            .run(["update-ref", branch_ref.as_str(), request.head.as_str(), tip.as_str()])
            .await?;
        return Ok(UpdateOutcome::Updated(updated));
    };
    let tree = LocalRepo::new(&path);
    let state = tree.state().await?;
    if !state.is_clean() {
        return Ok(UpdateOutcome::Blocked(Blocked::Changed(state.tracked_changes())));
    }
    let moved = if is_fast_forward {
        let merge = ["merge", "--ff-only", "--no-edit", request.head.as_str()];
        tree.git().run_with_progress(merge, &sink).await
    } else {
        tree.git().run_with_progress(["reset", "--keep", request.head.as_str()], &sink).await
    };
    match moved {
        Ok(_) => Ok(UpdateOutcome::Updated(updated)),
        Err(error) => Ok(UpdateOutcome::Blocked(Blocked::Refused(error.to_string()))),
    }
}
