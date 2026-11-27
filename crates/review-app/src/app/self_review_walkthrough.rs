//! "Add me as reviewer", offline: a GitHub connection answered by a scripted provider that
//! signs in as Grace and keeps the reviewers it is asked to add. Ada authored pull requests 1, 2
//! and 4; Grace authored 3.
//!
//! Checks the Overview's Reviewers card (click and Alt+R): one request with Grace's login, then
//! Grace in the card's list without a vote and the button gone; none for the author; the list
//! row's context menu doing the same without opening the pull request; a connection without a
//! token showing the button off with the reason and the way to a token; and a refused request
//! keeping the button with the reason under it and a retry that works. The header offers none.
//! While an add runs, a skeleton row of the new row's size stands where it lands.
//! `cargo test -p review-app self_review`

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{
    Capabilities, Connection, GitRemote, Label, MergeOptions, MergeStrategy, NewThread,
    PipelineJob, PipelineRun, Provider, ProviderError, ProviderKind, PullRequestDetail,
    PullRequestQuery, PullRequestState, PullRequestSummary, RepositoryInfo, Result, ReviewThread,
    ReviewVote, Reviewer, ThreadComment, ThreadStatus, UserIdentity,
};
use review_store::{AppPaths, ConnectionStore};
use tokio::io::AsyncWrite;

use super::ReviewApp;
use super::walkthrough::{has, press_button, pump, secondary_press, wait_until};
use cv_runtime::route::{PullRequestKey, Route};
use cv_services::self_review;
use cv_settings::onboarding::CliTools;
use cv_testkit::Clicks;

const REPOSITORY: &str = "contoso/site";
const GRACE: &str = "grace";

/// What a connection's scripted provider was asked to add, the reviewers it keeps per pull
/// request, and whether its next add is refused. Each test has its own connection.
#[derive(Default)]
struct Calls {
    adds: Mutex<Vec<(u64, String)>>,
    reviewers: Mutex<HashMap<u64, Vec<Reviewer>>>,
    refuses_next: AtomicBool,
    /// Adds wait while this is set, so a test can look at the card meanwhile.
    holds_adds: AtomicBool,
}

static CALLS: LazyLock<Mutex<HashMap<String, Arc<Calls>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[test]
fn self_review_adds_me_from_the_reviewers_card_once() {
    let (mut harness, calls) = start("card", true);
    // Whether field and button fit side by side is the fonts' width; the bundled ones are the
    // same everywhere.
    cv_testkit::use_bundled_fonts(&harness.ctx);
    open(&mut harness, "card", 1);
    wait_until(&mut harness, "the button", |harness| button(harness, self_review::LABEL));
    assert!(has(&harness, "Reviewers") && !has(&harness, "Grace Hopper"), "not listed yet");
    assert!(
        harness.query_all_by_role_and_label(Role::Button, self_review::LABEL).count() == 1,
        "one home: the card, not the header"
    );
    assert_beside_the_add_field(&harness);

    press_button(&mut harness, self_review::LABEL);
    wait_until(&mut harness, "Grace in the reviewer card", |harness| {
        has(harness, "Grace Hopper") && has(harness, "No vote")
    });
    assert!(
        harness.query_by_label("No vote").is_some(),
        "the vote reads as words alone; its icon sits on the avatar"
    );
    assert_eq!(*calls.adds.lock().unwrap(), [(1, GRACE.to_owned())], "one request, Grace's login");
    assert!(!button(&harness, self_review::LABEL), "the button goes once Grace is listed");

    // Alt+R does the same on another pull request.
    open(&mut harness, "card", 2);
    wait_until(&mut harness, "the second button", |harness| {
        has(harness, "Second change") && button(harness, self_review::LABEL)
    });
    harness.key_press_modifiers(egui::Modifiers::ALT, egui::Key::R);
    wait_until(&mut harness, "Grace on the second pull request", |harness| {
        has(harness, "Grace Hopper") && !button(harness, self_review::LABEL)
    });
    assert_eq!(calls.adds.lock().unwrap().len(), 2, "Alt+R sent one request");

    // The author is not offered a review of their own pull request.
    open(&mut harness, "card", 3);
    wait_until(&mut harness, "Grace's own pull request", |harness| has(harness, "Grace's change"));
    harness.run_steps(10);
    assert!(!button(&harness, self_review::LABEL));
    assert_eq!(calls.adds.lock().unwrap().len(), 2, "nothing asked for the author");
}

#[test]
fn self_review_from_a_list_row_marks_the_row_without_opening_it() {
    let (mut harness, calls) = start("list", true);
    wait_until(&mut harness, "the list", |harness| has(harness, "Fourth change"));
    secondary_press(&mut harness, "4 Fourth change");
    wait_until(&mut harness, "the menu entry", |harness| {
        harness
            .query_by_role_and_label(Role::MenuItem, self_review::LABEL)
            .is_some_and(|node| !node.accesskit_node().is_disabled())
    });
    harness.click_at(harness.get_by_role_and_label(Role::MenuItem, self_review::LABEL).rect());
    wait_until(&mut harness, "the request", |_| !calls.adds.lock().unwrap().is_empty());
    assert_eq!(*calls.adds.lock().unwrap(), [(4, GRACE.to_owned())]);
    wait_until(&mut harness, "the toast", |harness| has(harness, self_review::DONE));
    assert_eq!(harness.state().route, Route::MyPullRequests, "the list stays");

    secondary_press(&mut harness, "4 Fourth change");
    wait_until(&mut harness, "the pending entry", |harness| {
        harness
            .query_by_role_and_label(Role::MenuItem, self_review::PENDING_LABEL)
            .is_some_and(|node| node.accesskit_node().is_disabled())
    });
    assert!(harness.query_by_role_and_label(Role::MenuItem, self_review::LABEL).is_none());
}

#[test]
fn self_review_is_off_with_its_reason_without_a_token() {
    let (mut harness, calls) = start("read-only", false);
    open(&mut harness, "read-only", 1);
    wait_until(&mut harness, "the button", |harness| button(harness, self_review::LABEL));
    let node = harness.get_by_role_and_label(Role::Button, self_review::LABEL);
    assert!(node.accesskit_node().is_disabled(), "no token, no request");
    harness.get_by_role_and_label(Role::Button, self_review::LABEL).hover();
    pump(&mut harness, 1.5);
    assert!(
        has(&harness, "This connection has no token"),
        "the tooltip says why and how to fix it"
    );
    assert!(button(&harness, "Add a token"), "with the way to a token beside it");
    harness.key_press_modifiers(egui::Modifiers::ALT, egui::Key::R);
    harness.run_steps(5);
    assert!(calls.adds.lock().unwrap().is_empty(), "Alt+R asks nothing either");
    press_button(&mut harness, "Add a token");
    wait_until(&mut harness, "the connection's setup", |harness| {
        harness.state().route == Route::Settings
    });

    // A row's menu says the same and leads to the token.
    harness.state_mut().navigator.go(Route::MyPullRequests);
    wait_until(&mut harness, "the list", |harness| has(harness, "Fourth change"));
    secondary_press(&mut harness, "4 Fourth change");
    wait_until(&mut harness, "the menu", |harness| {
        harness
            .query_by_role_and_label(Role::MenuItem, self_review::LABEL)
            .is_some_and(|node| node.accesskit_node().is_disabled())
    });
    harness.click_at(harness.get_by_role_and_label(Role::MenuItem, "Add a token to review").rect());
    wait_until(&mut harness, "the connection's setup", |harness| {
        harness.state().route == Route::Settings
    });
    assert!(calls.adds.lock().unwrap().is_empty());
}

#[test]
fn self_review_keeps_a_refused_request_retryable_with_its_reason() {
    let (mut harness, calls) = start("refused", true);
    calls.refuses_next.store(true, Ordering::SeqCst);
    open(&mut harness, "refused", 1);
    wait_until(&mut harness, "the button", |harness| button(harness, self_review::LABEL));
    press_button(&mut harness, self_review::LABEL);
    wait_until(&mut harness, "the error toast", |harness| has(harness, "must be collaborators"));
    assert!(skeleton_row(&harness).is_none(), "the refused add leaves no skeleton row");
    assert!(button(&harness, self_review::LABEL), "the button stays to try again");
    let inline = "Could not add you as a reviewer: GitHub did not";
    assert!(has(&harness, inline), "the reason, inline under the button");
    harness.get_by_role_and_label(Role::Button, self_review::LABEL).hover();
    pump(&mut harness, 1.5);
    assert!(has(&harness, "Click to try again"), "its tooltip offers the retry");

    press_button(&mut harness, self_review::LABEL);
    wait_until(&mut harness, "Grace in the reviewer card", |harness| {
        has(harness, "Grace Hopper") && !button(harness, self_review::LABEL)
    });
    assert_eq!(calls.adds.lock().unwrap().len(), 2, "refused once, then added");
    assert!(!has(&harness, "Could not add you as a reviewer"), "the reason goes with the retry");
}

/// Regression: a refused add from a list row ended in a toast with no way out. Its toast now
/// offers Retry, which sends the same add once more, and the pull request on GitHub.
#[test]
fn self_review_refused_row_request_retries_from_its_toast() {
    let (mut harness, calls) = start("row-refused", true);
    calls.refuses_next.store(true, Ordering::SeqCst);
    wait_until(&mut harness, "the list", |harness| has(harness, "Fourth change"));
    secondary_press(&mut harness, "4 Fourth change");
    wait_until(&mut harness, "the menu entry", |harness| {
        harness.query_by_role_and_label(Role::MenuItem, self_review::LABEL).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::MenuItem, self_review::LABEL).rect());
    wait_until(&mut harness, "the failure toast", |harness| button(harness, "Retry"));
    assert!(button(&harness, "Open on GitHub"), "the toast leads to the pull request on GitHub");
    assert_eq!(calls.adds.lock().unwrap().len(), 1);

    press_button(&mut harness, "Retry");
    wait_until(&mut harness, "the retried add", |harness| has(harness, self_review::DONE));
    harness.run_steps(10);
    assert_eq!(
        *calls.adds.lock().unwrap(),
        [(4, GRACE.to_owned()), (4, GRACE.to_owned())],
        "Retry sent the same add once"
    );
    assert_eq!(harness.state().route, Route::MyPullRequests, "the list stays");
}

/// Regression: nothing showed between the click and the refreshed list. While the add runs, a
/// skeleton row stands where Grace lands, exactly as large as her row, which then takes its place
/// without moving anything below.
#[test]
fn self_review_holds_the_new_row_with_a_skeleton_of_its_size() {
    let (mut harness, calls) = start("skeleton", true);
    calls.holds_adds.store(true, Ordering::SeqCst);
    open(&mut harness, "skeleton", 1);
    wait_until(&mut harness, "the button", |harness| button(harness, self_review::LABEL));
    press_button(&mut harness, self_review::LABEL);
    wait_until(&mut harness, "the skeleton row", |harness| skeleton_row(harness).is_some());
    let placeholder = skeleton_row(&harness).expect("the skeleton row");
    let field = add_field(&harness, placeholder.bottom());

    calls.holds_adds.store(false, Ordering::SeqCst);
    let grace = "Reviewer Grace Hopper";
    wait_until(&mut harness, "Grace's row", |harness| harness.query_by_label(grace).is_some());
    assert!(skeleton_row(&harness).is_none(), "Grace's row replaced the skeleton");
    let row = harness.get_by_label(grace).rect();
    let gaps = [row.min - placeholder.min, row.max - placeholder.max];
    assert!(gaps.iter().all(|gap| gap.length() <= 0.5), "{placeholder:?} became {row:?}");
    let moved = add_field(&harness, row.bottom()).top() - field.top();
    assert!(moved.abs() <= 0.5, "the add field moved by {moved}");
}

/// The skeleton row of an add in flight.
fn skeleton_row(harness: &Harness<'_, ReviewApp>) -> Option<egui::Rect> {
    harness
        .query_by_role_and_label(Role::ProgressIndicator, "Adding reviewer")
        .map(|node| node.rect())
}

/// The first text field below `top`: the Reviewers card's add field under its list.
fn add_field(harness: &Harness<'_, ReviewApp>, top: f32) -> egui::Rect {
    harness
        .query_all_by_role(Role::TextInput)
        .map(|node| node.rect())
        .filter(|field| field.top() > top)
        .min_by(|a, b| a.top().total_cmp(&b.top()))
        .expect("the add field below the list")
}

/// Regression: the button had a row of its own above the list. It sits in the "Add reviewer"
/// field's row, at its right end with a gap, both one control tall.
fn assert_beside_the_add_field(harness: &Harness<'_, ReviewApp>) {
    let button = harness.get_by_role_and_label(Role::Button, self_review::LABEL).rect();
    let field = harness
        .query_all_by_role(Role::TextInput)
        .map(|node| node.rect())
        .find(|field| field.right() < button.left() && field.bottom() > button.top())
        .expect("the add field beside the button");
    assert!((field.top() - button.top()).abs() <= 1.0, "tops: {field:?} and {button:?}");
    assert!((field.center().y - button.center().y).abs() <= 1.0, "centres: {field:?} {button:?}");
    let gap = button.left() - field.right();
    assert!((12.0..=16.0).contains(&gap), "a generous gap between them, not {gap}");
}

/// The app with one GitHub connection with id `name`, with or without a token.
fn start(name: &str, has_token: bool) -> (Harness<'static, ReviewApp>, Arc<Calls>) {
    let calls = Arc::new(Calls::default());
    CALLS.lock().unwrap().insert(name.to_owned(), calls.clone());
    let root =
        std::env::temp_dir().join(format!("review-self-review-{name}-{}", std::process::id()));
    cv_testkit::remove_dir(&root);
    let paths = AppPaths::new(root.join("config"), root.join("cache"));
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    store
        .upsert(Connection {
            id: name.to_owned(),
            name: "Contoso".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: "https://github.com".to_owned(),
            organizations: vec!["contoso".to_owned()],
            token: has_token.then(|| "scripted".to_owned()),
        })
        .expect("store the connection");
    let builder = Harness::builder().with_size([1280.0, 860.0]).with_step_dt(1.0 / 30.0);
    let mut harness = super::startup::start(builder, move |creation| {
        ReviewApp::new(creation, paths, scripted_provider)
    });
    harness.state_mut().settings.cli_tools = Some(CliTools::default());
    (harness, calls)
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

fn button(harness: &Harness<'_, ReviewApp>, label: &str) -> bool {
    harness.query_by_role_and_label(Role::Button, label).is_some()
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "a ProviderFactory, whose signature hands the connection and the client over"
)]
fn scripted_provider(connection: Connection, _http: reqwest::Client) -> Arc<dyn Provider> {
    let calls = CALLS.lock().unwrap().get(&connection.id).cloned().unwrap_or_default();
    Arc::new(ScriptedProvider { calls, has_token: connection.token.is_some() })
}

struct ScriptedProvider {
    calls: Arc<Calls>,
    has_token: bool,
}

fn person(id: &str, name: &str, login: &str) -> UserIdentity {
    UserIdentity {
        id: id.to_owned(),
        display_name: name.to_owned(),
        login: Some(login.to_owned()),
        avatar_url: None,
    }
}

fn grace() -> UserIdentity {
    person("9", "Grace Hopper", GRACE)
}

fn summary(number: u64, reviewers: Vec<Reviewer>) -> PullRequestSummary {
    let (title, author) = match number {
        1 => ("First change", person("1", "Ada Lovelace", "ada")),
        2 => ("Second change", person("1", "Ada Lovelace", "ada")),
        3 => ("Grace's change", grace()),
        _ => ("Fourth change", person("1", "Ada Lovelace", "ada")),
    };
    PullRequestSummary {
        repository_key: REPOSITORY.to_owned(),
        repository_name: "site".to_owned(),
        number,
        title: title.to_owned(),
        author,
        state: PullRequestState::Open,
        is_draft: false,
        source_branch: format!("feature/{number}"),
        target_branch: "main".to_owned(),
        created_at: time::macros::datetime!(2026-10-16 09:30 UTC),
        updated_at: Some(time::macros::datetime!(2026-10-18 15:00 UTC)),
        web_url: format!("https://github.com/{REPOSITORY}/pull/{number}"),
        labels: Vec::new(),
        reviewers,
        checks: None,
        diff_stat: review_core::DiffStat::default(),
        head_sha: None,
        target_sha: None,
    }
}

static CAPABILITIES: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    votes: vec![ReviewVote::Approved, ReviewVote::ChangesRequested],
    merge_strategies: vec![MergeStrategy::Merge],
    has_label_catalog: true,
    running_logs: review_core::RunningLogs::WhenFinished,
});

impl ScriptedProvider {
    fn unused<T>() -> Result<T> {
        Err(ProviderError::Unsupported("Not part of the self review walkthrough.".to_owned()))
    }

    fn reviewers(&self, number: u64) -> Vec<Reviewer> {
        self.calls.reviewers.lock().unwrap().get(&number).cloned().unwrap_or_default()
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        if self.has_token {
            Ok(grace())
        } else {
            Err(ProviderError::requires_token("Reading the current user"))
        }
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
        Ok((1..=4).rev().map(|number| summary(number, self.reviewers(number))).collect())
    }

    async fn pull_request(&self, _repo: &str, number: u64) -> Result<PullRequestDetail> {
        Ok(PullRequestDetail {
            summary: summary(number, self.reviewers(number)),
            description: "Waits for a reviewer.".to_owned(),
            target_sha: "b".repeat(40),
            head_sha: "a".repeat(40),
            merge_status: review_core::MergeStatus::Mergeable,
            git_fetch_refs: Vec::new(),
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

    async fn add_reviewer(&self, _repo: &str, number: u64, user: &str) -> Result<()> {
        self.calls.adds.lock().unwrap().push((number, user.to_owned()));
        while self.calls.holds_adds.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if self.calls.refuses_next.swap(false, Ordering::SeqCst) {
            return Err(ProviderError::Local(
                "GitHub did not request a review from grace: reviewers must be collaborators on \
                 contoso/site."
                    .to_owned(),
            ));
        }
        let reviewer = Reviewer {
            identity: grace(),
            vote: ReviewVote::None,
            is_required: false,
            is_group: false,
        };
        self.calls.reviewers.lock().unwrap().entry(number).or_default().push(reviewer);
        Ok(())
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
