//! GitHub and GitHub Enterprise Server provider: REST v3 for most calls, GraphQL where REST
//! has no equivalent (review threads, resolving, drafts, "mine" searches).

// RS-DONT-03: no FFI here; unsafe belongs in the platform/ modules of review-store and review-app.
#![forbid(unsafe_code)]

mod client;
mod dto;
mod graphql;
mod images;
mod linked;
mod mapping;
mod owners;
mod pipelines;
mod pull_requests;
mod repo_key;
mod repositories;
pub mod setup;
mod sso;
mod threads;

use async_trait::async_trait;
use review_core::provider::CommentActions;
use review_core::{
    AccessProblem, AccessReport, Capabilities, Connection, DiffStat, GitRemote, ItemRef, Label,
    LinkedItem, MergeOptions, MergePolicy, MergeStrategy, NewThread, PipelineJob, PipelineRun,
    Provider, ProviderError, PullRequestDetail, PullRequestQuery, PullRequestRevision,
    PullRequestScope, PullRequestSummary, RepositoryInfo, RerunAction, Result, ReviewThread,
    ReviewVote, RowStatus, RunningLogs, ScopeCandidate, StateFilter, ThreadComment, ThreadStatus,
    UserIdentity,
};
use tokio::io::AsyncWrite;

use crate::client::GitHubClient;
use crate::pull_requests::{auto_merge, labels, lifecycle, merge_policy, reviews, revision};
use crate::threads::{edits as comment_edits, writes as thread_writes};

pub struct GitHubProvider {
    client: GitHubClient,
    capabilities: Capabilities,
    workflows: pipelines::WorkflowCache,
}

impl GitHubProvider {
    /// `http` is the shared, pooled client (see [`review_core::http::build_client`]); auth goes on each request.
    pub fn new(connection: Connection, http: reqwest::Client) -> Self {
        let capabilities = Capabilities {
            votes: vec![ReviewVote::Approved, ReviewVote::ChangesRequested],
            merge_strategies: vec![
                MergeStrategy::Merge,
                MergeStrategy::Squash,
                MergeStrategy::Rebase,
            ],
            has_label_catalog: true,
            // The job log is one file written when the job completes; the REST API documents no
            // live log (<https://docs.github.com/rest/actions/workflow-jobs#download-job-logs-for-a-workflow-run>).
            running_logs: RunningLogs::WhenFinished,
        };
        Self {
            client: GitHubClient::new(connection, http),
            capabilities,
            workflows: pipelines::WorkflowCache::default(),
        }
    }
}

#[async_trait]
impl Provider for GitHubProvider {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        repositories::current_user(&self.client).await
    }

    async fn list_repositories(&self, search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        repositories::list(&self.client, search).await
    }

    async fn git_remote(&self, repo: &str) -> Result<GitRemote> {
        repositories::git_remote(self.client.connection(), repo)
    }

    async fn list_pull_requests(
        &self,
        query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        pull_requests::list(&self.client, query).await
    }

    /// With a token one search covers ten repositories; without one, search is closed and each
    /// repository lists through REST.
    fn lists_repositories_together(&self) -> bool {
        self.client.has_token()
    }

    async fn list_repositories_pull_requests(
        &self,
        repositories: &[String],
        role: PullRequestScope,
        state: StateFilter,
        top: usize,
    ) -> Result<Vec<PullRequestSummary>> {
        pull_requests::list_repositories(&self.client, repositories, role, state, top).await
    }

    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        pull_requests::detail(&self.client, repo, number).await
    }

    async fn diff_stats(&self, repo: &str, numbers: &[u64]) -> Result<Vec<(u64, DiffStat)>> {
        pull_requests::diff_stats(&self.client, repo, numbers).await
    }

    async fn row_statuses(&self, repo: &str, numbers: &[u64]) -> Result<Vec<(u64, RowStatus)>> {
        pull_requests::statuses::row_statuses(&self.client, repo, numbers).await
    }

    async fn merge_policy(&self, repo: &str, target_branch: &str) -> Result<MergePolicy> {
        merge_policy::policy(&self.client, repo, target_branch, &self.capabilities.merge_strategies)
            .await
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
        auto_merge::enable(&self.client, repo, number, options).await
    }

    async fn cancel_auto_complete(&self, repo: &str, number: u64) -> Result<()> {
        auto_merge::disable(&self.client, repo, number).await
    }

    async fn close(&self, repo: &str, number: u64) -> Result<()> {
        lifecycle::set_state(&self.client, repo, number, "closed").await
    }

    async fn reopen(&self, repo: &str, number: u64) -> Result<()> {
        lifecycle::set_state(&self.client, repo, number, "open").await
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
        threads::threads(&self.client, repo, number).await
    }

    async fn create_thread(
        &self,
        repo: &str,
        number: u64,
        thread: &NewThread,
    ) -> Result<ReviewThread> {
        thread_writes::create(&self.client, repo, number, thread).await
    }

    async fn reply(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        body: &str,
    ) -> Result<ThreadComment> {
        thread_writes::reply(&self.client, repo, number, thread_id, body).await
    }

    async fn set_thread_status(
        &self,
        _repo: &str,
        _number: u64,
        thread_id: &str,
        status: ThreadStatus,
    ) -> Result<()> {
        thread_writes::set_status(&self.client, thread_id, status).await
    }

    fn comment_actions(&self, thread: &ReviewThread) -> CommentActions {
        comment_edits::actions(thread)
    }

    async fn edit_comment(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<ThreadComment> {
        comment_edits::edit(&self.client, repo, number, thread_id, comment_id, body).await
    }

    async fn delete_comment(
        &self,
        repo: &str,
        _number: u64,
        thread_id: &str,
        comment_id: &str,
    ) -> Result<()> {
        comment_edits::delete(&self.client, repo, thread_id, comment_id).await
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
        pipelines::runs(&self.client, &self.workflows, repo, number).await
    }

    async fn pipeline_jobs(
        &self,
        repo: &str,
        _number: u64,
        run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        pipelines::jobs(&self.client, &self.workflows, repo, run_id).await
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
        setup::inspect(&self.client).await
    }

    async fn discover_scopes(&self) -> Result<Vec<ScopeCandidate>> {
        setup::discover(&self.client).await
    }

    fn explain_denial(&self, error: &ProviderError) -> Option<AccessProblem> {
        setup::explain(self.client.connection(), error)
    }

    fn partial_access(&self) -> Option<AccessProblem> {
        sso::problem(self.client.connection(), &self.client.partial())
    }

    async fn fetch_image(
        &self,
        repo: &str,
        url: &str,
        limit: u64,
        known: &review_core::http::image::Validators,
    ) -> Result<review_core::http::image::Body> {
        images::fetch(&self.client, repo, url, limit, known).await
    }
}

#[cfg(test)]
mod tests {
    use super::GitHubProvider;
    use review_core::{Connection, Provider, ProviderError, ProviderKind, ReviewVote};

    fn anonymous() -> GitHubProvider {
        let connection = Connection {
            id: "gh".to_owned(),
            name: "GitHub".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: "https://github.com".to_owned(),
            organizations: Vec::new(),
            token: None,
        };
        GitHubProvider::new(connection, reqwest::Client::new())
    }

    #[tokio::test]
    async fn writes_without_token_are_unsupported_before_any_request() {
        let provider = anonymous();
        let results = [
            provider.close("cli/cli", 1).await,
            provider.set_labels("cli/cli", 1, &[]).await,
            provider.set_vote("cli/cli", 1, ReviewVote::Approved, None).await,
            provider.set_draft("cli/cli", 1, true).await,
        ];
        for result in results {
            assert!(matches!(result, Err(ProviderError::Unsupported(_))), "{result:?}");
        }
        assert!(matches!(provider.current_user().await, Err(ProviderError::Unsupported(_))));
        assert!(matches!(
            provider.list_repositories(None).await,
            Err(ProviderError::Unsupported(_))
        ));
    }
}
