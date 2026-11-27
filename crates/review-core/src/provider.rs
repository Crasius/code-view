use async_trait::async_trait;
use tokio::io::AsyncWrite;

use crate::error::{ProviderError, Result};
use crate::http;
use crate::model::{
    DiffStat, GitRemote, ItemRef, Label, LinkedItem, MergeOptions, MergePolicy, MergeStrategy,
    NewThread, PipelineJob, PipelineRun, PullRequestDetail, PullRequestQuery, PullRequestRevision,
    PullRequestScope, PullRequestSummary, RepositoryInfo, RerunAction, ReviewThread, ReviewVote,
    RowStatus, RunningLogs, StateFilter, ThreadComment, ThreadStatus, Unreported, UserIdentity,
};
use crate::setup::{self, AccessProblem, AccessReport, ScopeCandidate};

/// What a platform supports beyond the shared minimum, so the UI only offers what works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// Votes the current user can cast. Contains [`ReviewVote::None`] when "reset my vote" works.
    pub votes: Vec<ReviewVote>,
    /// Merge strategies the platform API accepts; what the merge button offers when
    /// [`Provider::merge_policy`] is not reported.
    pub merge_strategies: Vec<MergeStrategy>,
    /// True when [`Provider::available_labels`] returns a repository label catalog;
    /// false when labels are free text (Azure DevOps tags).
    pub has_label_catalog: bool,
    /// What [`Provider::download_log`] serves of a log whose job still runs.
    pub running_logs: RunningLogs,
}

/// What the platform's API lets the signed-in user do with their own comments in one thread
/// ([`Provider::comment_actions`]). Comments of other people never get these actions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommentActions {
    pub can_edit: bool,
    pub can_delete: bool,
}

impl CommentActions {
    pub const NONE: Self = Self { can_edit: false, can_delete: false };
    pub const EDIT_AND_DELETE: Self = Self { can_edit: true, can_delete: true };
    /// A submitted GitHub review's body: it can be updated, never deleted.
    pub const EDIT_ONLY: Self = Self { can_edit: true, can_delete: false };

    pub fn is_none(self) -> bool {
        !self.can_edit && !self.can_delete
    }
}

/// One platform bound to one [`crate::Connection`].
///
/// `repo` is always a [`RepositoryInfo::key`]; `number` the per-repository pull request number.
/// Every write needs a token; implementations return [`crate::ProviderError::Unsupported`] without one.
/// Read calls work anonymously where the platform allows it (public repositories).
#[async_trait]
pub trait Provider: Send + Sync {
    fn capabilities(&self) -> &Capabilities;

    // --- account and repositories -------------------------------------------------------

    async fn current_user(&self) -> Result<UserIdentity>;

    /// Repositories within the connection's organizations (or all the token sees),
    /// filtered by name when `search` is set. At most 100.
    async fn list_repositories(&self, search: Option<&str>) -> Result<Vec<RepositoryInfo>>;

    async fn git_remote(&self, repo: &str) -> Result<GitRemote>;

    // --- pull requests ------------------------------------------------------------------

    /// Newest first.
    async fn list_pull_requests(&self, query: &PullRequestQuery)
    -> Result<Vec<PullRequestSummary>>;

    /// True when [`Self::list_repositories_pull_requests`] lists several repositories in fewer
    /// requests than one per repository (GitHub's search with a token). Where it is false, callers
    /// list each repository with [`PullRequestQuery::in_repository`] instead.
    fn lists_repositories_together(&self) -> bool {
        false
    }

    /// Pull requests in `state` of every repository in `repositories` ([`RepositoryInfo::key`]s)
    /// that `role` keeps (see [`PullRequestQuery::in_repository`]), most recently updated first, at
    /// most `top` per repository. The default asks per repository.
    async fn list_repositories_pull_requests(
        &self,
        repositories: &[String],
        role: PullRequestScope,
        state: StateFilter,
        top: usize,
    ) -> Result<Vec<PullRequestSummary>> {
        let mut rows = Vec::new();
        for repository in repositories {
            let query = PullRequestQuery::in_repository(repository.clone(), role, state, top);
            rows.extend(self.list_pull_requests(&query).await?);
        }
        rows.sort_by_key(|row| std::cmp::Reverse(row.last_activity()));
        Ok(rows)
    }

    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail>;

    /// Lines and files the pull requests `numbers` of `repo` change, for list rows whose listing
    /// left them out, in as few requests as the platform allows: one for all of them where it can
    /// batch (GitLab GraphQL), else one or two per pull request. Parts the platform does not report
    /// stay `None` (Azure DevOps counts no lines); pull requests it does not know are left out.
    /// The default reports nothing.
    async fn diff_stats(&self, _repo: &str, _numbers: &[u64]) -> Result<Vec<(u64, DiffStat)>> {
        Ok(Vec::new())
    }

    /// Checks, branch policies and (where the listing had none) reviewer votes of the list rows
    /// `numbers` of `repo`, in as few requests as the platform allows: none where the latest listing
    /// brought them (GitHub's search), one for all of them where the platform batches (GraphQL),
    /// else one per pull request. Pull requests the platform does not report are left out. The
    /// default reports nothing.
    async fn row_statuses(&self, _repo: &str, _numbers: &[u64]) -> Result<Vec<(u64, RowStatus)>> {
        Ok(Vec::new())
    }

    /// Which strategies merging into `target_branch` (a branch name, not a ref) allows and which
    /// one the platform preselects. Missing permissions are not an error: they come back as
    /// [`crate::PolicySource::NotReported`] with everything the API accepts. The default reports nothing.
    async fn merge_policy(&self, _repo: &str, _target_branch: &str) -> Result<MergePolicy> {
        Ok(MergePolicy::not_reported(
            &self.capabilities().merge_strategies,
            Unreported::Unsupported,
        ))
    }

    async fn merge(&self, repo: &str, number: u64, options: &MergeOptions) -> Result<()>;

    /// Turns on the platform's auto-complete (Azure DevOps "Set auto-complete", GitHub "Enable
    /// auto-merge", GitLab "Set to auto-merge"): the platform completes the pull request with
    /// `options` once its policies pass. [`MergeOptions::head_sha`] guards it as it guards
    /// [`Self::merge`] where the platform takes a guard. Needs a token. The default refuses.
    async fn set_auto_complete(
        &self,
        _repo: &str,
        _number: u64,
        _options: &MergeOptions,
    ) -> Result<()> {
        Err(ProviderError::Unsupported(
            "Auto-complete is not supported on this platform.".to_owned(),
        ))
    }

    /// Turns the auto-complete [`Self::set_auto_complete`] set off again, whoever set it. Needs a
    /// token. The default refuses.
    async fn cancel_auto_complete(&self, _repo: &str, _number: u64) -> Result<()> {
        Err(ProviderError::Unsupported(
            "Auto-complete is not supported on this platform.".to_owned(),
        ))
    }

    /// "Abandon" on Azure DevOps.
    async fn close(&self, repo: &str, number: u64) -> Result<()>;

    async fn reopen(&self, repo: &str, number: u64) -> Result<()>;

    async fn set_draft(&self, repo: &str, number: u64, is_draft: bool) -> Result<()>;

    async fn update_title_description(
        &self,
        repo: &str,
        number: u64,
        title: &str,
        description: &str,
    ) -> Result<()>;

    // --- review -------------------------------------------------------------------------

    /// Casts the current user's vote. `comment` becomes the review body where the platform has one
    /// (GitHub reviews), otherwise a general comment when not empty.
    async fn set_vote(
        &self,
        repo: &str,
        number: u64,
        vote: ReviewVote,
        comment: Option<&str>,
    ) -> Result<()>;

    /// `user` is a login, username or e-mail as the user types it; the provider resolves it.
    async fn add_reviewer(&self, repo: &str, number: u64, user: &str) -> Result<()>;

    /// `reviewer_id` is a [`UserIdentity::id`] from [`PullRequestSummary::reviewers`].
    async fn remove_reviewer(&self, repo: &str, number: u64, reviewer_id: &str) -> Result<()>;

    // --- threads ------------------------------------------------------------------------

    /// All conversations, oldest first, comments inside oldest first.
    async fn threads(&self, repo: &str, number: u64) -> Result<Vec<ReviewThread>>;

    async fn create_thread(
        &self,
        repo: &str,
        number: u64,
        thread: &NewThread,
    ) -> Result<ReviewThread>;

    async fn reply(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        body: &str,
    ) -> Result<ThreadComment>;

    async fn set_thread_status(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        status: ThreadStatus,
    ) -> Result<()>;

    /// What [`Self::edit_comment`] and [`Self::delete_comment`] accept for the user's own comments
    /// of `thread`, by the kind of conversation it is. System notices get nothing. Callers check
    /// the authorship themselves. The default offers nothing.
    fn comment_actions(&self, _thread: &ReviewThread) -> CommentActions {
        CommentActions::NONE
    }

    /// What to append to the pull request's page URL to reach `thread`: a fragment
    /// (`#note_7`) or a query (`?discussionId=7`). `None` when the platform has no such link.
    fn thread_page_suffix(&self, _thread: &ReviewThread) -> Option<String> {
        None
    }

    /// Replaces the body of comment `comment_id` in thread `thread_id`; returns the comment as the
    /// platform stored it. Needs a token. The default refuses.
    async fn edit_comment(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _comment_id: &str,
        _body: &str,
    ) -> Result<ThreadComment> {
        Err(ProviderError::Unsupported("Comments cannot be edited on this platform.".to_owned()))
    }

    /// Deletes comment `comment_id` in thread `thread_id`. A thread whose last comment goes is gone
    /// on the platform too. Needs a token. The default refuses.
    async fn delete_comment(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _comment_id: &str,
    ) -> Result<()> {
        Err(ProviderError::Unsupported("Comments cannot be deleted on this platform.".to_owned()))
    }

    // --- live updates -------------------------------------------------------------------

    /// A cheap fingerprint of the pull request, polled while its page is open to notice pushes,
    /// comments, votes and status changes. Pipelines are not part of it. The default reads the
    /// detail and the threads; platforms override it with their cheapest documented request.
    async fn revision(&self, repo: &str, number: u64) -> Result<PullRequestRevision> {
        let (detail, threads) =
            tokio::try_join!(self.pull_request(repo, number), self.threads(repo, number))?;
        Ok(PullRequestRevision::of(&detail, &threads))
    }

    // --- linked items -------------------------------------------------------------------

    /// Title and state of referenced pull requests, issues and work items, in as few requests as
    /// the platform allows. Items the platform does not know (or the token cannot see) are left
    /// out; each answer carries the [`ItemRef`] it was asked for.
    async fn linked_items(&self, _references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
        Ok(Vec::new())
    }

    /// Items the platform itself links to the pull request, beyond what its texts mention
    /// (Azure DevOps work items). Empty where the platform has no such list.
    async fn linked_references(&self, _repo: &str, _number: u64) -> Result<Vec<ItemRef>> {
        Ok(Vec::new())
    }

    // --- labels -------------------------------------------------------------------------

    async fn available_labels(&self, repo: &str) -> Result<Vec<Label>>;

    /// Replaces the pull request's labels with exactly `labels` (by name).
    async fn set_labels(&self, repo: &str, number: u64, labels: &[String]) -> Result<()>;

    // --- pipelines ----------------------------------------------------------------------

    /// Runs and status checks of the pull request, from list calls only (no jobs): those of its
    /// head commit marked [`PipelineRun::is_head`], then older runs where the platform lists them.
    async fn pipeline_runs(&self, repo: &str, number: u64) -> Result<Vec<PipelineRun>>;

    /// Stage/job/step tree of one run.
    async fn pipeline_jobs(
        &self,
        repo: &str,
        number: u64,
        run_id: &str,
    ) -> Result<Vec<PipelineJob>>;

    /// The stage or job of an earlier attempt listed as [`crate::AttemptSource::Lazy`] in the run
    /// `run_id`, with its children and their logs (which [`Self::download_log`] serves). Loaded
    /// only when the user opens that attempt. Platforms that report earlier attempts with the run
    /// never list lazy ones, so the default refuses.
    async fn pipeline_attempt(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
        target: &str,
    ) -> Result<PipelineJob> {
        Err(ProviderError::Unsupported(format!(
            "Attempt '{target}' cannot be loaded on this platform."
        )))
    }

    /// Streams the raw log text into `sink` without buffering it whole.
    async fn download_log(
        &self,
        repo: &str,
        number: u64,
        log_id: &str,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()>;

    /// Starts a rerun offered in [`PipelineRun::reruns`] or [`PipelineJob::reruns`] of the run
    /// `run_id`. Needs a token. Platforms only offer what their API documents, so the default
    /// refuses everything.
    async fn rerun(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
        action: &RerunAction,
    ) -> Result<()> {
        Err(ProviderError::Unsupported(format!(
            "{:?} reruns are not supported on this platform.",
            action.kind
        )))
    }

    // --- embedded images ----------------------------------------------------------------

    /// An image a description, comment or rendered file of `repo` embeds: `url` is absolute (http
    /// or https) as written there, the body at most `limit` bytes. Credentials go along only to the
    /// connection's own hosts and never across a redirect to another host ([`http::image`]);
    /// platforms that serve private attachments under short-lived signed addresses sign `url` right
    /// before loading it. With the `known` validators of a kept copy, an unchanged image comes back
    /// as [`http::image::Body::NotModified`]. The default asks every host anonymously.
    async fn fetch_image(
        &self,
        _repo: &str,
        url: &str,
        limit: u64,
        known: &http::image::Validators,
    ) -> Result<http::image::Body> {
        let accept = http::image::IMAGE_ACCEPT;
        http::image::fetch(url, accept, limit, known, &http::image::Anonymous).await
    }

    // --- setup --------------------------------------------------------------------------

    /// Who the token signs in as and which features it unlocks, found with read-only probes.
    /// Never writes; features only a write could prove come back as [`setup::Access::Unverified`].
    async fn inspect_access(&self) -> Result<AccessReport> {
        let user = self.current_user().await?;
        Ok(AccessReport::unverified(user, "Personal access token"))
    }

    /// Organizations, groups or collections the token reaches, for the user to pick from.
    async fn discover_scopes(&self) -> Result<Vec<ScopeCandidate>> {
        Ok(Vec::new())
    }

    /// Explains a 401/403 in terms of the token (which permission is missing), else `None`.
    fn explain_denial(&self, error: &ProviderError) -> Option<AccessProblem> {
        setup::generic_denial(error)
    }

    /// What the latest successful [`Self::list_pull_requests`] silently left out because the token
    /// lacks access (GitHub drops organizations behind SAML single sign-on from search results),
    /// with the actions that fix it. `None` when the listing was complete.
    fn partial_access(&self) -> Option<AccessProblem> {
        None
    }
}
