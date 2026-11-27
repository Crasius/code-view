//! Walks the breadcrumb links, back and forward (Alt+Left/Right, the mouse's side buttons, the title
//! bar's arrows, across pages and pull request tabs, and macOS's Cmd+arrows) and the Linked card on
//! a real pull request page, offline:
//! a local git repository is the remote and a scripted provider describes the pull request and
//! answers the linked items. No network, no tokens. Runs with `cargo test`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use egui::{Event, Key, Modifiers, PointerButton, Role, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{
    Capabilities, Connection, GitRemote, ItemKind, ItemRef, Label, LinkedItem, LinkedState,
    MergeOptions, MergeStatus, MergeStrategy, NewThread, PipelineJob, PipelineRun, Provider,
    ProviderError, ProviderKind, PullRequestDetail, PullRequestQuery, PullRequestState,
    PullRequestSummary, RepositoryInfo, Result, ReviewThread, ReviewVote, RunStatus, ThreadComment,
    ThreadStatus, UserIdentity,
};
use review_store::{AppPaths, ConnectionStore};
use tokio::io::AsyncWrite;

use super::ReviewApp;
use cv_base::platform;
use cv_runtime::route::{PullRequestKey, PullRequestTab, Route};
use cv_testkit::Clicks;
use cv_testkit::{ScratchDir, git};

const CONNECTION_ID: &str = "local-navigation";
const CONNECTION_NAME: &str = "Acme GitHub";
const REPOSITORY: &str = "acme/widgets";
const NUMBER: u64 = 7;

const DESCRIPTION: &str = "Builds every widget with its final value right away.\n\n\
Follows #59 and needs acme/infra#3 first. Fixes #60.\n\n\
Tracked in [WEB-12](https://jira.example.com/browse/WEB-12); the follow-up is WEB-14.\n\n\
Upstream: https://github.com/acme/widgets/pull/61";

/// The local remote and the provider's call counter per connection id; the provider factory is
/// a plain function.
static FIXTURES: Mutex<Option<HashMap<String, Fixture>>> = Mutex::new(None);

#[derive(Clone)]
pub(super) struct Fixture {
    pub(super) clone_url: String,
    pub(super) target: String,
    pub(super) head: String,
    linked_calls: Arc<AtomicUsize>,
}

#[test]
fn navigation_breadcrumbs_back_and_linked_items() {
    let scratch = ScratchDir::new("navigation");
    let fixture = create_origin(&scratch.path().join("origin"));
    let linked_calls = fixture.linked_calls.clone();
    FIXTURES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(CONNECTION_ID.to_owned(), fixture);
    let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
    save_connection(&paths);
    let mut harness = harness(paths);

    // From outside (a link from another app): the trail is the pull request's structure.
    harness.state_mut().navigator.jump(Route::PullRequest(key(NUMBER)));
    wait_for(&mut harness, "the linked states", |harness| {
        harness.query_by_label_contains("Speed up login").is_some()
    });

    // The trail links every level above the page; the page itself is no link.
    for crumb in [review_core::APP_NAME, CONNECTION_NAME, REPOSITORY] {
        assert!(harness.query_by_role_and_label(Role::Link, crumb).is_some(), "{crumb} is a link");
    }
    assert!(harness.query_by_label("Pull request #7 (current page)").is_some());
    assert!(
        harness.query_by_label_contains("Back to My pull requests (").is_some(),
        "the back arrow names the list"
    );
    let forward = harness.get_by_label_contains("Forward (");
    assert!(forward.accesskit_node().is_disabled(), "nothing to go forward to yet");

    // One batch answered every reference; the tracker key without a link got the tracker's link.
    assert_eq!(linked_calls.load(Ordering::SeqCst), 1, "one linked_items call for the whole card");
    for row in [
        "#59 Speed up login (Merged)",
        "#60 Login is slow (Open)",
        "acme/infra#3 Provision cache (Draft)",
        "#61 Upstream widgets (Active)",
    ] {
        assert!(harness.query_by_label(row).is_some(), "row {row}");
    }
    for ticket in ["WEB-12", "WEB-14"] {
        assert!(harness.query_by_role_and_label(Role::Link, ticket).is_some(), "ticket {ticket}");
    }

    // A plain click on a pull request row opens it in the app.
    harness.click_at(harness.get_by_label("#59 Speed up login (Merged)").rect());
    wait_for(&mut harness, "pull request 59", |harness| {
        harness.query_by_label("Pull request #59 (current page)").is_some()
    });
    assert_eq!(harness.state().route, Route::PullRequest(key(59)));

    // Alt+Left goes back to the first pull request; its states come from the cache.
    harness.key_press_modifiers(Modifiers::ALT, egui::Key::ArrowLeft);
    wait_for(&mut harness, "pull request 7 again", |harness| {
        harness.query_by_label("Pull request #7 (current page)").is_some()
    });
    assert_eq!(harness.state().route, Route::PullRequest(key(NUMBER)));
    for _ in 0..120 {
        harness.step();
    }
    assert_eq!(
        linked_calls.load(Ordering::SeqCst),
        1,
        "revisiting asks nothing new and nothing polls"
    );

    // Every crumb leads to its level.
    harness.click_at(harness.get_by_role_and_label(Role::Link, REPOSITORY).rect());
    settle(&mut harness);
    assert_eq!(
        harness.state().route,
        Route::RepositoryPullRequests {
            connection_id: CONNECTION_ID.to_owned(),
            repository_key: REPOSITORY.to_owned()
        }
    );
    harness.click_at(harness.get_by_role_and_label(Role::Link, CONNECTION_NAME).rect());
    settle(&mut harness);
    assert_eq!(
        harness.state().route,
        Route::Connection { connection_id: CONNECTION_ID.to_owned() }
    );
    harness.click_at(harness.get_by_role_and_label(Role::Link, review_core::APP_NAME).rect());
    settle(&mut harness);
    assert_eq!(harness.state().route, Route::MyPullRequests);
    assert!(
        harness.query_by_role_and_label(Role::Link, review_core::APP_NAME).is_none(),
        "the root is the current page now"
    );

    back_and_forward_through_tabs(&mut harness);
}

/// From the list: a pull request's tabs are steps of the history like pages. The mouse's side
/// buttons, Alt+Left/Right and the bar's arrows walk them; a new step clears forward.
fn back_and_forward_through_tabs(harness: &mut Harness<'_, ReviewApp>) {
    let pull_request = Route::PullRequest(key(NUMBER));
    harness.state_mut().navigator.go(pull_request.clone());
    wait_for(harness, "the pull request again", |harness| tab_node(harness, "Files").is_some());
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Overview)));
    click_tab(harness, "Files");
    click_tab(harness, "Pipelines");
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Pipelines)));

    // A side button click is a press and a release: one step, over the page's content.
    side_button(harness, PointerButton::Extra1);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Files)));
    harness.key_press_modifiers(Modifiers::ALT, egui::Key::ArrowLeft);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Overview)));
    harness.key_press_modifiers(Modifiers::ALT, egui::Key::ArrowLeft);
    settle(harness);
    assert_eq!(place(harness), (Route::MyPullRequests, None));

    // Forward retraces the steps, tab by tab.
    side_button(harness, PointerButton::Extra2);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Overview)));
    harness.key_press_modifiers(Modifiers::ALT, egui::Key::ArrowRight);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Files)));
    assert!(
        harness.query_by_label_contains("Forward to Pull request #7, Pipelines tab (").is_some(),
        "the forward arrow names the tab it leads to"
    );

    // A new step clears forward: Alt+Right goes nowhere, the arrow rests disabled.
    click_tab(harness, "Commits");
    harness.key_press_modifiers(Modifiers::ALT, egui::Key::ArrowRight);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Commits)));
    let forward = harness.get_by_label_contains("Forward (");
    assert!(forward.accesskit_node().is_disabled(), "forward was cleared");

    // The bar's back arrow leads where its tooltip says.
    harness.click_at(harness.get_by_label_contains("Back to Pull request #7, Files tab (").rect());
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Files)));

    command_arrows_on_macos(harness, &pull_request);
}

/// On macOS Cmd+Right and Cmd+Left step like Chrome, except in a text field, where they move to
/// the line's ends. The arrows name the system's shortcut, as every other shortcut label does.
fn command_arrows_on_macos(harness: &mut Harness<'_, ReviewApp>, pull_request: &Route) {
    let expected =
        if cfg!(target_os = "macos") { ("⌘[", "⌘]") } else { ("Alt+Left", "Alt+Right") };
    assert_eq!((platform::BACK_SHORTCUT, platform::FORWARD_SHORTCUT), expected);
    let back_label = format!("Back to Pull request #7, Overview tab ({})", platform::BACK_SHORTCUT);
    assert!(harness.query_by_label(&back_label).is_some(), "the back arrow names {back_label}");
    let forward_label =
        format!("Forward to Pull request #7, Commits tab ({})", platform::FORWARD_SHORTCUT);
    assert!(harness.query_by_label(&forward_label).is_some(), "the forward arrow: {forward_label}");

    harness.ctx.set_os(egui::os::OperatingSystem::Mac);
    // What egui-winit reports for ⌘ on macOS: the Command key, which is also egui's `command`.
    let command = Modifiers::MAC_CMD | Modifiers::COMMAND;
    harness.key_press_modifiers(command, Key::ArrowRight);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Commits)));
    harness.key_press_modifiers(command, Key::ArrowLeft);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Files)));

    // The search box has the keys: Cmd+arrows and Option+arrows stay in it.
    harness.key_press_modifiers(command, Key::K);
    settle(harness);
    assert!(harness.ctx.text_edit_focused(), "the search box takes the keys");
    for modifiers in [command, Modifiers::ALT] {
        for arrow in [Key::ArrowLeft, Key::ArrowRight] {
            harness.key_press_modifiers(modifiers, arrow);
            settle(harness);
            assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Files)));
        }
    }
    harness.key_press(Key::Escape);
    settle(harness);
    assert!(!harness.ctx.text_edit_focused(), "Esc closed the search");
    harness.key_press_modifiers(command, Key::ArrowLeft);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Overview)));
}

/// The route and the pull request tab the app shows.
fn place(harness: &Harness<'_, ReviewApp>) -> (Route, Option<PullRequestTab>) {
    let app = harness.state();
    (app.route.clone(), app.current_tab())
}

fn tab_node<'a>(harness: &'a Harness<'_, ReviewApp>, name: &str) -> Option<egui_kittest::Node<'a>> {
    harness.query_all_by_role(Role::Tab).find(|node| {
        node.accesskit_node().label().is_some_and(|label| label.split(" (").next() == Some(name))
    })
}

fn click_tab(harness: &mut Harness<'_, ReviewApp>, name: &str) {
    harness.click_at(tab_node(harness, name).unwrap_or_else(|| panic!("the {name} tab")).rect());
    settle(harness);
}

/// Presses and releases a mouse side button in the middle of the page.
fn side_button(harness: &mut Harness<'_, ReviewApp>, button: PointerButton) {
    let pos = pos2(720.0, 500.0);
    harness.event(Event::PointerMoved(pos));
    for pressed in [true, false] {
        harness.event(Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE });
        harness.step();
    }
    settle(harness);
}

fn key(number: u64) -> PullRequestKey {
    PullRequestKey {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number,
    }
}

fn harness(paths: AppPaths) -> Harness<'static, ReviewApp> {
    let builder = Harness::builder().with_size([1440.0, 900.0]).with_step_dt(1.0 / 60.0);
    super::startup::start(builder, |creation| ReviewApp::new(creation, paths, local_provider))
}

fn wait_for(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    let started = Instant::now();
    super::walkthrough::wait_for(harness, what, |harness| is_done(harness).then_some(()), |_| {});
    println!("{what} after {:.1} s", started.elapsed().as_secs_f32());
    settle(harness);
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..20 {
        harness.step();
    }
}

/// A repository with one changed README.
pub(super) fn create_origin(origin: &Path) -> Fixture {
    std::fs::create_dir_all(origin).expect("origin folder");
    git(origin, &["init", "-q", "-b", "main"]);
    git(origin, &["config", "uploadpack.allowFilter", "true"]);
    git(origin, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    std::fs::write(origin.join("README.md"), "# Widgets\n").expect("write readme");
    git(origin, &["add", "-A"]);
    git(origin, &["commit", "-q", "-m", "base"]);
    let target = git(origin, &["rev-parse", "HEAD"]);
    git(origin, &["switch", "-q", "-c", "feature"]);
    std::fs::write(origin.join("README.md"), "# Widgets\n\nFast now.\n").expect("write readme");
    git(origin, &["add", "-A"]);
    git(origin, &["commit", "-q", "-m", "Build widgets eagerly"]);
    let head = git(origin, &["rev-parse", "HEAD"]);
    Fixture {
        clone_url: format!("file:///{}", origin.display().to_string().replace('\\', "/")),
        target,
        head,
        linked_calls: Arc::default(),
    }
}

fn save_connection(paths: &AppPaths) {
    let mut store = ConnectionStore::open(paths).expect("empty store");
    store
        .upsert(Connection {
            id: CONNECTION_ID.to_owned(),
            name: CONNECTION_NAME.to_owned(),
            kind: ProviderKind::GitHub,
            base_url: ProviderKind::GitHub.default_base_url().to_owned(),
            organizations: Vec::new(),
            token: None,
        })
        .expect("store the connection");
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "a ProviderFactory, whose signature hands the connection and the client over"
)]
fn local_provider(connection: Connection, _http: reqwest::Client) -> Arc<dyn Provider> {
    let fixture = FIXTURES
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|fixtures| fixtures.get(&connection.id).cloned());
    Arc::new(LocalProvider { fixture })
}

struct LocalProvider {
    fixture: Option<Fixture>,
}

impl LocalProvider {
    fn fixture(&self) -> Result<&Fixture> {
        self.fixture.as_ref().ok_or_else(|| ProviderError::Local("no local fixture".to_owned()))
    }

    fn unused<T>() -> Result<T> {
        Err(ProviderError::Unsupported("Not part of the navigation walkthrough.".to_owned()))
    }
}

fn author() -> UserIdentity {
    UserIdentity {
        id: "1".into(),
        display_name: "Ada Lovelace".into(),
        login: Some("ada".into()),
        avatar_url: None,
    }
}

/// What the scripted platform knows about the referenced numbers.
fn linked_item(reference: &ItemRef) -> Option<LinkedItem> {
    let (kind, title, state, label, checks) =
        match (reference.repository_key.as_str(), reference.number) {
            (REPOSITORY, 59) => (
                ItemKind::PullRequest,
                "Speed up login",
                LinkedState::Merged,
                "Merged",
                Some(RunStatus::Succeeded),
            ),
            (REPOSITORY, 60) => (ItemKind::Issue, "Login is slow", LinkedState::Open, "Open", None),
            (REPOSITORY, 61) => (
                ItemKind::PullRequest,
                "Upstream widgets",
                LinkedState::Open,
                "Active",
                Some(RunStatus::Failed),
            ),
            ("acme/infra", 3) => (
                ItemKind::PullRequest,
                "Provision cache",
                LinkedState::Draft,
                "Draft",
                Some(RunStatus::Running),
            ),
            _ => return None,
        };
    let path = if kind == ItemKind::PullRequest { "pull" } else { "issues" };
    Some(LinkedItem {
        reference: reference.clone(),
        kind,
        repository_key: reference.repository_key.clone(),
        title: title.to_owned(),
        state,
        state_label: label.to_owned(),
        checks,
        item_type: None,
        web_url: format!(
            "https://github.com/{}/{path}/{}",
            reference.repository_key, reference.number
        ),
    })
}

static CAPABILITIES: std::sync::LazyLock<Capabilities> =
    std::sync::LazyLock::new(|| Capabilities {
        votes: vec![ReviewVote::Approved, ReviewVote::None],
        merge_strategies: vec![MergeStrategy::Merge],
        has_label_catalog: false,
        running_logs: review_core::RunningLogs::Growing,
    });

#[async_trait]
impl Provider for LocalProvider {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(author())
    }

    async fn list_repositories(&self, _search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        Ok(Vec::new())
    }

    async fn git_remote(&self, _repo: &str) -> Result<GitRemote> {
        Ok(GitRemote { clone_url: self.fixture()?.clone_url.clone(), authorization_header: None })
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        Ok(Vec::new())
    }

    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        let fixture = self.fixture()?;
        let created_at = time::macros::datetime!(2026-10-01 09:30 UTC);
        let (title, description) = match number {
            NUMBER => ("Build widgets eagerly", DESCRIPTION),
            _ => ("Speed up login", "Caches the session lookup."),
        };
        Ok(PullRequestDetail {
            summary: PullRequestSummary {
                repository_key: repo.to_owned(),
                repository_name: "widgets".to_owned(),
                number,
                title: title.to_owned(),
                author: author(),
                state: PullRequestState::Open,
                is_draft: false,
                source_branch: "feature".to_owned(),
                target_branch: "main".to_owned(),
                created_at,
                updated_at: None,
                web_url: format!("https://github.com/{repo}/pull/{number}"),
                labels: Vec::new(),
                reviewers: Vec::new(),
                checks: None,
                diff_stat: review_core::DiffStat::default(),
                head_sha: None,
                target_sha: None,
            },
            description: description.to_owned(),
            target_sha: fixture.target.clone(),
            head_sha: fixture.head.clone(),
            merge_status: MergeStatus::Mergeable,
            git_fetch_refs: vec!["refs/heads/feature".to_owned(), "refs/heads/main".to_owned()],
            closed_at: None,
            auto_complete: None,
        })
    }

    async fn linked_items(&self, references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
        self.fixture()?.linked_calls.fetch_add(1, Ordering::SeqCst);
        Ok(references.iter().filter_map(linked_item).collect())
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
