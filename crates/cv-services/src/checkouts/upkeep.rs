//! Keeping a checkout current once it exists: when the page sees a new head of the pull request,
//! the checkout follows it silently where nothing of the user's can be lost
//! (`review_git::checkout::update`); otherwise it stays and says how far behind it is and why.
//! No toast, no popover, no focus change: each update only adds a line to the checkout's log. One
//! update (or removal) per checkout at a time, and none while its checkout job runs.
//!
//! Removing a worktree also lives here: asked once, and once more when the worktree has changes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use review_core::short_sha;
use review_git::GitMirror;
use review_git::checkout::{
    self, Blocked, Changes, CheckoutError, Event, LocalRepo, OwnBranch, Removal, UpdateOutcome,
    UpdateRequest, Updated,
};
use review_store::checkouts::CheckoutRecord;

use cv_runtime::background::{Background, TASK_LOST, Task, TaskPoll};
use cv_runtime::route::PullRequestKey;
use cv_widgets::numbers::counted;
use cv_widgets::progress_log::{ProgressLog, redact, timestamp};

/// How a checkout stands against the pull request's head the page last saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// Commits of the pull request the checkout lacks.
    pub behind: usize,
    /// Why the last update changed nothing.
    pub blocked: Option<Blocked>,
    pub is_updating: bool,
}

/// Where a removal stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalState {
    Removing,
    /// The worktree has changes; removing it needs a second yes.
    Changed(Vec<String>),
    Failed(String),
}

struct UpdateResult {
    outcome: Result<UpdateOutcome, CheckoutError>,
    head: String,
    behind: usize,
    log: Vec<String>,
}

#[derive(Default)]
pub(super) struct Upkeep {
    statuses: HashMap<PullRequestKey, Status>,
    /// The head an update last ran for, so one head is tried once.
    tried: HashMap<PullRequestKey, String>,
    updates: HashMap<PullRequestKey, Task<UpdateResult>>,
    removals: HashMap<PullRequestKey, Task<Result<Removal, CheckoutError>>>,
    removal_states: HashMap<PullRequestKey, RemovalState>,
}

/// What a finished update or removal asks of the records.
pub(super) enum Change {
    /// The checkout reached `head`; the lines go to its log.
    Updated {
        key: PullRequestKey,
        head: String,
        log: Vec<String>,
    },
    /// The checkout stayed; the lines go to its log.
    Stayed {
        key: PullRequestKey,
        log: Vec<String>,
    },
    Removed(PullRequestKey),
}

impl Upkeep {
    pub(super) fn status(&self, key: &PullRequestKey) -> Option<&Status> {
        self.statuses.get(key)
    }

    pub(super) fn removal(&self, key: &PullRequestKey) -> Option<&RemovalState> {
        self.removal_states.get(key)
    }

    pub(super) fn is_busy(&self, key: &PullRequestKey) -> bool {
        self.updates.contains_key(key) || self.removals.contains_key(key)
    }

    pub(super) fn wants_update(&self, key: &PullRequestKey, head: &str) -> bool {
        self.tried.get(key).is_none_or(|tried| tried != head)
    }

    /// Fetches `head` from the mirror and moves the checkout's branch to it where that is safe.
    pub(super) fn update(
        &mut self,
        record: &CheckoutRecord,
        key: &PullRequestKey,
        mirror: Arc<GitMirror>,
        head: &str,
        background: &Background,
    ) {
        self.tried.insert(key.clone(), head.to_owned());
        self.statuses.entry(key.clone()).or_default().is_updating = true;
        let request = UpdateRequest {
            clone: record.clone.clone(),
            branch: record.branch.clone(),
            head: head.to_owned(),
            recorded: Some(record.head.clone()),
            pull_ref: record.pull_ref.clone(),
        };
        let ctx = background.ctx();
        let task = background.spawn(async move {
            let log = Mutex::new(ProgressLog::default());
            let events = |event: Event| {
                if let Event::Git(event) = event {
                    log.lock().unwrap_or_else(PoisonError::into_inner).report_git(event);
                }
            };
            let outcome = checkout::update(&mirror, &request, &events).await;
            let behind = match &outcome {
                Ok(UpdateOutcome::Updated(_)) => 0,
                Ok(UpdateOutcome::Blocked(_)) | Err(_) => behind(&request).await,
            };
            ctx.request_repaint();
            let log = log.into_inner().unwrap_or_else(PoisonError::into_inner);
            UpdateResult {
                outcome,
                head: request.head,
                behind,
                log: log.text().lines().map(str::to_owned).collect(),
            }
        });
        self.updates.insert(key.clone(), task);
    }

    /// Removes the checkout's worktree; `changes` is [`Changes::Discard`] after the second yes for
    /// one with changes.
    pub(super) fn remove(
        &mut self,
        record: &CheckoutRecord,
        key: &PullRequestKey,
        changes: Changes,
        background: &Background,
    ) {
        let (clone, path) = (record.clone.clone(), record.path.clone());
        let own = record.is_own_branch.then(|| (record.branch.clone(), record.head.clone()));
        self.removal_states.insert(key.clone(), RemovalState::Removing);
        let task = background.spawn(async move {
            let own_branch =
                own.as_ref().map(|(name, tip)| OwnBranch { name: name.as_str(), tip: tip.as_str() });
            checkout::remove_worktree(&clone, &path, changes, own_branch).await
        });
        self.removals.insert(key.clone(), task);
    }

    /// Forgets everything about a checkout whose record goes.
    pub(super) fn forget(&mut self, key: &PullRequestKey) {
        self.statuses.remove(key);
        self.tried.remove(key);
        self.removal_states.remove(key);
    }

    /// Forgets a removal's second question or failure (the user kept the worktree).
    pub(super) fn keep(&mut self, key: &PullRequestKey) {
        self.removal_states.remove(key);
    }

    pub(super) fn poll(&mut self) -> Vec<Change> {
        let mut changes = Vec::new();
        let mut finished = Vec::new();
        for (key, task) in &mut self.updates {
            match task.poll() {
                TaskPoll::Pending => {}
                TaskPoll::Done(result) => finished.push((key.clone(), Some(result))),
                TaskPoll::Lost => finished.push((key.clone(), None)),
            }
        }
        for (key, result) in finished {
            self.updates.remove(&key);
            changes.push(self.updated(key, result));
        }
        let mut removed = Vec::new();
        for (key, task) in &mut self.removals {
            match task.poll() {
                TaskPoll::Pending => {}
                TaskPoll::Done(result) => removed.push((key.clone(), result)),
                TaskPoll::Lost => {
                    removed.push((key.clone(), Err(CheckoutError::Git(TASK_LOST.into()))));
                }
            }
        }
        for (key, result) in removed {
            self.removals.remove(&key);
            match result {
                Ok(Removal::Removed) => {
                    self.removal_states.remove(&key);
                    self.statuses.remove(&key);
                    self.tried.remove(&key);
                    changes.push(Change::Removed(key));
                }
                Ok(Removal::Changed(files)) => {
                    self.removal_states.insert(key, RemovalState::Changed(files));
                }
                Err(error) => {
                    self.removal_states.insert(key, RemovalState::Failed(error.to_string()));
                }
            }
        }
        changes
    }

    fn updated(&mut self, key: PullRequestKey, result: Option<UpdateResult>) -> Change {
        let Some(result) = result else {
            let blocked = Blocked::Refused(TASK_LOST.into());
            self.statuses
                .insert(key.clone(), Status { blocked: Some(blocked), ..Status::default() });
            return Change::Stayed { key, log: Vec::new() };
        };
        let now = timestamp(time::OffsetDateTime::now_utc());
        let mut log = result.log;
        let outcome = result
            .outcome
            .unwrap_or_else(|error| UpdateOutcome::Blocked(Blocked::Refused(error.to_string())));
        match outcome {
            UpdateOutcome::Updated(updated) => {
                self.statuses.insert(key.clone(), Status::default());
                let how = match updated {
                    Updated::UpToDate => "already there".to_owned(),
                    Updated::FastForwarded { from } => {
                        format!("fast-forward from {}", short_sha(&from))
                    }
                    Updated::Moved { from } => {
                        format!("followed a force-push from {}", short_sha(&from))
                    }
                };
                log.push(format!("{now}  Updated to {} ({how})", short_sha(&result.head)));
                Change::Updated { key, head: result.head, log }
            }
            UpdateOutcome::Blocked(blocked) => {
                log.push(redact(&format!(
                    "{now}  Not updated: {}",
                    reason(&blocked, result.behind)
                )));
                let status =
                    Status { behind: result.behind, blocked: Some(blocked), is_updating: false };
                self.statuses.insert(key.clone(), status);
                Change::Stayed { key, log }
            }
        }
    }
}

/// How many of the head's commits the branch lacks; 0 when that cannot be told.
async fn behind(request: &UpdateRequest) -> usize {
    let repo = LocalRepo::new(&request.clone);
    let Ok(Some(tip)) = repo.branch_tip(&request.branch).await else {
        return 0;
    };
    repo.behind(&tip, &request.head).await.unwrap_or(0)
}

/// Why a checkout was not updated, for its log and the menu's Update item.
pub fn reason(blocked: &Blocked, behind: usize) -> String {
    match blocked {
        Blocked::Changed(files) => {
            format!("{} in the checkout", counted(files.len(), "changed file", "changed files"))
        }
        Blocked::LocalCommits(commits) => {
            let commits = counted(commits.len(), "local commit", "local commits");
            format!("{commits} not in the pull request")
        }
        Blocked::Refused(message) => {
            let first = message.lines().next().unwrap_or(message);
            format!("git refused: {first}")
        }
        Blocked::NoBranch => format!("the branch is gone ({behind} behind)"),
    }
}

pub fn plural(count: usize, word: &str) -> String {
    if count == 1 { word.to_owned() } else { format!("{word}s") }
}
