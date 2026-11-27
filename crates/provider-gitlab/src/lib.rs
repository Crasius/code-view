//! GitLab provider (gitlab.com and self-managed) for the review app, built on the REST API v4.
//!
//! Repository keys are `path_with_namespace`; pull request numbers are merge request iids.
//! Requests carry `PRIVATE-TOKEN` when the connection has a token; every write requires one.

// RS-DONT-03: no FFI in this crate.
#![forbid(unsafe_code)]

mod client;
mod discussions;
mod dto;
mod graphql;
mod images;
mod linked;
mod merge_requests;
mod pipeline_mapping;
mod pipeline_names;
mod pipelines;
mod repositories;
pub mod setup;

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

use crate::client::ApiClient;
use crate::merge_requests::{labels, lifecycle, merge_policy, reviews, revision};
use crate::repositories::Projects;

pub struct GitLabProvider {
    client: ApiClient,
    organizations: Vec<String>,
    capabilities: Capabilities,
    /// The project lookup the git remote and the merge policy share.
    projects: Projects,
}

impl GitLabProvider {
    pub fn new(connection: Connection, http: reqwest::Client) -> Self {
        let client = ApiClient::new(&connection, http);
        // No ChangesRequested: GitLab's REST API has no endpoint to request changes.
        // Merge commit or squash: what is safe to send without knowing the project's merge method
        // (`merge_policy` reads it and offers fast-forward or semi-linear where the project uses them).
        let capabilities = Capabilities {
            votes: vec![ReviewVote::Approved, ReviewVote::None],
            merge_strategies: vec![MergeStrategy::Merge, MergeStrategy::Squash],
            has_label_catalog: true,
            // The job trace of a running job is what it has written so far
            // (<https://docs.gitlab.com/api/jobs/#retrieve-a-log-file-for-a-job>).
            running_logs: RunningLogs::Growing,
        };
        Self {
            client,
            organizations: connection.organizations,
            capabilities,
            projects: Projects::default(),
        }
    }
}

#[async_trait]
impl Provider for GitLabProvider {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        repositories::current_user(&self.client).await
    }

    async fn list_repositories(&self, search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        repositories::list_repositories(&self.client, &self.organizations, search).await
    }

    async fn git_remote(&self, repo: &str) -> Result<GitRemote> {
        repositories::git_remote(&self.client, &self.projects, repo).await
    }

    async fn list_pull_requests(
        &self,
        query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        merge_requests::list(&self.client, &self.organizations, query).await
    }

    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        merge_requests::detail(&self.client, repo, number).await
    }

    async fn diff_stats(&self, repo: &str, numbers: &[u64]) -> Result<Vec<(u64, DiffStat)>> {
        merge_requests::diff_stats(&self.client, repo, numbers).await
    }

    async fn row_statuses(&self, repo: &str, numbers: &[u64]) -> Result<Vec<(u64, RowStatus)>> {
        merge_requests::statuses::row_statuses(&self.client, repo, numbers).await
    }

    async fn merge_policy(&self, repo: &str, _target_branch: &str) -> Result<MergePolicy> {
        let capable = &self.capabilities.merge_strategies;
        merge_policy::policy(&self.client, &self.projects, repo, capable).await
    }

    async fn merge(&self, repo: &str, number: u64, options: &MergeOptions) -> Result<()> {
        lifecycle::merge(&self.client, repo, number, options).await
    }

    async fn set_auto_complete(
        &self,
        repo: &str,
        number: u64,
        options: &MergeOptions,
    ) -> Result<()> {
        lifecycle::set_auto_merge(&self.client, repo, number, options).await
    }

    async fn cancel_auto_complete(&self, repo: &str, number: u64) -> Result<()> {
        lifecycle::cancel_auto_merge(&self.client, repo, number).await
    }

    async fn close(&self, repo: &str, number: u64) -> Result<()> {
        lifecycle::close(&self.client, repo, number).await
    }

    async fn reopen(&self, repo: &str, number: u64) -> Result<()> {
        lifecycle::reopen(&self.client, repo, number).await
    }

    async fn set_draft(&self, repo: &str, number: u64, is_draft: bool) -> Result<()> {
        lifecycle::set_draft(&self.client, repo, number, is_draft).await
    }

    async fn update_title_description(
        &self,
        repo: &str,
        number: u64,
        title: &str,
        description: &str,
    ) -> Result<()> {
        lifecycle::update_title_description(&self.client, repo, number, title, description).await
    }

    async fn set_vote(
        &self,
        repo: &str,
        number: u64,
        vote: ReviewVote,
        comment: Option<&str>,
    ) -> Result<()> {
        reviews::set_vote(&self.client, repo, number, vote, comment).await
    }

    async fn add_reviewer(&self, repo: &str, number: u64, user: &str) -> Result<()> {
        reviews::add_reviewer(&self.client, repo, number, user).await
    }

    async fn remove_reviewer(&self, repo: &str, number: u64, reviewer_id: &str) -> Result<()> {
        reviews::remove_reviewer(&self.client, repo, number, reviewer_id).await
    }

    async fn threads(&self, repo: &str, number: u64) -> Result<Vec<ReviewThread>> {
        discussions::threads(&self.client, repo, number).await
    }

    async fn create_thread(
        &self,
        repo: &str,
        number: u64,
        thread: &NewThread,
    ) -> Result<ReviewThread> {
        discussions::create_thread(&self.client, repo, number, thread).await
    }

    async fn reply(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        body: &str,
    ) -> Result<ThreadComment> {
        discussions::reply(&self.client, repo, number, thread_id, body).await
    }

    async fn set_thread_status(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        status: ThreadStatus,
    ) -> Result<()> {
        discussions::set_status(&self.client, repo, number, thread_id, status).await
    }

    fn comment_actions(&self, thread: &ReviewThread) -> CommentActions {
        discussions::comment_actions(thread)
    }

    fn thread_page_suffix(&self, thread: &ReviewThread) -> Option<String> {
        discussions::page_suffix(thread)
    }

    async fn edit_comment(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<ThreadComment> {
        discussions::edit_note(&self.client, repo, number, thread_id, comment_id, body).await
    }

    async fn delete_comment(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        comment_id: &str,
    ) -> Result<()> {
        discussions::delete_note(&self.client, repo, number, thread_id, comment_id).await
    }

    async fn revision(&self, repo: &str, number: u64) -> Result<PullRequestRevision> {
        revision::revision(&self.client, repo, number).await
    }

    async fn linked_items(&self, references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
        linked::items(&self.client, references).await
    }

    async fn available_labels(&self, repo: &str) -> Result<Vec<Label>> {
        labels::available(&self.client, repo).await
    }

    async fn set_labels(&self, repo: &str, number: u64, labels: &[String]) -> Result<()> {
        labels::set(&self.client, repo, number, labels).await
    }

    async fn pipeline_runs(&self, repo: &str, number: u64) -> Result<Vec<PipelineRun>> {
        pipelines::runs(&self.client, repo, number).await
    }

    async fn pipeline_jobs(
        &self,
        _repo: &str,
        _number: u64,
        run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        // Three paged listings run at once; boxed, so their state stays off the caller's future.
        Box::pin(pipelines::jobs(&self.client, run_id)).await
    }

    async fn download_log(
        &self,
        _repo: &str,
        _number: u64,
        log_id: &str,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()> {
        pipelines::download_log(&self.client, log_id, sink).await
    }

    async fn rerun(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
        action: &RerunAction,
    ) -> Result<()> {
        pipelines::rerun(&self.client, action).await
    }

    async fn inspect_access(&self) -> Result<AccessReport> {
        setup::inspect(&self.client).await
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
