//! Token problems seen at runtime. Every provider the app hands out is wrapped in
//! `WatchedProvider`, which records 401/403 answers, explained by the provider, per connection.
//! Listings that succeeded but left pull requests out for lack of access (SAML single sign-on) are
//! recorded the same way. The access banner turns them into a "Fix token" action and the
//! problem's own actions ("Authorize for acme"); saving the connection clears them.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use async_trait::async_trait;
use review_core::provider::CommentActions;
use review_core::{
    AccessProblem, AccessReport, Capabilities, DiffStat, GitRemote, ItemRef, Label, LinkedItem,
    MergeOptions, MergePolicy, NewThread, PipelineJob, PipelineRun, Provider, ProviderError,
    PullRequestDetail, PullRequestQuery, PullRequestRevision, PullRequestScope, PullRequestSummary,
    RepositoryInfo, RerunAction, Result, ReviewThread, ReviewVote, RowStatus, ScopeCandidate,
    StateFilter, ThreadComment, ThreadStatus, UserIdentity, http,
};
use tokio::io::AsyncWrite;

/// The latest explained denial per connection id, shared with the background tasks.
#[derive(Clone, Default)]
pub struct AccessIssues {
    by_connection: Arc<Mutex<HashMap<String, AccessProblem>>>,
}

impl AccessIssues {
    pub fn record(&self, connection_id: &str, problem: AccessProblem) {
        self.lock().insert(connection_id.to_owned(), problem);
    }

    pub fn get(&self, connection_id: &str) -> Option<AccessProblem> {
        self.lock().get(connection_id).cloned()
    }

    /// `(connection id, problem)`, sorted by connection id for a stable banner order.
    pub fn all(&self) -> Vec<(String, AccessProblem)> {
        let mut issues: Vec<_> =
            self.lock().iter().map(|(id, problem)| (id.clone(), problem.clone())).collect();
        issues.sort_by(|left, right| left.0.cmp(&right.0));
        issues
    }

    pub fn clear(&self, connection_id: &str) {
        self.lock().remove(connection_id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, AccessProblem>> {
        self.by_connection.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Forwards every call to the connection's current provider and records denials it can explain.
/// Everything holding it (open pull request pages, their loads, caches) reaches the connection's
/// newest settings: [`Self::replace`] swaps the provider underneath, and each call takes the
/// provider of the moment.
pub(crate) struct WatchedProvider {
    inner: RwLock<Arc<dyn Provider>>,
    /// The platform's, fixed for the handle's lifetime (a connection never changes its platform).
    capabilities: Capabilities,
    connection_id: String,
    issues: AccessIssues,
}

impl WatchedProvider {
    pub(crate) fn new(
        inner: Arc<dyn Provider>,
        connection_id: String,
        issues: AccessIssues,
    ) -> Self {
        let capabilities = inner.capabilities().clone();
        Self { inner: RwLock::new(inner), capabilities, connection_id, issues }
    }

    /// Calls from now on go to `inner` (built for the connection's new settings); calls already
    /// running finish with the provider they started with.
    pub(crate) fn replace(&self, inner: Arc<dyn Provider>) {
        *self.inner.write().unwrap_or_else(PoisonError::into_inner) = inner;
    }

    fn current(&self) -> Arc<dyn Provider> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Runs one provider call with its requests counted for this connection, and records denials.
    /// `call` was made with the provider of the moment; a denial that arrives after the
    /// connection's settings changed ([`Self::replace`]) is about the old token and not recorded,
    /// so the banner does not ask to fix a token that was just fixed.
    async fn watch<T>(&self, call: impl Future<Output = Result<T>>) -> Result<T> {
        let started = self.current();
        let result = http::for_connection(&self.connection_id, call).await;
        if let Err(error) = &result
            && self.is_current(&started)
            && let Some(problem) = started.explain_denial(error)
        {
            self.issues.record(&self.connection_id, problem);
        }
        result
    }

    /// `provider` still serves the connection: its settings did not change since it was taken.
    fn is_current(&self, provider: &Arc<dyn Provider>) -> bool {
        Arc::ptr_eq(provider, &self.current())
    }
}

#[async_trait]
impl Provider for WatchedProvider {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        self.watch(self.current().current_user()).await
    }

    async fn list_repositories(&self, search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        self.watch(self.current().list_repositories(search)).await
    }

    async fn git_remote(&self, repo: &str) -> Result<GitRemote> {
        self.watch(self.current().git_remote(repo)).await
    }

    /// A complete answer can still leave pull requests out (GitHub drops organizations behind SAML
    /// single sign-on from searches); that is recorded like a denial, with the actions that fix it.
    async fn list_pull_requests(
        &self,
        query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        let provider = self.current();
        let result = self.watch(provider.list_pull_requests(query)).await;
        if result.is_ok()
            && self.is_current(&provider)
            && let Some(problem) = provider.partial_access()
        {
            self.issues.record(&self.connection_id, problem);
        }
        result
    }

    fn lists_repositories_together(&self) -> bool {
        self.current().lists_repositories_together()
    }

    async fn list_repositories_pull_requests(
        &self,
        repositories: &[String],
        role: PullRequestScope,
        state: StateFilter,
        top: usize,
    ) -> Result<Vec<PullRequestSummary>> {
        self.watch(self.current().list_repositories_pull_requests(repositories, role, state, top))
            .await
    }

    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        self.watch(self.current().pull_request(repo, number)).await
    }

    async fn diff_stats(&self, repo: &str, numbers: &[u64]) -> Result<Vec<(u64, DiffStat)>> {
        self.watch(self.current().diff_stats(repo, numbers)).await
    }

    /// Counted, but a denial is not recorded: the row's checks indicator tells it where it applies
    /// (a policy the token may not read says nothing about the rest of the connection).
    async fn row_statuses(&self, repo: &str, numbers: &[u64]) -> Result<Vec<(u64, RowStatus)>> {
        http::for_connection(&self.connection_id, self.current().row_statuses(repo, numbers)).await
    }

    async fn merge_policy(&self, repo: &str, target_branch: &str) -> Result<MergePolicy> {
        self.watch(self.current().merge_policy(repo, target_branch)).await
    }

    async fn merge(&self, repo: &str, number: u64, options: &MergeOptions) -> Result<()> {
        self.watch(self.current().merge(repo, number, options)).await
    }

    async fn set_auto_complete(
        &self,
        repo: &str,
        number: u64,
        options: &MergeOptions,
    ) -> Result<()> {
        self.watch(self.current().set_auto_complete(repo, number, options)).await
    }

    async fn cancel_auto_complete(&self, repo: &str, number: u64) -> Result<()> {
        self.watch(self.current().cancel_auto_complete(repo, number)).await
    }

    async fn close(&self, repo: &str, number: u64) -> Result<()> {
        self.watch(self.current().close(repo, number)).await
    }

    async fn reopen(&self, repo: &str, number: u64) -> Result<()> {
        self.watch(self.current().reopen(repo, number)).await
    }

    async fn set_draft(&self, repo: &str, number: u64, is_draft: bool) -> Result<()> {
        self.watch(self.current().set_draft(repo, number, is_draft)).await
    }

    async fn update_title_description(
        &self,
        repo: &str,
        number: u64,
        title: &str,
        description: &str,
    ) -> Result<()> {
        self.watch(self.current().update_title_description(repo, number, title, description)).await
    }

    async fn set_vote(
        &self,
        repo: &str,
        number: u64,
        vote: ReviewVote,
        comment: Option<&str>,
    ) -> Result<()> {
        self.watch(self.current().set_vote(repo, number, vote, comment)).await
    }

    async fn add_reviewer(&self, repo: &str, number: u64, user: &str) -> Result<()> {
        self.watch(self.current().add_reviewer(repo, number, user)).await
    }

    async fn remove_reviewer(&self, repo: &str, number: u64, reviewer_id: &str) -> Result<()> {
        self.watch(self.current().remove_reviewer(repo, number, reviewer_id)).await
    }

    async fn threads(&self, repo: &str, number: u64) -> Result<Vec<ReviewThread>> {
        self.watch(self.current().threads(repo, number)).await
    }

    async fn create_thread(
        &self,
        repo: &str,
        number: u64,
        thread: &NewThread,
    ) -> Result<ReviewThread> {
        self.watch(self.current().create_thread(repo, number, thread)).await
    }

    async fn reply(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        body: &str,
    ) -> Result<ThreadComment> {
        self.watch(self.current().reply(repo, number, thread_id, body)).await
    }

    async fn set_thread_status(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        status: ThreadStatus,
    ) -> Result<()> {
        self.watch(self.current().set_thread_status(repo, number, thread_id, status)).await
    }

    fn comment_actions(&self, thread: &ReviewThread) -> CommentActions {
        self.current().comment_actions(thread)
    }

    fn thread_page_suffix(&self, thread: &ReviewThread) -> Option<String> {
        self.current().thread_page_suffix(thread)
    }

    async fn edit_comment(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<ThreadComment> {
        self.watch(self.current().edit_comment(repo, number, thread_id, comment_id, body)).await
    }

    async fn delete_comment(
        &self,
        repo: &str,
        number: u64,
        thread_id: &str,
        comment_id: &str,
    ) -> Result<()> {
        self.watch(self.current().delete_comment(repo, number, thread_id, comment_id)).await
    }

    async fn revision(&self, repo: &str, number: u64) -> Result<PullRequestRevision> {
        self.watch(self.current().revision(repo, number)).await
    }

    async fn linked_items(&self, references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
        self.watch(self.current().linked_items(references)).await
    }

    async fn linked_references(&self, repo: &str, number: u64) -> Result<Vec<ItemRef>> {
        self.watch(self.current().linked_references(repo, number)).await
    }

    async fn available_labels(&self, repo: &str) -> Result<Vec<Label>> {
        self.watch(self.current().available_labels(repo)).await
    }

    async fn set_labels(&self, repo: &str, number: u64, labels: &[String]) -> Result<()> {
        self.watch(self.current().set_labels(repo, number, labels)).await
    }

    async fn pipeline_runs(&self, repo: &str, number: u64) -> Result<Vec<PipelineRun>> {
        self.watch(self.current().pipeline_runs(repo, number)).await
    }

    async fn pipeline_jobs(
        &self,
        repo: &str,
        number: u64,
        run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        self.watch(self.current().pipeline_jobs(repo, number, run_id)).await
    }

    async fn pipeline_attempt(
        &self,
        repo: &str,
        number: u64,
        run_id: &str,
        target: &str,
    ) -> Result<PipelineJob> {
        self.watch(self.current().pipeline_attempt(repo, number, run_id, target)).await
    }

    async fn download_log(
        &self,
        repo: &str,
        number: u64,
        log_id: &str,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()> {
        self.watch(self.current().download_log(repo, number, log_id, sink)).await
    }

    async fn rerun(
        &self,
        repo: &str,
        number: u64,
        run_id: &str,
        action: &RerunAction,
    ) -> Result<()> {
        self.watch(self.current().rerun(repo, number, run_id, action)).await
    }

    async fn inspect_access(&self) -> Result<AccessReport> {
        http::for_connection(&self.connection_id, self.current().inspect_access()).await
    }

    async fn discover_scopes(&self) -> Result<Vec<ScopeCandidate>> {
        http::for_connection(&self.connection_id, self.current().discover_scopes()).await
    }

    /// Counted, but a refused image is no token problem: it never raises the access banner.
    async fn fetch_image(
        &self,
        repo: &str,
        url: &str,
        limit: u64,
        known: &http::image::Validators,
    ) -> Result<http::image::Body> {
        let provider = self.current();
        let fetch = provider.fetch_image(repo, url, limit, known);
        http::for_connection(&self.connection_id, fetch).await
    }

    fn explain_denial(&self, error: &ProviderError) -> Option<AccessProblem> {
        self.current().explain_denial(error)
    }

    fn partial_access(&self) -> Option<AccessProblem> {
        self.current().partial_access()
    }
}

#[cfg(test)]
mod tests {
    use cv_testkit::samples;
    use provider_github::GitHubProvider;
    use review_core::{Connection, ProviderKind, RerunKind};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    /// A GitHub provider on `server`, signed in with `token`, for the connection id "watched".
    fn github(server: &MockServer, token: &str, organizations: &[&str]) -> Arc<dyn Provider> {
        let connection = Connection {
            organizations: organizations
                .iter()
                .map(|&organization| organization.to_owned())
                .collect(),
            token: Some(token.to_owned()),
            ..samples::connection("watched", ProviderKind::GitHub, &server.uri())
        };
        Arc::new(GitHubProvider::new(connection, reqwest::Client::new()))
    }

    /// Methods with a default body compile even when the wrapper forgets them, and then silently
    /// answer with the default instead of the platform: a rerun has to reach the provider.
    #[tokio::test]
    async fn forwards_reruns_to_the_platform() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/repos/cli/cli/actions/jobs/501/rerun"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let watched = WatchedProvider::new(
            github(&server, "test-token", &[]),
            "watched".to_owned(),
            AccessIssues::default(),
        );
        let action = RerunAction::new(RerunKind::Job, "501");
        watched.rerun("cli/cli", 7, "run:11", &action).await.expect("the rerun reaches GitHub");
    }

    /// A listing GitHub answered without the organizations behind SAML single sign-on succeeds, and
    /// the banner learns which organization to authorize.
    #[tokio::test]
    async fn records_what_a_listing_left_out() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/users/acme"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "login": "acme", "type": "Organization" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/organizations/4242"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "login": "secure-corp" })),
            )
            .mount(&server)
            .await;
        let empty = serde_json::json!({ "data": { "search": { "pageInfo": { "hasNextPage": false }, "nodes": [] } } });
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-github-sso", "partial-results; organizations=4242")
                    .set_body_json(empty),
            )
            .mount(&server)
            .await;
        let issues = AccessIssues::default();
        let inner = github(&server, "ghp_test-token", &["acme"]);
        let watched = WatchedProvider::new(inner, "watched".to_owned(), issues.clone());
        let query = PullRequestQuery {
            scope: review_core::PullRequestScope::Visible,
            state: review_core::StateFilter::Open,
            repository_key: None,
            top: 50,
        };
        watched.list_pull_requests(&query).await.expect("the listing itself succeeds");
        let problem = issues.get("watched").expect("recorded for the banner");
        assert_eq!(problem.actions.len(), 1);
        assert_eq!(problem.actions[0].label, "Authorize for secure-corp");
        assert_eq!(problem.actions[0].url, format!("{}/settings/tokens", server.uri()));
    }

    /// A request sent with the old token that is refused after the connection was saved with a new
    /// one says nothing about the new token: the banner stays away. Without the save it shows.
    #[tokio::test]
    async fn a_denial_of_the_replaced_token_is_not_recorded() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
        let issues = AccessIssues::default();
        let watched = WatchedProvider::new(
            github(&server, "old-token", &[]),
            "watched".to_owned(),
            issues.clone(),
        );
        // The first branch takes the old provider before the second replaces it.
        let (refused, ()) = tokio::join!(biased; watched.current_user(), async {
            watched.replace(github(&server, "new-token", &[]));
        });
        assert!(refused.is_err(), "the old token's request was refused");
        assert!(issues.get("watched").is_none(), "{:?}", issues.get("watched"));

        watched.current_user().await.expect_err("refused again");
        assert_eq!(issues.get("watched").map(|problem| problem.status), Some(401));
    }

    /// The live-update probe must be the platform's cheap request, not the default that reads the
    /// detail and every thread (a GraphQL query on GitHub) every 30 seconds.
    #[tokio::test]
    async fn forwards_revisions_to_the_platform() {
        let server = MockServer::start().await;
        let pull = serde_json::json!({
            "number": 7, "title": "Fix", "state": "open", "html_url": "https://github.example/cli/cli/pull/7",
            "created_at": "2026-01-01T10:00:00Z", "updated_at": "2026-01-02T10:00:00Z",
            "head": { "ref": "feature", "sha": "head1" }, "base": { "ref": "main", "sha": "base1" }
        });
        Mock::given(method("GET"))
            .and(path("/api/v3/repos/cli/cli/pulls/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pull))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/repos/cli/cli/pulls/7/reviews"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let watched = WatchedProvider::new(
            github(&server, "test-token", &[]),
            "watched".to_owned(),
            AccessIssues::default(),
        );
        let revision = watched.revision("cli/cli", 7).await.expect("the probe reaches GitHub");
        assert_eq!(revision.head_sha, "head1");
    }

    /// Without the forward every thread answers the default `None`, and "Copy link" disappears.
    #[tokio::test]
    async fn forwards_thread_links_to_the_platform() {
        let server = MockServer::start().await;
        let watched = WatchedProvider::new(
            github(&server, "test-token", &[]),
            "watched".to_owned(),
            AccessIssues::default(),
        );
        let comment = review_core::ThreadComment {
            id: "5".to_owned(),
            author: samples::author(),
            body: "Looks good".to_owned(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: None,
        };
        let thread = ReviewThread {
            can_resolve: false,
            ..samples::thread(
                "issue-comment:5",
                review_core::ThreadStatus::Active,
                None,
                vec![comment],
            )
        };
        assert_eq!(watched.thread_page_suffix(&thread).as_deref(), Some("#issuecomment-5"));
    }
}
