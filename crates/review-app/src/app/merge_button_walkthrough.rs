//! The header's merge split button, offline: an Azure DevOps connection answered by a scripted
//! provider whose target branch allows merge, squash and rebase (squash the default), not
//! semi-linear. Pull request 1 is mergeable, 2 has conflicts, 3 is blocked by policies.
//!
//! Checks the in-place confirm (click twice, Enter, Esc, the 4 s window), the dropdown (number
//! keys, picking keeps it open, message per strategy, Ctrl+Enter arms), what gets merged and
//! remembered, and that an inactive window loads no policy. Auto-complete on each platform is in
//! [`auto_complete`].
//! `cargo test -p review-app merge_button`

mod auto_complete;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{
    AutoComplete, Capabilities, Connection, GitRemote, Label, MergeOptions, MergePolicy,
    MergeStatus, MergeStrategy, NewThread, PipelineJob, PipelineRun, Provider, ProviderError,
    ProviderKind, PullRequestDetail, PullRequestQuery, PullRequestState, PullRequestSummary,
    RepositoryInfo, Result, ReviewThread, ReviewVote, ThreadComment, ThreadStatus, UserIdentity,
};
use review_store::{AppPaths, AppSettings, ConnectionStore};
use tokio::io::AsyncWrite;

use super::ReviewApp;
use cv_runtime::route::{PullRequestKey, Route};
use cv_settings::onboarding::CliTools;
use cv_testkit::Clicks;

const REPOSITORY: &str = "contoso/Web/site";
const POLICIES: &str = "https://dev.azure.com/contoso/Web/_settings/repositories?_a=policiesMid";
const OPTIONS: &str = "Merge options";

/// Every merge a connection's scripted provider was asked for, and how often for a policy.
/// Each test has its own connection, so tests running side by side count apart.
#[derive(Default)]
struct Calls {
    merges: Mutex<Vec<MergeOptions>>,
    policies: AtomicUsize,
    /// Auto-complete set (with its options) and cancelled, in order.
    auto_completes: Mutex<Vec<Option<MergeOptions>>>,
    /// The auto-complete the blocked pull request (3) has now.
    auto_complete: Mutex<Option<AutoComplete>>,
    /// The platform refuses to set auto-complete, as GitHub does where a repository forbids it.
    refuses_auto_complete: bool,
}

static CALLS: LazyLock<Mutex<HashMap<String, Arc<Calls>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[test]
fn merge_button_confirms_in_place_and_remembers_the_choice() {
    let (mut harness, paths, calls) = start("confirm");
    open(&mut harness, "confirm", 1);
    wait_for(&mut harness, "the preselected default", |harness| button(harness, "Squash merge"));
    assert!(
        has(&harness, "Merge type: Squash merge"),
        "the overview card names what the button runs"
    );

    // A click arms the button in place; the window runs out after 4 s.
    press(&mut harness, "Squash merge");
    assert!(button(&harness, "Click again to squash merge"), "armed in place");
    assert!(calls.merges.lock().unwrap().is_empty(), "one click merges nothing");
    pump(&mut harness, 4.5);
    assert!(button(&harness, "Squash merge"), "the confirm window ran out");

    // Esc disarms.
    press(&mut harness, "Squash merge");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(button(&harness, "Squash merge"), "Esc disarms");

    // The dropdown: number keys and clicks pick, it stays open, the message follows.
    press(&mut harness, OPTIONS);
    assert!(
        radio(&harness, "Squash merge", true) && radio(&harness, "Merge commit", false),
        "the preselection is checked"
    );
    assert!(
        harness.query_by_label("Semi-linear merge").is_none(),
        "the disallowed strategy is not offered"
    );
    assert!(
        has(&harness, "Semi-linear merge isn't allowed on main."),
        "what the branch forbids is said"
    );
    assert!(has(&harness, "Squash commit message"));
    harness.key_press(egui::Key::Num3);
    harness.run_steps(3);
    assert!(radio(&harness, "Rebase and fast-forward", true), "3 picks the third strategy");
    assert!(button(&harness, "Rebase and fast-forward"), "the main part follows");
    assert!(!has(&harness, "commit message"), "rebase writes no commit of its own");
    harness.click_at(harness.get_by_role_and_label(Role::MenuItemRadio, "Merge commit").rect());
    harness.run_steps(3);
    assert!(
        has(&harness, "Merge commit message") && radio(&harness, "Merge commit", true),
        "a click picks and keeps it open"
    );
    harness.click_at(harness.get_by_role_and_label(Role::Link, "Branch policies").rect());
    harness.step();
    assert_eq!(opened_urls(&harness), [POLICIES], "Branch policies opens the platform's page");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(harness.query_by_role(Role::MenuItemRadio).is_none(), "Esc closes the dropdown");
    assert!(button(&harness, "Merge commit"), "the pick stays for this pull request");

    // Ctrl+Enter in the dropdown arms; Enter completes.
    press(&mut harness, OPTIONS);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    harness.run_steps(3);
    assert!(button(&harness, "Click again to merge"), "Ctrl+Enter arms the button");
    harness.key_press(egui::Key::Enter);
    wait_for(&mut harness, "the merge", |_| !calls.merges.lock().unwrap().is_empty());
    let merged = calls.merges.lock().unwrap()[0].clone();
    assert_eq!(merged.strategy, MergeStrategy::Merge);
    assert_eq!(merged.commit_message.as_deref(), Some("Merged PR 1: Speed up login"));
    assert!(merged.delete_source_branch);
    let saved = AppSettings::load(&paths.settings_file()).expect("saved settings");
    assert_eq!(
        saved.merge.last_used("confirm", REPOSITORY),
        Some(MergeStrategy::Merge),
        "the merge is remembered"
    );

    // A second click within the window completes; the branch choice is remembered per repository.
    // The mock still answers open; a reload by hand takes its word over the merge on the page.
    harness.key_press(egui::Key::F5);
    wait_for(&mut harness, "the button after the merge", |harness| button(harness, "Merge commit"));
    press(&mut harness, OPTIONS);
    harness.click_at(harness.get_by_label("Delete feature/login after merging").rect());
    harness.run_steps(3);
    harness.key_press(egui::Key::Num1);
    harness.run_steps(3);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    press(&mut harness, "Merge commit");
    press(&mut harness, "Click again to merge");
    wait_for(&mut harness, "the second merge", |_| calls.merges.lock().unwrap().len() == 2);
    assert!(!calls.merges.lock().unwrap()[1].delete_source_branch, "the unticked box is sent");
    let saved = AppSettings::load(&paths.settings_file()).expect("saved settings");
    assert_eq!(saved.merge.delete_source_branch("confirm", REPOSITORY), Some(false));
}

#[test]
fn merge_button_is_disabled_with_conflicts_and_warns_when_forced_past_policies() {
    let (mut harness, _, calls) = start("states");
    open(&mut harness, "states", 2);
    wait_for(&mut harness, "the conflicted pull request", |harness| {
        harness.query_by_role_and_label(Role::Button, "Squash merge").is_some()
    });
    let main = harness.get_by_role_and_label(Role::Button, "Squash merge");
    assert!(main.accesskit_node().is_disabled(), "conflicts disable the button");
    harness.get_by_role_and_label(Role::Button, "Squash merge").hover();
    pump(&mut harness, 1.5);
    assert!(has(&harness, "This pull request has merge conflicts"), "the tooltip says why");
    // The overview card has its own; the tooltip opens above the page, its action comes last.
    harness.click_at(
        harness
            .query_all_by_label("Open in Azure DevOps")
            .last()
            .expect("the tooltip's action")
            .rect(),
    );
    harness.step();
    assert_eq!(opened_urls(&harness), [web_url(2)], "the tooltip's action opens the pull request");
    harness.event(egui::Event::PointerGone);

    // Blocked, the button sets auto-complete; unticked in the dropdown, it completes at once.
    open(&mut harness, "states", 3);
    wait_for(&mut harness, "the blocked pull request", |harness| {
        button(harness, "Set auto-complete") && has(harness, "Blocked change")
    });
    press(&mut harness, OPTIONS);
    harness.click_at(harness.get_by_label("Complete automatically when policies pass").rect());
    harness.run_steps(3);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(button(&harness, "Squash merge"), "unticked, the button completes now");
    press(&mut harness, "Squash merge");
    assert!(
        has(&harness, "Policies or required reviews still block this merge."),
        "armed, the block is said"
    );
    assert!(
        harness.query_all_by_label("Open in Azure DevOps").count() >= 1,
        "with the way to the platform"
    );
    assert!(calls.merges.lock().unwrap().is_empty());
    assert!(calls.auto_completes.lock().unwrap().is_empty(), "nothing set either");
}

#[test]
fn merge_button_loads_no_policy_while_the_window_is_inactive() {
    let (mut harness, _, calls) = start("inactive");
    wait_for(&mut harness, "the list", |harness| has(harness, "Speed up login"));
    set_focus(&mut harness, false);
    open(&mut harness, "inactive", 1);
    wait_for(&mut harness, "the header", |harness| {
        has(harness, "Speed up login") && harness.query_all_by_role(Role::Tab).next().is_some()
    });
    pump(&mut harness, 3.0);
    assert_eq!(calls.policies.load(Ordering::SeqCst), 0, "no policy request while inactive");
    assert!(button(&harness, "Complete"), "the button waits with its neutral label");
    set_focus(&mut harness, true);
    wait_for(&mut harness, "the policy once active", |harness| button(harness, "Squash merge"));
    assert_eq!(calls.policies.load(Ordering::SeqCst), 1, "one request once active");
}

/// The app with one Azure DevOps connection with id `name`, and what its provider is asked.
fn start(name: &str) -> (Harness<'static, ReviewApp>, AppPaths, Arc<Calls>) {
    start_on(name, ProviderKind::AzureDevOps, Calls::default())
}

/// [`start`] with a connection of `kind` whose provider answers with `calls`.
fn start_on(
    name: &str,
    kind: ProviderKind,
    calls: Calls,
) -> (Harness<'static, ReviewApp>, AppPaths, Arc<Calls>) {
    let calls = Arc::new(calls);
    CALLS.lock().unwrap().insert(name.to_owned(), calls.clone());
    let root =
        std::env::temp_dir().join(format!("review-merge-button-{name}-{}", std::process::id()));
    cv_testkit::remove_dir(&root);
    let paths = AppPaths::new(root.join("config"), root.join("cache"));
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    store
        .upsert(Connection {
            id: name.to_owned(),
            name: "Contoso".to_owned(),
            kind,
            base_url: "https://dev.azure.com/contoso".to_owned(),
            organizations: vec!["contoso".to_owned()],
            token: Some("scripted".to_owned()),
        })
        .expect("store the connection");
    let app_paths = paths.clone();
    let builder = Harness::builder().with_size([1280.0, 860.0]).with_step_dt(1.0 / 30.0);
    let mut harness = super::startup::start(builder, move |creation| {
        ReviewApp::new(creation, app_paths, scripted_provider)
    });
    harness.state_mut().settings.cli_tools = Some(CliTools::default());
    (harness, paths, calls)
}

fn open(harness: &mut Harness<'_, ReviewApp>, connection_id: &str, number: u64) {
    let key = PullRequestKey {
        connection_id: connection_id.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number,
    };
    harness.state_mut().navigator.go(Route::PullRequest(key));
    harness.step();
}

/// A button labelled exactly `label`.
fn button(harness: &Harness<'_, ReviewApp>, label: &str) -> bool {
    harness.query_by_role_and_label(Role::Button, label).is_some()
}

/// The strategy row `label` exists and is checked as `is_checked`.
fn radio(harness: &Harness<'_, ReviewApp>, label: &str, is_checked: bool) -> bool {
    harness.query_by_role_and_label(Role::MenuItemRadio, label).is_some_and(|node| {
        let toggled = node.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True);
        toggled == is_checked
    })
}

/// Moves the pointer onto the labelled widget first, as a mouse does, then clicks it.
fn press(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    harness.get_by_label(label).hover();
    harness.step();
    harness.click_at(harness.get_by_label(label).rect());
    harness.run_steps(4);
}

fn has(harness: &Harness<'_, ReviewApp>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
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

fn set_focus(harness: &mut Harness<'_, ReviewApp>, is_focused: bool) {
    let viewport = harness.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default();
    viewport.focused = Some(is_focused);
    harness.input_mut().focused = is_focused;
    harness.step();
}

/// Advances simulated time; each frame also gives background work a moment of real time.
fn pump(harness: &mut Harness<'_, ReviewApp>, seconds: f64) {
    let end = harness.ctx.input(|input| input.time) + seconds;
    while harness.ctx.input(|input| input.time) < end {
        harness.step();
        cv_testkit::let_threads_run(Duration::from_millis(2));
    }
}

fn wait_for(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    super::walkthrough::wait_for(harness, what, |harness| is_done(harness).then_some(()), |_| {});
    harness.run_steps(3);
}

fn web_url(number: u64) -> String {
    format!("https://dev.azure.com/contoso/Web/_git/site/pullrequest/{number}")
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "a ProviderFactory, whose signature hands the connection and the client over"
)]
fn scripted_provider(connection: Connection, _http: reqwest::Client) -> Arc<dyn Provider> {
    let calls = CALLS.lock().unwrap().get(&connection.id).cloned().unwrap_or_default();
    Arc::new(ScriptedProvider { calls })
}

struct ScriptedProvider {
    calls: Arc<Calls>,
}

fn person(id: &str, name: &str) -> UserIdentity {
    UserIdentity {
        id: id.to_owned(),
        display_name: name.to_owned(),
        login: Some(name.to_lowercase()),
        avatar_url: None,
    }
}

/// (title, source branch, merge status) of pull requests 1 to 3.
fn change(number: u64) -> (&'static str, &'static str, MergeStatus) {
    match number {
        1 => ("Speed up login", "feature/login", MergeStatus::Mergeable),
        2 => ("Conflicting change", "feature/conflict", MergeStatus::Conflicts),
        _ => ("Blocked change", "feature/blocked", MergeStatus::Blocked),
    }
}

fn summary(number: u64) -> PullRequestSummary {
    let (title, source, _) = change(number);
    PullRequestSummary {
        repository_key: REPOSITORY.to_owned(),
        repository_name: "site".to_owned(),
        number,
        title: title.to_owned(),
        author: person("1", "Ada Lovelace"),
        state: PullRequestState::Open,
        is_draft: false,
        source_branch: source.to_owned(),
        target_branch: "main".to_owned(),
        created_at: time::macros::datetime!(2026-10-16 09:30 UTC),
        updated_at: Some(time::macros::datetime!(2026-10-18 15:00 UTC)),
        web_url: web_url(number),
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
    merge_strategies: MergeStrategy::ALL.to_vec(),
    has_label_catalog: false,
    running_logs: review_core::RunningLogs::Growing,
});

impl ScriptedProvider {
    fn unused<T>() -> Result<T> {
        Err(ProviderError::Unsupported("Not part of the merge button walkthrough.".to_owned()))
    }
}

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
        Err(ProviderError::Unsupported("No files in this walkthrough.".to_owned()))
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        Ok((1..=3).map(summary).collect())
    }

    async fn pull_request(&self, _repo: &str, number: u64) -> Result<PullRequestDetail> {
        Ok(PullRequestDetail {
            summary: summary(number),
            description: "Completes with the branch's merge types.".to_owned(),
            target_sha: "b".repeat(40),
            head_sha: "a".repeat(40),
            merge_status: change(number).2,
            git_fetch_refs: Vec::new(),
            closed_at: None,
            auto_complete: if number == 3 {
                self.calls.auto_complete.lock().unwrap().clone()
            } else {
                None
            },
        })
    }

    async fn merge_policy(&self, _repo: &str, _target_branch: &str) -> Result<MergePolicy> {
        self.calls.policies.fetch_add(1, Ordering::SeqCst);
        let allowed = vec![MergeStrategy::Merge, MergeStrategy::Squash, MergeStrategy::Rebase];
        Ok(MergePolicy::reported(allowed, Some(MergeStrategy::Squash))
            .with_settings_url(Some(POLICIES.to_owned())))
    }

    async fn merge(&self, _repo: &str, _number: u64, options: &MergeOptions) -> Result<()> {
        self.calls.merges.lock().unwrap().push(options.clone());
        Ok(())
    }

    async fn set_auto_complete(
        &self,
        _repo: &str,
        _number: u64,
        options: &MergeOptions,
    ) -> Result<()> {
        if self.calls.refuses_auto_complete {
            return Err(ProviderError::Unsupported(
                "Auto-merge is turned off for contoso/site.".to_owned(),
            ));
        }
        self.calls.auto_completes.lock().unwrap().push(Some(options.clone()));
        *self.calls.auto_complete.lock().unwrap() = Some(AutoComplete {
            set_by: Some(person("1", "Ada Lovelace")),
            strategy: Some(options.strategy),
        });
        Ok(())
    }

    async fn cancel_auto_complete(&self, _repo: &str, _number: u64) -> Result<()> {
        self.calls.auto_completes.lock().unwrap().push(None);
        *self.calls.auto_complete.lock().unwrap() = None;
        Ok(())
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
