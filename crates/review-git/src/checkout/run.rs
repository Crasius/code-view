//! One checkout of a pull request: into a new worktree of the user's clone, into the clone itself,
//! or into a new clone made from the mirror. Each step reports itself; whatever the checkout makes
//! (a folder, a branch, a worktree) is noted in [`Made`] the moment it exists, so [`undo`] can take
//! exactly that away again after a failure or a cancel, and nothing else.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use super::repo::{LocalRepo, same_folder};
use super::supply;
use super::{CheckoutError, Event, Step};
use crate::command::GitCommand;
use crate::mirror::GitMirror;
use crate::progress::GitEvent;

/// Where the pull request is checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A new worktree of the clone, in this folder.
    Worktree { path: PathBuf },
    /// The clone itself switches to the branch; it needs a clean working tree.
    Clone,
    /// A new clone, made from the mirror in [`Request::clone`]; its `origin` then points to this
    /// URL, which carries no credentials.
    NewClone { origin: String },
}

/// The branch a same-repository pull request's local branch follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub remote: String,
    pub branch: String,
}

/// What to check out where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The user's clone; for [`Target::NewClone`] the folder the new clone goes to.
    pub clone: PathBuf,
    pub target: Target,
    /// The pull request's head commit.
    pub head: String,
    /// The local branch.
    pub branch: String,
    /// Set for a pull request from the clone's own repository; a fork's branch has none.
    pub upstream: Option<Upstream>,
    /// Where the clone keeps the pull request's head (`refs/codeview/pull/12/head`), so its
    /// distance to the branch can be counted without the mirror.
    pub pull_ref: String,
}

/// What a checkout made, for [`undo`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Made {
    /// A folder that did not exist before.
    pub folder: Option<PathBuf>,
    pub worktree: Option<MadeWorktree>,
    pub branch: Option<MadeBranch>,
}

/// A worktree a checkout registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MadeWorktree {
    /// The clone it is registered in.
    pub clone: PathBuf,
    /// The worktree's folder.
    pub path: PathBuf,
}

/// A branch a checkout created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MadeBranch {
    /// The repository it is in.
    pub repo: PathBuf,
    /// The branch's short name.
    pub name: String,
    /// The commit it was created at.
    pub tip: String,
}

/// A finished checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    /// The working tree the branch is checked out in.
    pub path: PathBuf,
    pub branch: String,
    /// Where the branch points now: the head, or the branch's own commits when it had some.
    pub tip: String,
    /// The branch was made by this checkout (and may go with its worktree).
    pub is_new_branch: bool,
}

type Events<'a> = &'a (dyn Fn(Event) + Send + Sync);

/// Checks the pull request out as `request` says, noting in `made` what exists because of it.
pub async fn check_out(
    mirror: &GitMirror,
    request: &Request,
    events: Events<'_>,
    made: &Mutex<Made>,
) -> Result<Done, CheckoutError> {
    let sink = |event: GitEvent| events(Event::Git(event));
    events(Event::Step(Step::FindClone));
    if let Target::NewClone { origin } = &request.target {
        return new_clone(mirror, request, origin, events, made).await;
    }
    let repo = LocalRepo::new(&request.clone);
    let state =
        repo.state().await.map_err(|_| CheckoutError::NotARepository(request.clone.clone()))?;
    let mut is_current = false;
    if let Some(path) = repo.checked_out_at(&request.branch).await? {
        is_current = request.target == Target::Clone && same_folder(&path, &request.clone);
        if !is_current {
            return Err(CheckoutError::CheckedOut(path));
        }
    }
    if request.target == Target::Clone && !state.is_clean() {
        return Err(CheckoutError::Dirty(state.tracked_changes()));
    }
    if let Target::Worktree { path } = &request.target
        && is_taken(path)
    {
        return Err(CheckoutError::FolderTaken(path.clone()));
    }
    events(Event::Step(Step::Fetch));
    supply::supply(mirror, &repo, &request.head, &request.pull_ref, &sink).await?;
    let (tip, is_new_branch) = branch_at_head(&repo, request, is_current, events, made).await?;
    let path = match &request.target {
        Target::Worktree { path } => {
            events(Event::Step(Step::CreateWorktree));
            add_worktree(&repo, path, &request.branch, made).await?;
            events(Event::Step(Step::CheckOut));
            let tree = LocalRepo::new(path);
            tree.git().run_with_progress(["read-tree", "-m", "-u", "HEAD"], &sink).await?;
            path.clone()
        }
        Target::Clone | Target::NewClone { .. } => {
            events(Event::Step(Step::CheckOut));
            if !is_current {
                let switch = ["switch", "--no-guess", request.branch.as_str()];
                repo.git().run_with_progress(switch, &sink).await?;
            }
            request.clone.clone()
        }
    };
    if is_new_branch && let Some(upstream) = &request.upstream {
        set_upstream(&repo, &request.branch, upstream).await?;
    }
    Ok(Done { path, branch: request.branch.clone(), tip, is_new_branch })
}

/// Makes the local branch point at the head: a new branch there; an existing one that only lags
/// behind moves forward (by a fast-forward merge where it is checked out, `is_current`); one with
/// commits of its own stays as it is. Returns its tip and whether it is new.
async fn branch_at_head(
    repo: &LocalRepo,
    request: &Request,
    is_current: bool,
    events: Events<'_>,
    made: &Mutex<Made>,
) -> Result<(String, bool), CheckoutError> {
    let branch_ref = format!("refs/heads/{}", request.branch);
    let Some(tip) = repo.branch_tip(&request.branch).await? else {
        let zero = "0".repeat(request.head.len());
        repo.git().run(["update-ref", branch_ref.as_str(), request.head.as_str(), &zero]).await?;
        note(made, |made| {
            made.branch = Some(MadeBranch {
                repo: request.clone.clone(),
                name: request.branch.clone(),
                tip: request.head.clone(),
            });
        });
        return Ok((request.head.clone(), true));
    };
    if tip == request.head {
        return Ok((tip, false));
    }
    if repo.is_ancestor(&tip, &request.head).await? {
        if is_current {
            let sink = |event: GitEvent| events(Event::Git(event));
            let merge = ["merge", "--ff-only", "--no-edit", request.head.as_str()];
            repo.git().run_with_progress(merge, &sink).await?;
        } else {
            repo.git()
                .run(["update-ref", branch_ref.as_str(), request.head.as_str(), tip.as_str()])
                .await?;
        }
        events(Event::Git(GitEvent::Message(format!(
            "{} moved forward to the pull request's head",
            request.branch
        ))));
        return Ok((request.head.clone(), false));
    }
    events(Event::Git(GitEvent::Message(format!(
        "{} has commits the pull request lacks; it stays as it is",
        request.branch
    ))));
    Ok((tip, false))
}

/// `git worktree add --no-checkout <path> <branch>`: the files come in the next step.
async fn add_worktree(
    repo: &LocalRepo,
    path: &Path,
    branch: &str,
    made: &Mutex<Made>,
) -> Result<(), CheckoutError> {
    if !path.exists() {
        note(made, |made| made.folder = Some(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            CheckoutError::Git(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    let args: [OsString; 5] = [
        "worktree".into(),
        "add".into(),
        "--no-checkout".into(),
        path.as_os_str().to_owned(),
        branch.into(),
    ];
    // Registered before git returns: a failure after this point leaves a registration to remove.
    note(made, |made| {
        made.worktree =
            Some(MadeWorktree { clone: repo.dir().to_path_buf(), path: path.to_path_buf() });
    });
    repo.git().run(args).await?;
    Ok(())
}

async fn set_upstream(
    repo: &LocalRepo,
    branch: &str,
    upstream: &Upstream,
) -> Result<(), CheckoutError> {
    let remote_key = format!("branch.{branch}.remote");
    let merge_key = format!("branch.{branch}.merge");
    let merge = format!("refs/heads/{}", upstream.branch);
    repo.git().run(["config", remote_key.as_str(), upstream.remote.as_str()]).await?;
    repo.git().run(["config", merge_key.as_str(), merge.as_str()]).await?;
    Ok(())
}

/// A new clone in `request.clone`: a partial clone of the mirror (commits and trees), the branch
/// checked out (its blobs come from the mirror), then `origin` pointed at the real repository.
async fn new_clone(
    mirror: &GitMirror,
    request: &Request,
    origin: &str,
    events: Events<'_>,
    made: &Mutex<Made>,
) -> Result<Done, CheckoutError> {
    let sink = |event: GitEvent| events(Event::Git(event));
    let dir = &request.clone;
    if is_taken(dir) {
        return Err(CheckoutError::FolderTaken(dir.clone()));
    }
    if !dir.exists() {
        note(made, |made| made.folder = Some(dir.clone()));
    }
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|error| CheckoutError::Git(format!("cannot create {}: {error}", dir.display())))?;
    let init: [OsString; 3] = ["init".into(), "--quiet".into(), dir.as_os_str().to_owned()];
    GitCommand::in_tree(dir).run(init).await?;
    let repo = LocalRepo::new(dir);
    let mirror_dir = mirror.git_dir().to_string_lossy().into_owned();
    let settings = [
        ("core.repositoryformatversion", "1"),
        ("extensions.partialClone", "origin"),
        ("remote.origin.url", mirror_dir.as_str()),
        ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
        ("remote.origin.promisor", "true"),
        ("remote.origin.partialclonefilter", "blob:none"),
    ];
    for (name, value) in settings {
        repo.git().run(["config", name, value]).await?;
    }
    events(Event::Step(Step::Fetch));
    supply::supply_partial(mirror, &repo, &request.head, &request.pull_ref, &sink).await?;
    let (tip, is_new_branch) = branch_at_head(&repo, request, false, events, made).await?;
    events(Event::Step(Step::CheckOut));
    repo.git().run_with_progress(["switch", "--no-guess", request.branch.as_str()], &sink).await?;
    let origin = super::remotes::without_credentials(origin);
    repo.git().run(["config", "remote.origin.url", origin.as_str()]).await?;
    if let Some(upstream) = &request.upstream {
        set_upstream(&repo, &request.branch, upstream).await?;
    }
    Ok(Done { path: dir.clone(), branch: request.branch.clone(), tip, is_new_branch })
}

/// True when `path` exists and is not an empty folder.
fn is_taken(path: &Path) -> bool {
    match std::fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_some(),
        Err(_) => path.exists(),
    }
}

fn note(made: &Mutex<Made>, change: impl FnOnce(&mut Made)) {
    change(&mut made.lock().unwrap_or_else(PoisonError::into_inner));
}

/// Takes away what a failed or cancelled checkout made, newest first: the worktree's
/// registration, the folder it created, the branch it created (only while it still points where
/// the checkout put it). Returns a line per thing removed, and per thing that could not be.
pub async fn undo(made: Made) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(MadeWorktree { clone, path }) = &made.worktree {
        let args: [OsString; 4] =
            ["worktree".into(), "remove".into(), "--force".into(), path.as_os_str().to_owned()];
        match GitCommand::in_tree(clone).run(args).await {
            Ok(_) => lines.push(format!("Removed the worktree {}", path.display())),
            Err(_) => {
                // Not registered after all (git failed before): pruning drops a stale entry.
                let _nothing_to_prune = GitCommand::in_tree(clone).run(["worktree", "prune"]).await;
            }
        }
    }
    if let Some(folder) = &made.folder
        && folder.exists()
    {
        match tokio::fs::remove_dir_all(folder).await {
            Ok(()) => lines.push(format!("Removed the folder {}", folder.display())),
            Err(error) => lines.push(format!("Could not remove {}: {error}", folder.display())),
        }
    }
    if let Some(MadeBranch { repo, name, tip }) = &made.branch
        && made.folder.as_ref().is_none_or(|folder| !same_folder(folder, repo))
    {
        let branch_ref = format!("refs/heads/{name}");
        let deleted =
            GitCommand::in_tree(repo).run(["update-ref", "-d", branch_ref.as_str(), tip]).await;
        match deleted {
            Ok(_) => lines.push(format!("Removed the branch {name}")),
            Err(error) => lines.push(format!("Could not remove the branch {name}: {error}")),
        }
    }
    lines
}
