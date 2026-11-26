//! Checkouts of pull requests that were merged or closed elsewhere: on the host, by someone or
//! something other than a merge from Code View. The page sees it on a refresh or the live path;
//! at app start every recorded pull request is asked once. Settings › Local checkouts decides
//! what happens ([`WhenFinished`]):
//!
//! - Remove automatically: a worktree the app made goes when nothing of the user's would go with
//!   it (`review_git::checkout::remove_finished`); otherwise the page asks, with the reason.
//! - Ask me first: the page offers the removal.
//! - Keep worktree: nothing happens.
//!
//! A pull request merged from Code View always asks. A checkout in a clone (the user's, a new one
//! or an adopted working tree) is never removed: only its record goes. Silent like an update: no
//! toast; a worktree that stays gets the reason in its log.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use review_core::{Provider, PullRequestState};
use review_git::checkout::{self, CheckoutError, Finished, FinishedTree, Keep};
use review_store::checkouts::{CheckoutKind, CheckoutRecord, WhenFinished};

use super::upkeep::plural;
use cv_runtime::background::{Background, Task, TaskPoll};
use cv_runtime::route::PullRequestKey;
use cv_widgets::progress_log::timestamp;

/// What a finished pull request's checkout gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Decision {
    Remove,
    /// The page offers the removal; with why it did not go by itself.
    Ask(Option<String>),
    Keep,
    /// The record goes, the folder stays: a clone is never removed.
    Forget,
}

/// The decision for `record` under `mode`.
pub(super) fn decide(mode: WhenFinished, record: &CheckoutRecord) -> Decision {
    if record.kind != CheckoutKind::Worktree {
        return if mode == WhenFinished::Keep { Decision::Keep } else { Decision::Forget };
    }
    if record.merged_here {
        return if mode == WhenFinished::Keep { Decision::Keep } else { Decision::Ask(None) };
    }
    match mode {
        WhenFinished::RemoveAutomatically => Decision::Remove,
        WhenFinished::Ask => Decision::Ask(None),
        WhenFinished::Keep => Decision::Keep,
    }
}

/// The page of a finished pull request offers to remove its worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Offer {
    /// Why the worktree did not go by itself, when it was meant to.
    pub kept: Option<String>,
}

/// What a finished removal asks of the records.
pub(super) enum Change {
    Removed(PullRequestKey),
    /// The worktree stayed; the line goes to its log.
    Kept {
        key: PullRequestKey,
        line: String,
    },
}

#[derive(Default)]
pub(super) struct Finish {
    /// Pull requests handled this run, so each is handled once.
    handled: HashSet<PullRequestKey>,
    has_asked_states: bool,
    /// At start: the state of each recorded pull request; `None` when it could not be read.
    states: HashMap<PullRequestKey, Task<Option<PullRequestState>>>,
    removals: HashMap<PullRequestKey, Task<Result<Finished, CheckoutError>>>,
    /// Pull requests whose page offers the removal, and why it did not go by itself.
    offers: HashMap<PullRequestKey, Offer>,
}

impl Finish {
    pub(super) fn is_busy(&self, key: &PullRequestKey) -> bool {
        self.removals.contains_key(key)
    }

    pub(super) fn is_handled(&self, key: &PullRequestKey) -> bool {
        self.handled.contains(key)
    }

    pub(super) fn handled(&mut self, key: &PullRequestKey) {
        self.handled.insert(key.clone());
    }

    pub(super) fn offer(&self, key: &PullRequestKey) -> Option<&Offer> {
        self.offers.get(key)
    }

    pub(super) fn ask(&mut self, key: &PullRequestKey, kept: Option<String>) {
        self.offers.insert(key.clone(), Offer { kept });
    }

    pub(super) fn has_asked_states(&self) -> bool {
        self.has_asked_states
    }

    /// Asks for the state of each pull request in `records`, once per app run, through the
    /// provider `provider_of` gives for its connection.
    pub(super) fn ask_states(
        &mut self,
        records: &[CheckoutRecord],
        provider_of: impl Fn(&str) -> Option<Arc<dyn Provider>>,
        background: &Background,
    ) {
        self.has_asked_states = true;
        for record in records {
            let Some(provider) = provider_of(&record.connection_id) else {
                continue;
            };
            let key = PullRequestKey {
                connection_id: record.connection_id.clone(),
                repository_key: record.repository_key.clone(),
                number: record.number,
            };
            let (repository, number) = (record.repository_key.clone(), record.number);
            let task = background.spawn(async move {
                let detail = provider.pull_request(&repository, number).await;
                detail.ok().map(|detail| detail.summary.state)
            });
            self.states.insert(key, task);
        }
    }

    /// The pull requests whose state came in finished.
    pub(super) fn finished_states(&mut self) -> Vec<PullRequestKey> {
        let mut done = Vec::new();
        for (key, task) in &mut self.states {
            match task.poll() {
                TaskPoll::Pending => {}
                TaskPoll::Done(state) => done.push((key.clone(), state)),
                TaskPoll::Lost => done.push((key.clone(), None)),
            }
        }
        let mut finished = Vec::new();
        for (key, state) in done {
            self.states.remove(&key);
            if state.is_some_and(|state| state != PullRequestState::Open) {
                finished.push(key);
            }
        }
        finished
    }

    /// Removes the finished pull request's worktree where nothing of the user's is in it.
    pub(super) fn remove(
        &mut self,
        record: &CheckoutRecord,
        key: &PullRequestKey,
        background: &Background,
    ) {
        let record = record.clone();
        let task = background.spawn(async move {
            checkout::remove_finished(FinishedTree {
                clone: &record.clone,
                path: &record.path,
                branch: &record.branch,
                is_own_branch: record.is_own_branch,
                head: &record.head,
            })
            .await
        });
        self.removals.insert(key.clone(), task);
    }

    pub(super) fn poll(&mut self) -> Vec<Change> {
        let mut done = Vec::new();
        for (key, task) in &mut self.removals {
            match task.poll() {
                TaskPoll::Pending => {}
                TaskPoll::Done(result) => done.push((key.clone(), result)),
                TaskPoll::Lost => done.push((
                    key.clone(),
                    Err(CheckoutError::Git("The background task stopped unexpectedly.".into())),
                )),
            }
        }
        let mut changes = Vec::new();
        for (key, result) in done {
            self.removals.remove(&key);
            let reason = match result {
                Ok(Finished::Removed { .. }) => {
                    changes.push(Change::Removed(key));
                    continue;
                }
                Ok(Finished::Kept(keep)) => kept_reason(&keep),
                Err(error) => format!("git refused: {error}"),
            };
            let now = timestamp(time::OffsetDateTime::now_utc());
            let line = format!("{now}  Pull request done; worktree kept: {reason}");
            self.offers.insert(key.clone(), Offer { kept: Some(reason) });
            changes.push(Change::Kept { key, line });
        }
        changes
    }
}

/// Why a finished pull request's worktree stayed, for its log and the offer.
fn kept_reason(keep: &Keep) -> String {
    match keep {
        Keep::Changed(files) => {
            let count = files.len();
            format!("{count} uncommitted {} in the worktree", plural(count, "file"))
        }
        Keep::LocalCommits(commits) => {
            let count = commits.len();
            format!("{count} local {} not in the pull request", plural(count, "commit"))
        }
        Keep::UnknownHead => "Code View does not know the pull request's last head".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use review_store::checkouts::{CheckoutKind, CheckoutRecord, OpenWith, WhenFinished};

    use super::{Decision, decide};

    fn record(kind: CheckoutKind, merged_here: bool) -> CheckoutRecord {
        CheckoutRecord {
            connection_id: "gh".into(),
            repository_key: "acme/app".into(),
            number: 7,
            clone: PathBuf::from("/dev/app"),
            path: PathBuf::from("/dev/app-wt/pr-7-feature"),
            kind,
            branch: "feature".into(),
            is_own_branch: true,
            pull_ref: "refs/codeview/pull/7/head".into(),
            head: "abc".into(),
            last_opened: OpenWith::Nothing,
            took_ms: 0,
            merged_here,
            log: Vec::new(),
        }
    }

    #[test]
    fn the_setting_decides_for_a_worktree_merged_elsewhere() {
        let worktree = record(CheckoutKind::Worktree, false);
        assert_eq!(decide(WhenFinished::RemoveAutomatically, &worktree), Decision::Remove);
        assert_eq!(decide(WhenFinished::Ask, &worktree), Decision::Ask(None));
        assert_eq!(decide(WhenFinished::Keep, &worktree), Decision::Keep);
    }

    #[test]
    fn a_merge_from_code_view_asks_and_a_clone_only_loses_its_record() {
        let merged_here = record(CheckoutKind::Worktree, true);
        assert_eq!(decide(WhenFinished::RemoveAutomatically, &merged_here), Decision::Ask(None));
        assert_eq!(decide(WhenFinished::Keep, &merged_here), Decision::Keep);
        for kind in [CheckoutKind::Clone, CheckoutKind::NewClone, CheckoutKind::Adopted] {
            let clone = record(kind, false);
            assert_eq!(decide(WhenFinished::RemoveAutomatically, &clone), Decision::Forget);
            assert_eq!(decide(WhenFinished::Ask, &clone), Decision::Forget);
            assert_eq!(decide(WhenFinished::Keep, &clone), Decision::Keep);
        }
    }
}
