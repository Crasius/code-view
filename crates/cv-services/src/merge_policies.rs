//! Merge policies per connection, repository and target branch, shared by every page.
//!
//! Nothing loads on its own: the page of an open pull request asks ([`MergePolicies::want`])
//! for its merge button and merge card while it is on screen, and only while the window is active. An
//! answer stays for `FRESH_FOR`; after that the next ask loads it again behind the shown one.
//! A failed read becomes the platform's full capability list marked as not reported, and stays
//! until the user retries, so a broken host is not asked again on every frame.

use std::collections::HashMap;
use std::sync::Arc;

use review_core::{MergePolicy, MergeStrategy, Provider, ProviderError, Unreported};

use cv_runtime::background::{Background, TASK_LOST, Task, TaskPoll};

/// Branch policies and merge settings change rarely.
pub(crate) const FRESH_FOR: f64 = 30.0 * 60.0;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PolicyKey {
    pub connection_id: String,
    pub repository: String,
    pub target_branch: String,
}

/// What a view can show for one key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PolicyView<'a> {
    /// Not loaded yet (asked while the window was inactive, or loading): reserve the space.
    Pending,
    Ready(&'a MergePolicy),
}

struct Entry {
    shown: Option<(MergePolicy, f64)>,
    task: Option<Task<Result<MergePolicy, ProviderError>>>,
    /// The capability list a failure falls back to.
    capable: Vec<MergeStrategy>,
}

#[derive(Default)]
pub struct MergePolicies {
    entries: HashMap<PolicyKey, Entry>,
}

impl MergePolicies {
    /// The policy of `key`, loading it when unknown or older than `FRESH_FOR` and `is_active`.
    pub fn want(
        &mut self,
        background: &Background,
        provider: &Arc<dyn Provider>,
        key: &PolicyKey,
        now: f64,
        is_active: bool,
    ) -> PolicyView<'_> {
        let capable = provider.capabilities().merge_strategies.clone();
        let entry = self.entries.entry(key.clone()).or_insert_with(|| Entry {
            shown: None,
            task: None,
            capable,
        });
        let is_stale = entry
            .shown
            .as_ref()
            .is_none_or(|(policy, at)| is_success(policy) && now - at > FRESH_FOR);
        if is_stale && entry.task.is_none() && is_active {
            let provider = provider.clone();
            let (repository, branch) = (key.repository.clone(), key.target_branch.clone());
            entry.task = Some(
                background.spawn(async move { provider.merge_policy(&repository, &branch).await }),
            );
        }
        match &entry.shown {
            Some((policy, _)) => PolicyView::Ready(policy),
            None => PolicyView::Pending,
        }
    }

    /// Asks again on the next [`Self::want`] (Retry after a failure).
    pub fn forget(&mut self, key: &PolicyKey) {
        if let Some(entry) = self.entries.get_mut(key)
            && entry.task.is_none()
        {
            entry.shown = None;
        }
    }

    /// Moves finished loads in. Returns true when anything arrived.
    pub fn poll(&mut self, now: f64) -> bool {
        let mut changed = false;
        for entry in self.entries.values_mut() {
            let Some(task) = &mut entry.task else {
                continue;
            };
            let failure = match task.poll() {
                TaskPoll::Pending => continue,
                TaskPoll::Done(Ok(policy)) => Ok(policy),
                TaskPoll::Done(Err(error)) => Err(error.to_string()),
                TaskPoll::Lost => Err(TASK_LOST.to_owned()),
            };
            entry.task = None;
            let policy = match (failure, entry.shown.take()) {
                (Ok(policy), _) => policy,
                // A refresh that failed keeps the earlier answer on screen, for another FRESH_FOR.
                (Err(_), Some((known, _))) if is_success(&known) => known,
                (Err(message), _) => {
                    MergePolicy::not_reported(&entry.capable, Unreported::Failed(message))
                }
            };
            entry.shown = Some((policy, now));
            changed = true;
        }
        changed
    }
}

/// Failures stay until Retry; successes refresh after [`FRESH_FOR`].
fn is_success(policy: &MergePolicy) -> bool {
    !matches!(policy.source, review_core::PolicySource::NotReported(Unreported::Failed(_)))
}
