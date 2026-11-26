//! The breadcrumb trail shows the way the user came, offline: a scripted GitHub connection lists
//! pull requests 12 and 13, whose descriptions name each other, and one repository. No network,
//! no tokens, no git.
//!
//! Walks the list into a pull request and on to the one its description links (the trail grows,
//! and a pull request already in it cuts it back), back and forward (each page with the trail it
//! had), the crumbs (they go up the trail they show), the repositories page down to a pull request
//! (the structure, as before) and the entries without a way to show: the first page, the search,
//! a pasted link and the rail, which all fall back to the structure.
//! `cargo test -p review-app crumb_trail`

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use egui::{Key, Modifiers, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{
    Capabilities, Connection, GitRemote, ItemKind, ItemRef, Label, LinkedItem, LinkedState,
    MergeOptions, MergeStrategy, NewThread, PipelineJob, PipelineRun, Provider, ProviderError,
    ProviderKind, PullRequestDetail, PullRequestQuery, PullRequestState, PullRequestSummary,
    RepositoryInfo, Result, ReviewThread, ReviewVote, ThreadComment, ThreadStatus, UserIdentity,
};
use review_store::{AppPaths, ConnectionStore};
use tokio::io::AsyncWrite;

use super::ReviewApp;
use super::clicks::Clicks;
use cv_runtime::route::{PullRequestKey, Route};
use cv_testkit::ScratchDir;

const CONNECTION_ID: &str = "crumb-trail";
const CONNECTION_NAME: &str = "Contoso";
const REPOSITORY: &str = "contoso/site";
const LIST: &str = "My pull requests";
/// The title bar's height: crumbs are the links and labels above it.
const BAR_BOTTOM: f32 = 44.0;

#[test]
fn crumb_trail_follows_the_way_the_user_came() {
    let scratch = ScratchDir::new("crumb-trail");
    let mut harness = start(&scratch);

    // The first page after a start: the structure.
    assert_trail(&harness, &[(review_core::APP_NAME, list()), (LIST, list())]);

    // (a) A row of My pull requests opens below the list; its crumb leads back there.
    wait_for(&mut harness, "the list", |harness| has(harness, "12 Twelfth change"));
    harness.click_at(harness.get_by_label("12 Twelfth change").rect());
    wait_for(&mut harness, "pull request 12", |harness| is_on(harness, 12));
    let from_list = [(review_core::APP_NAME, list()), (LIST, list()), (PR_12, pull_request(12))];
    assert_trail(&harness, &from_list);
    assert!(has(&harness, "Back to My pull requests"), "the back arrow names the list");

    // (c) The pull request its description links follows it.
    open_linked(&mut harness, "#13 Thirteenth change (Open)", 13);
    let linked = [
        (review_core::APP_NAME, list()),
        (LIST, list()),
        (PR_12, pull_request(12)),
        (PR_13, pull_request(13)),
    ];
    assert_trail(&harness, &linked);
    assert!(has(&harness, "Back to Pull request #12"), "the back arrow names the pull request");

    // A pull request already in the trail cuts it back to its level: no loop.
    open_linked(&mut harness, "#12 Twelfth change (Open)", 12);
    assert_trail(&harness, &from_list);

    // (d) Back and forward restore each page with the trail it had.
    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowLeft);
    settle(&mut harness);
    assert!(is_on(&harness, 13));
    assert_trail(&harness, &linked);
    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowLeft);
    settle(&mut harness);
    assert!(is_on(&harness, 12));
    assert_trail(&harness, &from_list);
    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowRight);
    settle(&mut harness);
    assert!(is_on(&harness, 13));
    assert_trail(&harness, &linked);

    // A crumb goes up the trail it shows.
    click_crumb(&mut harness, PR_12);
    assert!(is_on(&harness, 12));
    assert_trail(&harness, &from_list);
    click_crumb(&mut harness, LIST);
    assert_eq!(harness.state().route, list());
    assert_trail(&harness, &[(review_core::APP_NAME, list()), (LIST, list())]);

    entries_without_a_way(&mut harness);
    repositories_down_to_a_pull_request(&mut harness);
}

/// (e) The search, a pasted link and the rail have no way to show: the structure.
fn entries_without_a_way(harness: &mut Harness<'_, ReviewApp>) {
    harness.key_press_modifiers(Modifiers::COMMAND, Key::K);
    harness.event(egui::Event::Text("Thirteenth".to_owned()));
    harness.run_steps(3);
    harness.key_press(Key::Enter);
    wait_for(harness, "the searched pull request", |harness| is_on(harness, 13));
    assert_trail(harness, &structure(13));

    // From a page with a way of its own, a pasted link starts over too.
    open_linked(harness, "#12 Twelfth change (Open)", 12);
    assert_eq!(trail(harness).len(), 5, "pull request 12 follows 13's structure");
    harness.event(egui::Event::Paste(format!("https://github.com/{REPOSITORY}/pull/13")));
    wait_for(harness, "the pasted pull request", |harness| is_on(harness, 13));
    assert_trail(harness, &structure(13));

    harness.click_at(harness.get_by_role_and_label(Role::Button, "Repositories").rect());
    settle(harness);
    assert_trail(
        harness,
        &[(review_core::APP_NAME, list()), ("Repositories", Route::Repositories)],
    );
}

/// (b) The repositories page leads down the structure: connection, repository, pull request.
fn repositories_down_to_a_pull_request(harness: &mut Harness<'_, ReviewApp>) {
    wait_for(harness, "the repository row", |harness| repository_row(harness).is_some());
    harness.click_at(repository_row(harness).expect("the repository row").rect());
    wait_for(harness, "the repository's list", |harness| has(harness, "12 Twelfth change"));
    let repository_list = &structure(12)[..3];
    assert_trail(harness, repository_list);
    harness.click_at(harness.get_by_label("12 Twelfth change").rect());
    wait_for(harness, "pull request 12", |harness| is_on(harness, 12));
    assert_trail(harness, &structure(12));

    // Up to the connection and down again: the same structure.
    click_crumb(harness, CONNECTION_NAME);
    assert_trail(harness, &structure(12)[..2]);
    wait_for(harness, "the repository row", |harness| repository_row(harness).is_some());
    harness.click_at(repository_row(harness).expect("the repository row").rect());
    wait_for(harness, "the repository's list", |harness| has(harness, "12 Twelfth change"));
    assert_trail(harness, repository_list);
}

const PR_12: &str = "Pull request #12";
const PR_13: &str = "Pull request #13";

fn list() -> Route {
    Route::MyPullRequests
}

fn key(number: u64) -> PullRequestKey {
    PullRequestKey {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number,
    }
}

fn pull_request(number: u64) -> Route {
    Route::PullRequest(key(number))
}

/// The app's name, the connection, the repository and the pull request.
fn structure(number: u64) -> [(&'static str, Route); 4] {
    let name = if number == 12 { PR_12 } else { PR_13 };
    [
        (review_core::APP_NAME, list()),
        (CONNECTION_NAME, Route::Connection { connection_id: CONNECTION_ID.to_owned() }),
        (
            REPOSITORY,
            Route::RepositoryPullRequests {
                connection_id: CONNECTION_ID.to_owned(),
                repository_key: REPOSITORY.to_owned(),
            },
        ),
        (name, pull_request(number)),
    ]
}

/// The crumbs the title bar shows: each one's text and the route it leads to.
fn trail(harness: &Harness<'_, ReviewApp>) -> Vec<(String, Route)> {
    harness.state().title_bar.crumbs().levels()
}

fn assert_trail(harness: &Harness<'_, ReviewApp>, expected: &[(&str, Route)]) {
    let expected: Vec<(String, Route)> =
        expected.iter().map(|(text, route)| ((*text).to_owned(), route.clone())).collect();
    assert_eq!(trail(harness), expected, "the breadcrumb trail");
    // The crumbs above the page are links (the app's name is the list's page itself there).
    let current = &harness.state().route;
    for (text, route) in &expected[..expected.len() - 1] {
        let is_link = crumb_link(harness, text).is_some();
        assert_eq!(is_link, route != current, "{text} is a link in the title bar unless current");
    }
}

fn crumb_link<'a>(
    harness: &'a Harness<'_, ReviewApp>,
    text: &'a str,
) -> Option<egui_kittest::Node<'a>> {
    harness
        .query_all_by_role_and_label(Role::Link, text)
        .find(|node| node.rect().bottom() <= BAR_BOTTOM)
}

fn click_crumb(harness: &mut Harness<'_, ReviewApp>, text: &str) {
    harness
        .click_at(crumb_link(harness, text).unwrap_or_else(|| panic!("the crumb {text}")).rect());
    settle(harness);
}

/// The repositories page's row of the repository (the crumb of the same name is in the bar).
fn repository_row<'a>(harness: &'a Harness<'_, ReviewApp>) -> Option<egui_kittest::Node<'a>> {
    harness
        .query_all_by_role_and_label(Role::Link, REPOSITORY)
        .find(|node| node.rect().top() > BAR_BOTTOM)
}

/// Clicks the Linked card's row of a pull request and waits for its page.
fn open_linked(harness: &mut Harness<'_, ReviewApp>, row: &str, number: u64) {
    wait_for(harness, row, |harness| harness.query_by_label(row).is_some());
    harness.click_at(harness.get_by_label(row).rect());
    wait_for(harness, "the linked pull request", |harness| is_on(harness, number));
}

fn is_on(harness: &Harness<'_, ReviewApp>, number: u64) -> bool {
    harness.state().route == pull_request(number)
}

fn has(harness: &Harness<'_, ReviewApp>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

fn start(scratch: &ScratchDir) -> Harness<'static, ReviewApp> {
    let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    store
        .upsert(Connection {
            id: CONNECTION_ID.to_owned(),
            name: CONNECTION_NAME.to_owned(),
            kind: ProviderKind::GitHub,
            base_url: ProviderKind::GitHub.default_base_url().to_owned(),
            organizations: vec!["contoso".to_owned()],
            token: None,
        })
        .expect("store the connection");
    Harness::builder()
        .with_size([1280.0, 860.0])
        .with_step_dt(1.0 / 30.0)
        .with_max_steps(1_000_000)
        .build_eframe(move |creation| ReviewApp::new(creation, paths, scripted_provider))
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    harness.run_steps(20);
}

fn wait_for(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    super::action_e2e::wait_for(harness, what, |harness| is_done(harness).then_some(()), |_| {});
    settle(harness);
}

fn scripted_provider(_connection: Connection, _http: reqwest::Client) -> Arc<dyn Provider> {
    Arc::new(ScriptedProvider)
}

struct ScriptedProvider;

impl ScriptedProvider {
    fn unused<T>() -> Result<T> {
        Err(ProviderError::Unsupported("Not part of the crumb trail walkthrough.".to_owned()))
    }
}

fn author() -> UserIdentity {
    UserIdentity {
        id: "1".to_owned(),
        display_name: "Ada Lovelace".to_owned(),
        login: Some("ada".to_owned()),
        avatar_url: None,
    }
}

fn title(number: u64) -> &'static str {
    if number == 12 { "Twelfth change" } else { "Thirteenth change" }
}

fn summary(number: u64) -> PullRequestSummary {
    PullRequestSummary {
        repository_key: REPOSITORY.to_owned(),
        repository_name: "site".to_owned(),
        number,
        title: title(number).to_owned(),
        author: author(),
        state: PullRequestState::Open,
        is_draft: false,
        source_branch: format!("feature/{number}"),
        target_branch: "main".to_owned(),
        created_at: time::macros::datetime!(2026-10-16 09:30 UTC),
        updated_at: None,
        web_url: format!("https://github.com/{REPOSITORY}/pull/{number}"),
        labels: Vec::new(),
        reviewers: Vec::new(),
        checks: None,
        diff_stat: review_core::DiffStat::default(),
        head_sha: None,
        target_sha: None,
    }
}

static CAPABILITIES: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    votes: vec![ReviewVote::Approved, ReviewVote::None],
    merge_strategies: vec![MergeStrategy::Merge],
    has_label_catalog: false,
    running_logs: review_core::RunningLogs::WhenFinished,
});

#[async_trait]
impl Provider for ScriptedProvider {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(author())
    }

    async fn list_repositories(&self, _search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        Ok(vec![RepositoryInfo {
            key: REPOSITORY.to_owned(),
            name: "site".to_owned(),
            description: None,
            web_url: format!("https://github.com/{REPOSITORY}"),
            default_branch: Some("main".to_owned()),
        }])
    }

    async fn git_remote(&self, _repo: &str) -> Result<GitRemote> {
        Err(ProviderError::Unsupported("No files in this walkthrough.".to_owned()))
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        Ok(vec![summary(13), summary(12)])
    }

    async fn pull_request(&self, _repo: &str, number: u64) -> Result<PullRequestDetail> {
        let other = if number == 12 { 13 } else { 12 };
        Ok(PullRequestDetail {
            summary: summary(number),
            description: format!("Goes with #{other}."),
            target_sha: "b".repeat(40),
            head_sha: "a".repeat(40),
            merge_status: review_core::MergeStatus::Mergeable,
            git_fetch_refs: Vec::new(),
            closed_at: None,
            auto_complete: None,
        })
    }

    async fn linked_items(&self, references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
        let linked = references
            .iter()
            .filter(|reference| {
                reference.repository_key == REPOSITORY && [12, 13].contains(&reference.number)
            })
            .map(|reference| LinkedItem {
                reference: reference.clone(),
                kind: ItemKind::PullRequest,
                repository_key: REPOSITORY.to_owned(),
                title: title(reference.number).to_owned(),
                state: LinkedState::Open,
                state_label: "Open".to_owned(),
                checks: None,
                item_type: None,
                web_url: format!("https://github.com/{REPOSITORY}/pull/{}", reference.number),
            })
            .collect();
        Ok(linked)
    }

    async fn merge(&self, _repo: &str, _number: u64, _options: &MergeOptions) -> Result<()> {
        Self::unused()
    }

    async fn close(&self, _repo: &str, _number: u64) -> Result<()> {
        Self::unused()
    }

    async fn reopen(&self, _repo: &str, _number: u64) -> Result<()> {
        Self::unused()
    }

    async fn set_draft(&self, _repo: &str, _number: u64, _is_draft: bool) -> Result<()> {
        Self::unused()
    }

    async fn update_title_description(
        &self,
        _repo: &str,
        _number: u64,
        _title: &str,
        _description: &str,
    ) -> Result<()> {
        Self::unused()
    }

    async fn set_vote(
        &self,
        _repo: &str,
        _number: u64,
        _vote: ReviewVote,
        _comment: Option<&str>,
    ) -> Result<()> {
        Self::unused()
    }

    async fn add_reviewer(&self, _repo: &str, _number: u64, _user: &str) -> Result<()> {
        Self::unused()
    }

    async fn remove_reviewer(&self, _repo: &str, _number: u64, _reviewer_id: &str) -> Result<()> {
        Self::unused()
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
        Self::unused()
    }

    async fn reply(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _body: &str,
    ) -> Result<ThreadComment> {
        Self::unused()
    }

    async fn set_thread_status(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _status: ThreadStatus,
    ) -> Result<()> {
        Self::unused()
    }

    async fn available_labels(&self, _repo: &str) -> Result<Vec<Label>> {
        Ok(Vec::new())
    }

    async fn set_labels(&self, _repo: &str, _number: u64, _labels: &[String]) -> Result<()> {
        Self::unused()
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

    async fn download_log(
        &self,
        _repo: &str,
        _number: u64,
        _log_id: &str,
        _sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()> {
        Self::unused()
    }
}
