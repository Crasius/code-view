//! A scripted [`Provider`] for tests that are not about HTTP: a fake implements [`Script`] with
//! the methods its flow asks for, and [`Scripted`] makes it the `Arc<dyn Provider>` a
//! `ProviderFactory` hands out. Every other method answers as an empty platform (lists) or refuses
//! (single items and writes), so a new required `Provider` method edits this file instead of each
//! fake.
//!
//! The wrapper exists because the orphan rule refuses `impl<S: Script> Provider for S` (E0210):
//! `Provider` is review-core's trait, `S` any type.

use std::sync::LazyLock;

use async_trait::async_trait;
use review_core::setup::{self, AccessProblem, AccessReport, ScopeCandidate};
use review_core::{
    Capabilities, GitRemote, ItemRef, Label, LinkedItem, MergeOptions, MergePolicy, MergeStrategy,
    NewThread, PipelineJob, PipelineRun, Provider, ProviderError, PullRequestDetail,
    PullRequestQuery, PullRequestRevision, PullRequestSummary, RepositoryInfo, RerunAction, Result,
    ReviewThread, ReviewVote, RunningLogs, ThreadComment, ThreadStatus, Unreported, UserIdentity,
};
use tokio::io::AsyncWrite;

use crate::samples;

/// What [`Script`] answers when a test scripts no capabilities: votes Approved and None, the merge
/// strategy Merge, no label catalog, logs when a job finished.
pub static CAPABILITIES: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    votes: vec![ReviewVote::Approved, ReviewVote::None],
    merge_strategies: vec![MergeStrategy::Merge],
    has_label_catalog: false,
    running_logs: RunningLogs::WhenFinished,
});

/// The refusal of a method the test did not script.
fn refused<T>(method: &str) -> Result<T> {
    Err(ProviderError::Unsupported(format!("{method} is not scripted in this test.")))
}

/// A provider for a test that is not about HTTP: implement what the test's flow asks for.
/// Everything else answers as an empty platform (lists) or refuses with
/// `ProviderError::Unsupported("<method> is not scripted in this test.")` (single items and writes).
///
/// Each method has the signature of the [`Provider`] method of the same name. `merge_policy`,
/// `revision`, `inspect_access` and `explain_denial` repeat the bodies of `Provider`'s own
/// defaults (review-core's `provider.rs`), so they run on what the script answers; a changed
/// default there changes here too. `Provider`'s other defaulted methods are not mirrored and keep
/// their default; the test that first needs one adds it here and to [`Scripted`].
#[async_trait]
pub trait Script: Send + Sync {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(samples::author())
    }

    async fn list_repositories(&self, _search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        Ok(Vec::new())
    }

    async fn git_remote(&self, _repo: &str) -> Result<GitRemote> {
        refused("git_remote")
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        Ok(Vec::new())
    }

    async fn pull_request(&self, _repo: &str, _number: u64) -> Result<PullRequestDetail> {
        refused("pull_request")
    }

    async fn merge_policy(&self, _repo: &str, _target_branch: &str) -> Result<MergePolicy> {
        Ok(MergePolicy::not_reported(
            &self.capabilities().merge_strategies,
            Unreported::Unsupported,
        ))
    }

    async fn merge(&self, _repo: &str, _number: u64, _options: &MergeOptions) -> Result<()> {
        refused("merge")
    }

    async fn set_auto_complete(
        &self,
        _repo: &str,
        _number: u64,
        _options: &MergeOptions,
    ) -> Result<()> {
        refused("set_auto_complete")
    }

    async fn cancel_auto_complete(&self, _repo: &str, _number: u64) -> Result<()> {
        refused("cancel_auto_complete")
    }

    async fn close(&self, _repo: &str, _number: u64) -> Result<()> {
        refused("close")
    }

    async fn reopen(&self, _repo: &str, _number: u64) -> Result<()> {
        refused("reopen")
    }

    async fn set_draft(&self, _repo: &str, _number: u64, _is_draft: bool) -> Result<()> {
        refused("set_draft")
    }

    async fn update_title_description(
        &self,
        _repo: &str,
        _number: u64,
        _title: &str,
        _description: &str,
    ) -> Result<()> {
        refused("update_title_description")
    }

    async fn set_vote(
        &self,
        _repo: &str,
        _number: u64,
        _vote: ReviewVote,
        _comment: Option<&str>,
    ) -> Result<()> {
        refused("set_vote")
    }

    async fn add_reviewer(&self, _repo: &str, _number: u64, _user: &str) -> Result<()> {
        refused("add_reviewer")
    }

    async fn remove_reviewer(&self, _repo: &str, _number: u64, _reviewer_id: &str) -> Result<()> {
        refused("remove_reviewer")
    }

    async fn threads(&self, _repo: &str, _number: u64) -> Result<Vec<ReviewThread>> {
        Ok(Vec::new())
    }

    async fn create_thread(
        &self,
        _repo: &str,
        _number: u64,
        _thread: &NewThread,
    ) -> Result<ReviewThread> {
        refused("create_thread")
    }

    async fn reply(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _body: &str,
    ) -> Result<ThreadComment> {
        refused("reply")
    }

    async fn set_thread_status(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _status: ThreadStatus,
    ) -> Result<()> {
        refused("set_thread_status")
    }

    async fn revision(&self, repo: &str, number: u64) -> Result<PullRequestRevision> {
        let (detail, threads) =
            tokio::try_join!(self.pull_request(repo, number), self.threads(repo, number))?;
        Ok(PullRequestRevision::of(&detail, &threads))
    }

    async fn linked_items(&self, _references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
        Ok(Vec::new())
    }

    async fn linked_references(&self, _repo: &str, _number: u64) -> Result<Vec<ItemRef>> {
        Ok(Vec::new())
    }

    async fn available_labels(&self, _repo: &str) -> Result<Vec<Label>> {
        Ok(Vec::new())
    }

    async fn set_labels(&self, _repo: &str, _number: u64, _labels: &[String]) -> Result<()> {
        refused("set_labels")
    }

    async fn pipeline_runs(&self, _repo: &str, _number: u64) -> Result<Vec<PipelineRun>> {
        Ok(Vec::new())
    }

    async fn pipeline_jobs(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        Ok(Vec::new())
    }

    async fn pipeline_attempt(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
        _target: &str,
    ) -> Result<PipelineJob> {
        refused("pipeline_attempt")
    }

    async fn download_log(
        &self,
        _repo: &str,
        _number: u64,
        _log_id: &str,
        _sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()> {
        refused("download_log")
    }

    async fn rerun(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
        _action: &RerunAction,
    ) -> Result<()> {
        refused("rerun")
    }

    async fn inspect_access(&self) -> Result<AccessReport> {
        let user = self.current_user().await?;
        Ok(AccessReport::unverified(user, "Personal access token"))
    }

    async fn discover_scopes(&self) -> Result<Vec<ScopeCandidate>> {
        Ok(Vec::new())
    }

    fn explain_denial(&self, error: &ProviderError) -> Option<AccessProblem> {
        setup::generic_denial(error)
    }
}

/// `Arc::new(Scripted(script))` is the `Arc<dyn Provider>` a `ProviderFactory` hands out.
pub struct Scripted<S>(pub S);

#[async_trait]
impl<S: Script> Provider for Scripted<S> {
    fn capabilities(&self) -> &Capabilities {
        self.0.capabilities()
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        self.0.current_user().await
    }

    async fn list_repositories(&self, search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        self.0.list_repositories(search).await
    }

    async fn git_remote(&self, repo: &str) -> Result<GitRemote> {
        self.0.git_remote(repo).await
    }

    async fn list_pull_requests(
        &self,
        query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        self.0.list_pull_requests(query).await
    }

    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        self.0.pull_request(repo, number).await
    }

    async fn merge_policy(&self, repo: &str, target_branch: &str) -> Result<MergePolicy> {
        self.0.merge_policy(repo, target_branch).await
    }

    async fn merge(&self, repo: &str, number: u64, options: &MergeOptions) -> Result<()> {
        self.0.merge(repo, number, options).await
    }

    async fn set_auto_complete(
        &self,
        repo: &str,
        number: u64,
        options: &MergeOptions,
    ) -> Result<()> {
        self.0.set_auto_complete(repo, number, options).await
    }

    async fn cancel_auto_complete(&self, repo: &str, number: u64) -> Result<()> {
        self.0.cancel_auto_complete(repo, number).await
    }

    async fn close(&self, repo: &str, number: u64) -> Result<()> {
        self.0.close(repo, number).await
    }

    async fn reopen(&self, repo: &str, number: u64) -> Result<()> {
        self.0.reopen(repo, number).await
    }

    async fn set_draft(&self, repo: &str, number: u64, is_draft: bool) -> Result<()> {
        self.0.set_draft(repo, number, is_draft).await
    }

    async fn update_title_description(
        &self,
        repo: &str,
        number: u64,
        title: &str,
        description: &str,
    ) -> Result<()> {
        self.0.update_title_description(repo, number, title, description).await
    }

    async fn set_vote(
        &self,
        repo: &str,
        number: u64,
        vote: ReviewVote,
        comment: Option<&str>,
    ) -> Result<()> {
        self.0.set_vote(repo, number, vote, comment).await
    }

    async fn add_reviewer(&self, repo: &str, number: u64, user: &str) -> Result<()> {
        self.0.add_reviewer(repo, number, user).await
    }

    async fn remove_reviewer(&self, repo: &str, number: u64, reviewer_id: &str) -> Result<()> {
        self.0.remove_reviewer(repo, number, reviewer_id).await
    }

    async fn threads(&self, repo: &str, number: u64) -> Result<Vec<ReviewThread>> {
        self.0.threads(repo, number).await
    }

    async fn create_thread(
        &self,
        repo: &str,
        number: u64,
        thread: &NewThread,
    ) -> Result<ReviewThread> {
        self.0.create_thread(repo, number, thread).await
    }

    async fn reply(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        body: &str,
    ) -> Result<ThreadComment> {
        self.0.reply(repo, number, thread_id, body).await
    }

    async fn set_thread_status(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        status: ThreadStatus,
    ) -> Result<()> {
        self.0.set_thread_status(repo, number, thread_id, status).await
    }

    async fn revision(&self, repo: &str, number: u64) -> Result<PullRequestRevision> {
        self.0.revision(repo, number).await
    }

    async fn linked_items(&self, references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
        self.0.linked_items(references).await
    }

    async fn linked_references(&self, repo: &str, number: u64) -> Result<Vec<ItemRef>> {
        self.0.linked_references(repo, number).await
    }

    async fn available_labels(&self, repo: &str) -> Result<Vec<Label>> {
        self.0.available_labels(repo).await
    }

    async fn set_labels(&self, repo: &str, number: u64, labels: &[String]) -> Result<()> {
        self.0.set_labels(repo, number, labels).await
    }

    async fn pipeline_runs(&self, repo: &str, number: u64) -> Result<Vec<PipelineRun>> {
        self.0.pipeline_runs(repo, number).await
    }

    async fn pipeline_jobs(
        &self,
        repo: &str,
        number: u64,
        run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        self.0.pipeline_jobs(repo, number, run_id).await
    }

    async fn pipeline_attempt(
        &self,
        repo: &str,
        number: u64,
        run_id: &str,
        target: &str,
    ) -> Result<PipelineJob> {
        self.0.pipeline_attempt(repo, number, run_id, target).await
    }

    async fn download_log(
        &self,
        repo: &str,
        number: u64,
        log_id: &str,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()> {
        self.0.download_log(repo, number, log_id, sink).await
    }

    async fn rerun(
        &self,
        repo: &str,
        number: u64,
        run_id: &str,
        action: &RerunAction,
    ) -> Result<()> {
        self.0.rerun(repo, number, run_id, action).await
    }

    async fn inspect_access(&self) -> Result<AccessReport> {
        self.0.inspect_access().await
    }

    async fn discover_scopes(&self) -> Result<Vec<ScopeCandidate>> {
        self.0.discover_scopes().await
    }

    fn explain_denial(&self, error: &ProviderError) -> Option<AccessProblem> {
        self.0.explain_denial(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use review_core::{
        MergeOptions, MergeStrategy, Provider, ProviderError, PullRequestDetail,
        PullRequestRevision, ReviewThread, ThreadStatus,
    };

    use super::{Script, Scripted};
    use crate::samples;

    struct PullRequestWithThread;

    fn thread() -> ReviewThread {
        ReviewThread {
            id: "t1".to_owned(),
            status: ThreadStatus::Active,
            anchor: None,
            is_outdated: false,
            origin: None,
            is_system: false,
            can_reply: true,
            can_resolve: true,
            comments: Vec::new(),
        }
    }

    #[async_trait::async_trait]
    impl Script for PullRequestWithThread {
        async fn pull_request(
            &self,
            repo: &str,
            number: u64,
        ) -> review_core::Result<PullRequestDetail> {
            Ok(samples::detail(repo, number))
        }

        async fn threads(
            &self,
            _repo: &str,
            _number: u64,
        ) -> review_core::Result<Vec<ReviewThread>> {
            Ok(vec![thread()])
        }
    }

    #[test]
    fn a_script_answers_what_it_scripts_and_refuses_the_rest() {
        let provider: Arc<dyn Provider> = Arc::new(Scripted(PullRequestWithThread));
        let runtime =
            tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        runtime.block_on(async {
            let detail = provider.pull_request("acme/app", 7).await.expect("scripted detail");
            assert_eq!(detail, samples::detail("acme/app", 7));
            let threads = provider.threads("acme/app", 7).await.expect("scripted threads");
            assert_eq!(threads, vec![thread()]);

            let options = MergeOptions {
                strategy: MergeStrategy::Merge,
                delete_source_branch: false,
                commit_message: None,
                head_sha: detail.head_sha.clone(),
            };
            let merge = provider.merge("acme/app", 7, &options).await;
            assert!(
                matches!(&merge, Err(ProviderError::Unsupported(message)) if message.starts_with("merge ")),
                "{merge:?}"
            );

            let revision = provider.revision("acme/app", 7).await.expect("revision");
            assert_eq!(revision, PullRequestRevision::of(&detail, &threads));
        });
    }
}
