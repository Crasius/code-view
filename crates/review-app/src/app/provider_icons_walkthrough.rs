//! Platform marks across the app, offline: one Azure DevOps, one GitHub and one self-managed
//! GitLab connection answered by a scripted provider. Checks that every mark carries the platform
//! and host as its accessible name: on the overview, each connection's breadcrumb and list, and
//! a pull request header, whose mark links to the pull request.
//! `cargo test -p review-app provider_icons`

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{
    Capabilities, Connection, GitRemote, Label, MergeOptions, MergeStatus, MergeStrategy,
    NewThread, PipelineJob, PipelineRun, Provider, ProviderError, ProviderKind, PullRequestDetail,
    PullRequestQuery, PullRequestState, PullRequestSummary, RepositoryInfo, Result, ReviewThread,
    ReviewVote, RunStatus, ThreadComment, ThreadStatus, UserIdentity,
};
use review_store::{AppPaths, ConnectionStore};
use tokio::io::AsyncWrite;

use super::ReviewApp;
use super::navigation_walkthrough::{Fixture, create_origin};
use cv_runtime::route::{PullRequestKey, Route};
use cv_settings::onboarding::CliTools;
use cv_testkit::Clicks;
use cv_testkit::{self as test_support, ScratchDir};

const TIMEOUT: Duration = Duration::from_secs(60);

/// A local repository GitHub's pull requests are fetched from, so its Files tab has a file.
static ORIGIN: Mutex<Option<Fixture>> = Mutex::new(None);

/// (id, name, platform, web root, repository, accessible name of its mark).
const HOSTS: [(&str, &str, ProviderKind, &str, &str, &str); 3] = [
    (
        "ado",
        "Contoso Azure DevOps",
        ProviderKind::AzureDevOps,
        "https://dev.azure.com/contoso",
        "contoso/Web/site",
        "Azure DevOps · dev.azure.com",
    ),
    (
        "gh",
        "Contoso GitHub",
        ProviderKind::GitHub,
        "https://github.com",
        "contoso/widgets",
        "GitHub · github.com",
    ),
    (
        "gl",
        "Fabrikam GitLab",
        ProviderKind::GitLab,
        "https://gitlab.example.com",
        "fabrikam/app",
        "GitLab self-managed · gitlab.example.com",
    ),
];

#[test]
fn provider_icons_name_the_platform_everywhere() {
    let scratch = ScratchDir::new("provider-icons-origin");
    *ORIGIN.lock().unwrap() = Some(create_origin(&scratch.path().join("origin")));
    let paths = isolated_paths();
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    for (id, name, kind, base_url, _, _) in HOSTS {
        store
            .upsert(Connection {
                id: id.to_owned(),
                name: name.to_owned(),
                kind,
                base_url: base_url.to_owned(),
                organizations: if kind == ProviderKind::AzureDevOps {
                    vec!["contoso".to_owned()]
                } else {
                    Vec::new()
                },
                token: Some("scripted".to_owned()),
            })
            .expect("store the connection");
    }
    let builder = Harness::builder().with_size([1280.0, 1080.0]).with_step_dt(1.0 / 60.0);
    let mut harness = super::startup::start(builder, move |creation| {
        ReviewApp::new(creation, paths, scripted_provider)
    });
    harness.state_mut().settings.cli_tools = Some(CliTools::default());

    wait_for(&mut harness, "every card", |harness| {
        HOSTS.iter().all(|(_, _, kind, ..)| has(harness, &title(*kind, 1)))
    });
    for (_, _, _, _, _, mark) in HOSTS {
        // Card header plus one per row: never a mark without its platform and host.
        assert!(
            harness.query_all_by_role_and_label(Role::Image, mark).count() >= 3,
            "{mark} on the card and its rows"
        );
    }
    harness.state_mut().navigator.go(Route::MyPullRequests);
    settle(&mut harness);
    for (id, connection_name, kind, _, repository, mark) in HOSTS {
        // As from outside: the trail is the structure, with the connection's crumb.
        harness.state_mut().navigator.jump(Route::RepositoryPullRequests {
            connection_id: id.to_owned(),
            repository_key: repository.to_owned(),
        });
        wait_for(&mut harness, "the repository's rows", |harness| has(harness, &title(kind, 1)));
        assert!(
            harness.query_by_role_and_label(Role::Link, connection_name).is_some(),
            "{connection_name} crumb"
        );
        if kind == ProviderKind::AzureDevOps {
            crumb_tooltip(&mut harness, connection_name, mark);
        }
    }
    for (id, _, kind, _, repository, _) in HOSTS {
        let key = PullRequestKey {
            connection_id: id.to_owned(),
            repository_key: repository.to_owned(),
            number: 1,
        };
        harness.state_mut().navigator.go(Route::PullRequest(key));
        // The page keeps the tab it showed last; its header names it on every tab.
        let shown = title(kind, 1);
        wait_for(&mut harness, "the pull request header", |harness| {
            harness.query_by_role_and_label(Role::Tab, "Overview").is_some() && has(harness, &shown)
        });
    }
    // GitHub's page was opened last: its platform mark links to the pull request.
    platform_link(&mut harness);
}

/// Hovering the connection's crumb opens a tooltip that names the platform and host too.
fn crumb_tooltip(harness: &mut Harness<'_, ReviewApp>, connection_name: &str, mark: &str) {
    let marks = harness.query_all_by_label(mark).count();
    harness.get_by_role_and_label(Role::Link, connection_name).hover();
    for _ in 0..60 {
        harness.step();
    }
    assert!(has(harness, "This connection's repositories"), "the crumb's tooltip is open");
    assert!(harness.query_all_by_label(mark).count() > marks, "the crumb's tooltip names {mark}");
    harness.event(egui::Event::PointerGone);
    settle(harness);
}

/// The pull request header's platform mark: a link (Enter opens the pull request), with a hover
/// state, and a menu that adds the file the Files tab shows.
fn platform_link(harness: &mut Harness<'_, ReviewApp>) {
    const LINK: &str = "Open pull request on GitHub · github.com";
    const PAGE: &str = "https://github.com/contoso/widgets/pull/1";
    let key = PullRequestKey {
        connection_id: "gh".to_owned(),
        repository_key: "contoso/widgets".to_owned(),
        number: 1,
    };
    harness.state_mut().navigator.go(Route::PullRequest(key));
    wait_for(harness, "the GitHub pull request", |harness| {
        harness.query_by_role_and_label(Role::Link, LINK).is_some()
    });
    harness.get_by_role_and_label(Role::Link, LINK).focus();
    harness.step();
    assert!(
        harness.get_by_role_and_label(Role::Link, LINK).is_focused(),
        "the mark takes keyboard focus"
    );
    // One event per frame: the key-down frame is the one that opens the page.
    let enter = |pressed| egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    harness.event(enter(true));
    harness.step();
    assert_eq!(opened_urls(harness), [PAGE], "Enter on the focused mark opens the pull request");
    harness.event(enter(false));
    harness.step();
    // A click (on release) and a middle click open it too.
    harness.click_at(harness.get_by_role_and_label(Role::Link, LINK).rect());
    harness.step();
    assert_eq!(opened_urls(harness), [PAGE], "a click opens the pull request");
    let at = harness.get_by_role_and_label(Role::Link, LINK).rect().center();
    hooked_middle_click(harness, at);
    assert_eq!(opened_urls(harness), [PAGE], "a middle click opens the pull request");
    assert!(!harness.state().autoscroll.is_active(), "and starts no autoscroll");
    harness.event(egui::Event::PointerGone);
    settle(harness);
    harness.get_by_role_and_label(Role::Link, LINK).hover();
    for _ in 0..60 {
        harness.step();
    }
    assert!(has(harness, "Right-click for more links"), "the mark's tooltip");

    // On the Files tab the menu also offers the shown file.
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files"))
            })
            .rect(),
    );
    // The tab opens on the summary of the changed files;
    // the card's View opens the file.
    wait_for(harness, "the changed files", |harness| has(harness, "README.md"));
    if let Some(view) = harness.query_by_label("View README.md") {
        harness.click_at(view.rect());
        settle(harness);
    }
    harness.secondary_click_at(harness.get_by_role_and_label(Role::Link, LINK).rect());
    settle(harness);
    for item in ["Open pull request on GitHub", "Open this file on GitHub", "Copy link"] {
        assert!(harness.query_by_label(item).is_some(), "menu item {item}");
    }
    harness.click_at(harness.get_by_label("Open this file on GitHub").rect());
    harness.step();
    let opened = opened_urls(harness);
    assert!(
        opened.len() == 1
            && opened[0].starts_with("https://github.com/contoso/widgets/blob/")
            && opened[0].ends_with("/README.md"),
        "the menu opens the file at the head commit: {opened:?}"
    );
    // The file header no longer offers a platform link beside its toggles.
    assert!(
        harness.query_by_label("Open on GitHub").is_none(),
        "no platform button in the file header"
    );
    harness.key_press(egui::Key::Escape);
    harness.event(egui::Event::PointerGone);
    settle(harness);
}

fn opened_urls(harness: &Harness<'_, ReviewApp>) -> Vec<String> {
    harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
            _ => None,
        })
        .collect()
}

fn title(kind: ProviderKind, number: u64) -> String {
    format!("{} change {number}", kind.label())
}

fn isolated_paths() -> AppPaths {
    let root = std::env::temp_dir().join(format!("review-provider-icons-{}", std::process::id()));
    test_support::remove_dir(&root);
    AppPaths::new(root.join("config"), root.join("cache"))
}

fn has(harness: &Harness<'_, ReviewApp>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

fn wait_for(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    let started = Instant::now();
    loop {
        harness.step();
        if is_done(harness) {
            settle(harness);
            return;
        }
        if started.elapsed() > TIMEOUT {
            let tree: String = format!("{:?}", harness.root()).chars().take(6000).collect();
            panic!("timed out waiting for {what}; screen:\n{tree}");
        }
        test_support::let_threads_run(Duration::from_millis(10));
    }
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..30 {
        harness.step();
    }
}

/// A middle click at `at`, one frame each for press and release, whose raw input first runs
/// through the app's `raw_input_hook` as eframe does: autoscroll sees the press first.
fn hooked_middle_click(harness: &mut Harness<'_, ReviewApp>, at: egui::Pos2) {
    harness.hover_at(at);
    harness.step();
    for pressed in [true, false] {
        let ctx = harness.ctx.clone();
        let mut input = std::mem::take(harness.input_mut());
        input.events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Middle,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        eframe::App::raw_input_hook(harness.state_mut(), &ctx, &mut input);
        *harness.input_mut() = input;
        harness.step();
    }
}

pub(super) fn scripted_provider(
    connection: Connection,
    _http: reqwest::Client,
) -> Arc<dyn Provider> {
    Arc::new(ScriptedProvider { connection })
}

/// Answers listings and pull requests of its connection's platform; everything else is unused.
struct ScriptedProvider {
    connection: Connection,
}

impl ScriptedProvider {
    fn repository(&self) -> &'static str {
        HOSTS
            .iter()
            .find(|(id, ..)| *id == self.connection.id)
            .map_or("unknown/repo", |(_, _, _, _, repository, _)| repository)
    }

    fn summary(&self, number: u64) -> PullRequestSummary {
        let kind = self.connection.kind;
        let repository = self.repository();
        let base = &self.connection.base_url;
        let web_url = match kind {
            ProviderKind::AzureDevOps => format!("{base}/Web/_git/site/pullrequest/{number}"),
            ProviderKind::GitHub => format!("{base}/{repository}/pull/{number}"),
            ProviderKind::GitLab => format!("{base}/{repository}/-/merge_requests/{number}"),
        };
        PullRequestSummary {
            repository_key: repository.to_owned(),
            repository_name: repository.rsplit('/').next().unwrap_or(repository).to_owned(),
            number,
            title: title(kind, number),
            author: person("1", "Ada Lovelace"),
            state: PullRequestState::Open,
            is_draft: number == 2,
            source_branch: format!("feature/{number}"),
            target_branch: "main".to_owned(),
            created_at: time::macros::datetime!(2026-10-16 09:30 UTC),
            updated_at: Some(time::macros::datetime!(2026-10-18 15:00 UTC)),
            web_url,
            labels: Vec::new(),
            reviewers: Vec::new(),
            checks: Some(if number == 1 { RunStatus::Succeeded } else { RunStatus::Running }),
            diff_stat: review_core::DiffStat::default(),
            head_sha: None,
            target_sha: None,
        }
    }

    fn origin(&self) -> Option<Fixture> {
        (self.connection.kind == ProviderKind::GitHub)
            .then(|| ORIGIN.lock().unwrap().clone())
            .flatten()
    }

    fn unused<T>() -> Result<T> {
        Err(ProviderError::Unsupported("Not part of the provider icons walkthrough.".to_owned()))
    }
}

fn person(id: &str, name: &str) -> UserIdentity {
    UserIdentity {
        id: id.to_owned(),
        display_name: name.to_owned(),
        login: Some(name.to_lowercase()),
        avatar_url: None,
    }
}

static CAPABILITIES: std::sync::LazyLock<Capabilities> =
    std::sync::LazyLock::new(|| Capabilities {
        votes: vec![ReviewVote::Approved, ReviewVote::None],
        merge_strategies: vec![MergeStrategy::Merge],
        has_label_catalog: false,
        running_logs: review_core::RunningLogs::Growing,
    });

#[async_trait]
impl Provider for ScriptedProvider {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(person("1", "Ada Lovelace"))
    }

    async fn list_repositories(&self, _search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        Ok(Vec::new())
    }

    async fn git_remote(&self, _repo: &str) -> Result<GitRemote> {
        let origin = self.origin().ok_or_else(|| {
            ProviderError::Unsupported("No files in this walkthrough.".to_owned())
        })?;
        Ok(GitRemote { clone_url: origin.clone_url, authorization_header: None })
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        Ok((1..=3).map(|number| self.summary(number)).collect())
    }

    async fn pull_request(&self, _repo: &str, number: u64) -> Result<PullRequestDetail> {
        // GitHub's pull requests come from the local origin; the others have no files to show.
        let (target_sha, head_sha, git_fetch_refs) = match self.origin() {
            Some(origin) => (
                origin.target,
                origin.head,
                vec!["refs/heads/feature".to_owned(), "refs/heads/main".to_owned()],
            ),
            None => ("b".repeat(40), "a".repeat(40), Vec::new()),
        };
        Ok(PullRequestDetail {
            summary: self.summary(number),
            description: "Merge or complete this change on any platform.".to_owned(),
            target_sha,
            head_sha,
            merge_status: MergeStatus::Mergeable,
            git_fetch_refs,
            closed_at: None,
            auto_complete: None,
        })
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
