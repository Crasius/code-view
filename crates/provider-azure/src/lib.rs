//! Azure DevOps Services and Server provider.
//!
//! Repository keys are `organization/project/repository` (raw names; each part is URL-encoded
//! per request). On Azure DevOps Server the "organization" is the collection and `base_url` the
//! server root. Requests use `api-version=7.1` and Basic auth with the PAT when one is set.
//!
//! Mapping notes:
//! * Thread anchors: right side → `new_line`, left side → `old_line`. Comments on unchanged context
//!   lines only carry the right side, so `old_line` stays `None` for them.
//! * Outdated threads: Azure DevOps has no flag; a filled `trackingCriteria` (the anchor was re-tracked
//!   onto a newer iteration) counts as outdated, otherwise false. Such a thread's origin is its
//!   `iterationContext` (first == second compares that iteration's common commit with its source
//!   commit, otherwise the two iterations' source commits) and the `orig*` span of the criteria;
//!   reading the iterations' commits needs a token, so anonymous connections get no origin.
//!   New threads are written on the app's diff (merge base against head), so their
//!   `iterationContext` names the latest iteration on both sides.
//! * Labels are free-text tags: [`Provider::available_labels`] returns an empty list.
//! * Merge strategies: Merge = `noFastForward`, Squash = `squash`, Rebase = `rebase` (rebase and
//!   fast-forward), `SemiLinear` = `rebaseMerge`. The target branch's "Require a merge strategy"
//!   policy narrows them (see `merge_policy`); an auto-complete's strategy is the pull request's default.
//! * Pull request statuses need sign-in even on public projects; anonymous connections see builds only.
//! * List rows show the branch policies' evaluations (`policies`), one request per pull request.
//! * Stage dependencies: the build timeline has no `dependsOn`, and a stage's `order` stays
//!   sequential even for parallel stages, so the graph is inferred from start and finish times.
//! * Stage reruns use the documented stage update (`state: retry`), keyed by the timeline
//!   record's `identifier`; Azure DevOps has no run-level rerun call.
//! * Attempts: timeline records carry `attempt` and `previousAttempts` (timeline and record ids
//!   only). An earlier attempt's records, with their logs, come from that attempt's own timeline,
//!   read when the user opens it.

// RS-DONT-03: no FFI in this crate.
#![forbid(unsafe_code)]

mod client;
mod dto;
mod identity;
mod images;
mod iterations;
mod linked;
mod pipelines;
mod pull_requests;
mod repositories;
pub mod setup;
mod threads;

use async_trait::async_trait;
use review_core::provider::CommentActions;
use review_core::{
    AccessProblem, AccessReport, Capabilities, Connection, DiffStat, GitRemote, ItemRef, Label,
    LinkedItem, MergeOptions, MergePolicy, MergeStrategy, NewThread, PipelineJob, PipelineRun,
    Provider, ProviderError, PullRequestDetail, PullRequestQuery, PullRequestRevision,
    PullRequestSummary, RepositoryInfo, RerunAction, Result, ReviewThread, ReviewVote, RowStatus,
    RunningLogs, ScopeCandidate, ThreadComment, ThreadStatus, UserIdentity,
};
use tokio::io::AsyncWrite;

use crate::client::Client;
use crate::identity::CurrentUsers;
use crate::pull_requests::policies::{self, ProjectIds};
use crate::pull_requests::{labels, merge_policy, reviews, revision};
use crate::setup::TokenFacts;

pub struct AzureDevOpsProvider {
    client: Client,
    users: CurrentUsers,
    projects: ProjectIds,
    capabilities: Capabilities,
    token: TokenFacts,
}

impl AzureDevOpsProvider {
    pub fn new(connection: Connection, http: reqwest::Client) -> Self {
        let capabilities = Capabilities {
            votes: vec![
                ReviewVote::Approved,
                ReviewVote::ApprovedWithSuggestions,
                ReviewVote::None,
                ReviewVote::WaitingForAuthor,
                ReviewVote::ChangesRequested,
            ],
            merge_strategies: MergeStrategy::ALL.to_vec(),
            has_label_catalog: false,
            // A timeline record lists its log once the agent uploaded some of it, usually when the
            // task finished; what is uploaded is served as it grows.
            running_logs: RunningLogs::Growing,
        };
        let token = TokenFacts::of(connection.token());
        AzureDevOpsProvider {
            client: Client::new(connection, http),
            users: CurrentUsers::default(),
            projects: ProjectIds::default(),
            capabilities,
            token,
        }
    }
}

#[async_trait]
impl Provider for AzureDevOpsProvider {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// The authenticated user of the first organization (identity ids differ per organization).
    async fn current_user(&self) -> Result<UserIdentity> {
        let organization = &self.client.organizations()?[0];
        self.users.get(&self.client, organization).await
    }

    async fn list_repositories(&self, search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        repositories::list(&self.client, search).await
    }

    async fn git_remote(&self, repo: &str) -> Result<GitRemote> {
        repositories::git_remote(&self.client, repo)
    }

    async fn list_pull_requests(
        &self,
        query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        pull_requests::list(&self.client, &self.users, query).await
    }

    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        pull_requests::detail(&self.client, repo, number).await
    }

    async fn diff_stats(&self, repo: &str, numbers: &[u64]) -> Result<Vec<(u64, DiffStat)>> {
        pull_requests::diff_stats(&self.client, repo, numbers).await
    }

    async fn row_statuses(&self, repo: &str, numbers: &[u64]) -> Result<Vec<(u64, RowStatus)>> {
        policies::row_statuses(&self.client, &self.projects, repo, numbers).await
    }

    async fn merge_policy(&self, repo: &str, target_branch: &str) -> Result<MergePolicy> {
        merge_policy::policy(&self.client, repo, target_branch).await
    }

    async fn merge(&self, repo: &str, number: u64, options: &MergeOptions) -> Result<()> {
        pull_requests::merge(&self.client, repo, number, options).await
    }

    async fn set_auto_complete(
        &self,
        repo: &str,
        number: u64,
        options: &MergeOptions,
    ) -> Result<()> {
        pull_requests::set_auto_complete(&self.client, &self.users, repo, number, options).await
    }

    async fn cancel_auto_complete(&self, repo: &str, number: u64) -> Result<()> {
        pull_requests::cancel_auto_complete(&self.client, repo, number).await
    }

    async fn close(&self, repo: &str, number: u64) -> Result<()> {
        pull_requests::set_status(&self.client, repo, number, "abandoned").await
    }

    async fn reopen(&self, repo: &str, number: u64) -> Result<()> {
        pull_requests::set_status(&self.client, repo, number, "active").await
    }

    async fn set_draft(&self, repo: &str, number: u64, is_draft: bool) -> Result<()> {
        pull_requests::set_draft(&self.client, repo, number, is_draft).await
    }

    async fn update_title_description(
        &self,
        repo: &str,
        number: u64,
        title: &str,
        description: &str,
    ) -> Result<()> {
        pull_requests::update_title_description(&self.client, repo, number, title, description)
            .await
    }

    async fn set_vote(
        &self,
        repo: &str,
        number: u64,
        vote: ReviewVote,
        comment: Option<&str>,
    ) -> Result<()> {
        reviews::set_vote(&self.client, &self.users, repo, number, vote, comment).await
    }

    async fn add_reviewer(&self, repo: &str, number: u64, user: &str) -> Result<()> {
        reviews::add_reviewer(&self.client, &self.users, repo, number, user).await
    }

    async fn remove_reviewer(&self, repo: &str, number: u64, reviewer_id: &str) -> Result<()> {
        reviews::remove_reviewer(&self.client, repo, number, reviewer_id).await
    }

    async fn threads(&self, repo: &str, number: u64) -> Result<Vec<ReviewThread>> {
        threads::list(&self.client, repo, number).await
    }

    async fn create_thread(
        &self,
        repo: &str,
        number: u64,
        thread: &NewThread,
    ) -> Result<ReviewThread> {
        threads::create(&self.client, repo, number, thread).await
    }

    async fn reply(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        body: &str,
    ) -> Result<ThreadComment> {
        threads::reply(&self.client, repo, number, thread_id, body).await
    }

    async fn set_thread_status(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        status: ThreadStatus,
    ) -> Result<()> {
        threads::set_status(&self.client, repo, number, thread_id, status).await
    }

    fn comment_actions(&self, thread: &ReviewThread) -> CommentActions {
        threads::comment_actions(thread)
    }

    async fn edit_comment(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<ThreadComment> {
        threads::edit_comment(&self.client, repo, number, thread_id, comment_id, body).await
    }

    async fn delete_comment(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        comment_id: &str,
    ) -> Result<()> {
        threads::delete_comment(&self.client, repo, number, thread_id, comment_id).await
    }

    async fn revision(&self, repo: &str, number: u64) -> Result<PullRequestRevision> {
        revision::revision(&self.client, repo, number).await
    }

    async fn linked_items(&self, references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
        linked::items(&self.client, references).await
    }

    async fn linked_references(&self, repo: &str, number: u64) -> Result<Vec<ItemRef>> {
        linked::references(&self.client, repo, number).await
    }

    async fn available_labels(&self, _repo: &str) -> Result<Vec<Label>> {
        // Free-text tags: no catalog (`has_label_catalog` is false).
        Ok(Vec::new())
    }

    async fn set_labels(&self, repo: &str, number: u64, labels: &[String]) -> Result<()> {
        labels::set(&self.client, repo, number, labels).await
    }

    async fn pipeline_runs(&self, repo: &str, number: u64) -> Result<Vec<PipelineRun>> {
        pipelines::runs(&self.client, repo, number).await
    }

    /// Run ids are unique per project, so `number` is not needed.
    async fn pipeline_jobs(
        &self,
        repo: &str,
        _number: u64,
        run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        pipelines::jobs(&self.client, repo, run_id).await
    }

    async fn pipeline_attempt(
        &self,
        repo: &str,
        _number: u64,
        _run_id: &str,
        target: &str,
    ) -> Result<PipelineJob> {
        pipelines::attempt(&self.client, repo, target).await
    }

    async fn download_log(
        &self,
        repo: &str,
        _number: u64,
        log_id: &str,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()> {
        pipelines::download_log(&self.client, repo, log_id, sink).await
    }

    async fn rerun(
        &self,
        repo: &str,
        _number: u64,
        _run_id: &str,
        action: &RerunAction,
    ) -> Result<()> {
        pipelines::rerun(&self.client, repo, action).await
    }

    async fn inspect_access(&self) -> Result<AccessReport> {
        setup::inspect(&self.client, &self.token).await
    }

    async fn discover_scopes(&self) -> Result<Vec<ScopeCandidate>> {
        setup::discover(&self.client).await
    }

    fn explain_denial(&self, error: &ProviderError) -> Option<AccessProblem> {
        setup::explain(error)
    }

    async fn fetch_image(
        &self,
        _repo: &str,
        url: &str,
        limit: u64,
        known: &review_core::http::image::Validators,
    ) -> Result<review_core::http::image::Body> {
        images::fetch(&self.client, url, limit, known).await
    }
}
