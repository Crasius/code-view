//! The user's writes against a scripted host that never catches up: every read answers as before
//! the write. Each write must show on the page in the frame it succeeds, in the page cache and in
//! a list's rows, and stay there through the refresh after it and the list's next answer.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use review_core::{
    Capabilities, Connection, Label, MergeOptions, MergeStrategy, ProviderKind, PullRequestDetail,
    PullRequestQuery, PullRequestState, PullRequestSummary, Result, ReviewThread, ReviewVote,
    Reviewer, ThreadStatus, UserIdentity,
};

use cv_lists::pull_request_list::PullRequestListPage;
use cv_pr_session::actions::{ActionRunner, Refresh, Ticket, Write};
use cv_pr_session::lifecycle::LifecycleAction;
use cv_pr_session::session::PullRequestSession;
use cv_runtime::cache::page_cache::pull_request_part;
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use cv_services::writes::Effect;
use cv_testkit::provider::{Script, Scripted};
use cv_testkit::{ScratchDir, poll_until, samples};

mod overview;

const CONNECTION: &str = "scripted";
const REPOSITORY: &str = "acme/app";
const NUMBER: u64 = 7;

/// The page of pull request 7, an overview list that lists it, and the services both use.
struct World {
    services: Services,
    session: PullRequestSession,
    list: PullRequestListPage,
    actions: ActionRunner,
    _dir: ScratchDir,
}

impl World {
    fn new(name: &str) -> Self {
        let dir = ScratchDir::new(&format!("writes-{name}"));
        let mut services = scripted_services(&dir, egui::Context::default());
        let row_key = pull_request_part(&key().storage_key(), "summary");
        services.cache.put(&row_key, summary(), 0.0);
        let mut session = PullRequestSession::open(key(), &mut services).expect("the page opens");
        poll_until("the page's first answers", || {
            session.poll(&mut services);
            !session.detail.is_loading() && !session.threads.is_loading()
        });
        let list = PullRequestListPage::loaded_for_test(CONNECTION, vec![summary()]);
        Self { services, session, list, actions: ActionRunner::default(), _dir: dir }
    }

    /// Starts a write and polls it as the page does; returns what to reload, from the frame the
    /// write succeeded in, before anything is reloaded.
    fn write(&mut self, start: impl FnOnce(&mut Self) -> Option<Ticket>) -> Refresh {
        let ticket = start(self).expect("the write starts");
        let World { services, session, actions, .. } = self;
        let mut refresh = None;
        poll_until("the write", || {
            refresh = actions.poll(services, session);
            assert_ne!(actions.outcome(ticket), Some(false), "the write failed");
            refresh.is_some()
        });
        refresh.expect("the write succeeded")
    }

    /// What `reload_after_write` does with `refresh`, then the answers of a host that has not
    /// caught up.
    fn reload(&mut self, refresh: Refresh) {
        let World { services, session, .. } = self;
        match refresh {
            Refresh::PullRequest => session.refresh(services),
            Refresh::Threads => session.reload_threads(services),
        }
        poll_until("the refresh", || {
            session.poll(services);
            !session.detail.is_loading() && !session.threads.is_loading()
        });
    }

    /// The list's rows after it caught up with the writes, and after its next answer.
    fn listed(&mut self) -> (Vec<PullRequestSummary>, Vec<PullRequestSummary>) {
        let World { services, list, .. } = self;
        list.poll_for_test(services);
        let caught_up = rows(list);
        list.revisit(services);
        poll_until("the list's answer", || !list.poll_for_test(services));
        (caught_up, rows(list))
    }

    fn detail(&self) -> &PullRequestDetail {
        self.session.detail.ready().expect("a detail on screen")
    }

    fn cached_detail(&mut self) -> PullRequestDetail {
        let part = pull_request_part(&key().storage_key(), "detail");
        self.services.cache.get(&part).expect("the detail is remembered")
    }
}

/// Services over the scripted host, with its one connection stored and "me" known.
fn scripted_services(dir: &ScratchDir, ctx: egui::Context) -> Services {
    let paths = super::walkthrough::saved_paths(
        dir,
        &[Connection {
            name: "Scripted".into(),
            token: Some("scripted".into()),
            ..samples::connection(
                CONNECTION,
                ProviderKind::GitHub,
                ProviderKind::GitHub.default_base_url(),
            )
        }],
    );
    let services = Services::new(ctx, paths, |_, _| Arc::new(Scripted(LaggingHost)));
    services.identities.learned(CONNECTION, samples::author());
    services
}

fn rows(list: &PullRequestListPage) -> Vec<PullRequestSummary> {
    list.loaded_matching("").into_iter().map(|row| row.pull_request.clone()).collect()
}

#[test]
fn a_merge_shows_at_once_and_stays_while_the_host_says_open() {
    let mut world = World::new("merge");
    let head_sha = world.detail().head_sha.clone();
    let refresh = world.write(|world| {
        let options = MergeOptions {
            strategy: MergeStrategy::Squash,
            delete_source_branch: false,
            commit_message: None,
            head_sha,
        };
        Write::Merge(options).start(&world.session, &mut world.services, &mut world.actions)
    });
    assert_eq!(world.detail().summary.state, PullRequestState::Merged, "in the same frame");
    assert_eq!(world.cached_detail().summary.state, PullRequestState::Merged);
    let summary_key = pull_request_part(&key().storage_key(), "summary");
    let row: PullRequestSummary = world.services.cache.get(&summary_key).unwrap();
    assert_eq!(row.state, PullRequestState::Merged, "the row the page was opened from");

    world.reload(refresh);
    assert_eq!(world.detail().summary.state, PullRequestState::Merged, "the refresh said open");
    assert_eq!(world.cached_detail().summary.state, PullRequestState::Merged);
    let (caught_up, answered) = world.listed();
    assert!(caught_up.is_empty(), "the open list drops the merged row: {caught_up:?}");
    assert!(answered.is_empty(), "also when the host lists it open again: {answered:?}");
}

#[test]
fn my_vote_shows_at_once_and_stays_while_the_host_has_none() {
    let mut world = World::new("vote");
    let refresh = world.write(|world| {
        let vote = LifecycleAction::Vote { vote: ReviewVote::Approved, comment: None };
        vote.start(&world.session, &world.services, &mut world.actions)
    });
    let my_vote = |reviewers: &[Reviewer]| {
        reviewers
            .iter()
            .find(|reviewer| reviewer.identity.id == samples::author().id)
            .map(|mine| mine.vote)
    };
    assert_eq!(my_vote(&world.detail().summary.reviewers), Some(ReviewVote::Approved));
    world.reload(refresh);
    assert_eq!(my_vote(&world.detail().summary.reviewers), Some(ReviewVote::Approved));
    assert_eq!(my_vote(&world.cached_detail().summary.reviewers), Some(ReviewVote::Approved));
    let (caught_up, answered) = world.listed();
    for rows in [caught_up, answered] {
        assert_eq!(my_vote(&rows[0].reviewers), Some(ReviewVote::Approved), "{rows:?}");
    }
}

#[test]
fn marking_as_draft_shows_at_once_and_stays_while_the_host_says_ready() {
    let mut world = World::new("draft");
    let refresh = world.write(|world| {
        let draft = LifecycleAction::SetDraft(true);
        draft.start(&world.session, &world.services, &mut world.actions)
    });
    assert!(world.detail().summary.is_draft);
    world.reload(refresh);
    assert!(world.detail().summary.is_draft && world.cached_detail().summary.is_draft);
    let (caught_up, answered) = world.listed();
    assert!(caught_up[0].is_draft && answered[0].is_draft, "the row stays, as a draft");
}

#[test]
fn new_labels_show_at_once_and_stay_while_the_host_has_the_old_ones() {
    let mut world = World::new("labels");
    let labels = vec![label("bug"), label("ui")];
    let refresh = world.write(|world| {
        let names: Vec<String> = labels.iter().map(|label| label.name.clone()).collect();
        let provider = world.session.provider.clone();
        let key = key();
        let effect = (&key, Effect::Labels(labels.clone()));
        world.actions.run(&world.services, "Labels updated.", Refresh::PullRequest, effect, {
            async move { provider.set_labels(REPOSITORY, NUMBER, &names).await }
        })
    });
    let names =
        |labels: &[Label]| labels.iter().map(|label| label.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&world.detail().summary.labels), ["bug", "ui"]);
    world.reload(refresh);
    assert_eq!(names(&world.detail().summary.labels), ["bug", "ui"]);
    let (_, answered) = world.listed();
    assert_eq!(names(&answered[0].labels), ["bug", "ui"]);
}

#[test]
fn a_removed_reviewer_goes_at_once_and_stays_gone_while_the_host_lists_them() {
    let mut world = World::new("reviewer");
    let refresh = world.write(|world| {
        let provider = world.session.provider.clone();
        let key = key();
        let effect = (&key, Effect::ReviewerRemoved(bea().id));
        world.actions.run(&world.services, "Reviewer removed.", Refresh::PullRequest, effect, {
            async move { provider.remove_reviewer(REPOSITORY, NUMBER, "3").await }
        })
    });
    let no_one: [Reviewer; 0] = [];
    assert_eq!(world.detail().summary.reviewers, no_one);
    world.reload(refresh);
    assert_eq!(world.detail().summary.reviewers, no_one);
    assert_eq!(world.cached_detail().summary.reviewers, no_one);
    let (caught_up, answered) = world.listed();
    assert!(caught_up[0].reviewers.is_empty() && answered[0].reviewers.is_empty());
}

#[test]
fn a_resolved_thread_shows_at_once_and_stays_while_the_host_says_active() {
    let mut world = World::new("thread");
    let refresh = world.write(|world| {
        let provider = world.session.provider.clone();
        let status = ThreadStatus::Resolved;
        let key = key();
        let effect = (&key, Effect::ThreadStatus { thread_id: "t1".to_owned(), status });
        world.actions.run(&world.services, "Thread resolved.", Refresh::Threads, effect, {
            async move { provider.set_thread_status(REPOSITORY, NUMBER, "t1", status).await }
        })
    });
    let status = |world: &World| world.session.threads.ready().expect("threads")[0].status;
    assert_eq!(status(&world), ThreadStatus::Resolved);
    world.reload(refresh);
    assert_eq!(status(&world), ThreadStatus::Resolved, "the reload still said active");
    let part = pull_request_part(&key().storage_key(), "threads");
    let cached: Vec<ReviewThread> = world.services.cache.get(&part).expect("remembered");
    assert_eq!(cached[0].status, ThreadStatus::Resolved);
}

fn key() -> PullRequestKey {
    PullRequestKey {
        connection_id: CONNECTION.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number: NUMBER,
    }
}

fn person(id: &str, login: &str) -> UserIdentity {
    UserIdentity {
        id: id.to_owned(),
        display_name: login.to_owned(),
        login: Some(login.to_owned()),
        avatar_url: None,
    }
}

fn bea() -> UserIdentity {
    person("3", "bea")
}

fn label(name: &str) -> Label {
    Label { name: name.to_owned(), color: None, description: None }
}

/// The pull request as the host keeps answering, whatever was written.
fn summary() -> PullRequestSummary {
    PullRequestSummary {
        title: "Speed up login".into(),
        author: person("2", "ana"),
        source_branch: "feature".into(),
        labels: vec![label("bug")],
        reviewers: vec![Reviewer {
            identity: bea(),
            vote: ReviewVote::Approved,
            is_required: false,
            is_group: false,
        }],
        ..samples::summary(REPOSITORY, NUMBER)
    }
}

/// Accepts every write and keeps answering every read as before it.
struct LaggingHost;

static CAPABILITIES: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    votes: vec![ReviewVote::Approved, ReviewVote::None],
    merge_strategies: MergeStrategy::ALL.to_vec(),
    has_label_catalog: true,
    running_logs: review_core::RunningLogs::Growing,
});

#[async_trait]
impl Script for LaggingHost {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        Ok(vec![summary()])
    }

    async fn pull_request(&self, _repo: &str, _number: u64) -> Result<PullRequestDetail> {
        Ok(PullRequestDetail { summary: summary(), ..samples::detail(REPOSITORY, NUMBER) })
    }

    async fn merge(&self, _repo: &str, _number: u64, _options: &MergeOptions) -> Result<()> {
        Ok(())
    }

    async fn close(&self, _repo: &str, _number: u64) -> Result<()> {
        Ok(())
    }

    async fn reopen(&self, _repo: &str, _number: u64) -> Result<()> {
        Ok(())
    }

    async fn set_draft(&self, _repo: &str, _number: u64, _is_draft: bool) -> Result<()> {
        Ok(())
    }

    async fn update_title_description(
        &self,
        _repo: &str,
        _number: u64,
        _title: &str,
        _description: &str,
    ) -> Result<()> {
        Ok(())
    }

    async fn set_vote(
        &self,
        _repo: &str,
        _number: u64,
        _vote: ReviewVote,
        _comment: Option<&str>,
    ) -> Result<()> {
        Ok(())
    }

    async fn add_reviewer(&self, _repo: &str, _number: u64, _user: &str) -> Result<()> {
        Ok(())
    }

    async fn remove_reviewer(&self, _repo: &str, _number: u64, _reviewer_id: &str) -> Result<()> {
        Ok(())
    }

    async fn threads(&self, _repo: &str, _number: u64) -> Result<Vec<ReviewThread>> {
        Ok(vec![samples::thread("t1", ThreadStatus::Active, None, Vec::new())])
    }

    async fn set_thread_status(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _status: ThreadStatus,
    ) -> Result<()> {
        Ok(())
    }

    async fn available_labels(&self, _repo: &str) -> Result<Vec<Label>> {
        Ok(vec![label("bug"), label("ui")])
    }

    async fn set_labels(&self, _repo: &str, _number: u64, _labels: &[String]) -> Result<()> {
        Ok(())
    }
}
