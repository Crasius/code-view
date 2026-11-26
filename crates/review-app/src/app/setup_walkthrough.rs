//! Walks the connection wizard and the "Fix token" path like a user, against a scripted provider:
//! no network, no real tokens, no command-line tools. Runs with `cargo test`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::setup::{Access, Feature, FeatureCheck, ScopeKind};
use review_core::{
    AccessProblem, AccessReport, Capabilities, Connection, GitRemote, Label, MergeOptions,
    MergeStrategy, NewThread, PipelineJob, PipelineRun, Provider, ProviderError, ProviderKind,
    PullRequestDetail, PullRequestQuery, PullRequestSummary, RepositoryInfo, Result, ReviewThread,
    ReviewVote, ScopeCandidate, ThreadComment, ThreadStatus, UserIdentity,
};
use review_store::{AppPaths, ConnectionStore, LINUX_HELP_URL, TokenVault, VaultProblem};
use tokio::io::AsyncWrite;

use super::ReviewApp;
use cv_settings::onboarding::CliTools;
use cv_testkit as test_support;

/// Scripted tokens; they only ever reach the fake provider below.
const LIMITED_TOKEN: &str = "glpat-walkthrough-fine-grained";
const COMPLETE_TOKEN: &str = "glpat-walkthrough-complete";
const TIMEOUT: Duration = Duration::from_secs(20);

#[test]
fn setup_wizard_first_run_to_saved_connection() {
    let paths = isolated_paths("first-run");
    let mut harness = harness(paths.clone());

    wait_for(&mut harness, "the welcome screen", |harness| {
        harness.query_by_label_contains("GitLab.com or self-managed").is_some()
    });
    harness.get_by_label_contains("GitLab.com or self-managed").click();
    wait_for(&mut harness, "the host step", |harness| harness.query_by_label("Continue").is_some());
    harness.get_by_label("Continue").click();

    wait_for(&mut harness, "the token step", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    type_token(&mut harness, LIMITED_TOKEN);
    wait_for(&mut harness, "the access report", |harness| {
        harness.query_by_label_contains("Some features won't work").is_some()
    });
    assert!(harness.query_by_label_contains("Needs Merge Request: Read.").is_some());

    harness.get_by_label("Paste a new token").click();
    wait_for(&mut harness, "the token step again", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    type_token(&mut harness, COMPLETE_TOKEN);
    wait_for(&mut harness, "a complete report", |harness| {
        harness.query_by_label_contains("Everything works with this token").is_some()
    });

    harness.get_by_label("Continue").click();
    wait_for(&mut harness, "the groups step", |harness| {
        harness.query_by_label_contains("Only selected groups").is_some()
    });
    harness.get_by_label_contains("Only selected groups").click();
    wait_for(&mut harness, "discovered groups", |harness| {
        harness.query_by_label("TTRPG").is_some()
    });
    harness.get_by_label("TTRPG").click();
    settle(&mut harness);

    harness.get_by_label("Save connection").click();
    wait_for(&mut harness, "the saved confirmation", |harness| {
        harness.query_by_label_contains("is connected").is_some()
    });

    let store = ConnectionStore::open(&paths).expect("saved store");
    let saved = store.all().first().expect("one connection");
    assert_eq!(saved.kind, ProviderKind::GitLab);
    assert_eq!(saved.base_url, "https://gitlab.com");
    assert_eq!(saved.organizations, ["ttrpg"]);
    assert_eq!(saved.name, "ada on gitlab.com");
    assert_eq!(saved.token(), Some(COMPLETE_TOKEN));
}

/// A save that fails (here: a file where the config folder belongs) keeps the wizard on its step
/// with the cause, the file it tried, and "Try again"; nothing is a silent no-op.
#[test]
fn setup_save_failure_stays_on_the_step_with_its_cause() {
    let paths = isolated_paths("save-failure");
    let mut harness = harness(paths.clone());
    wait_for(&mut harness, "the welcome screen", |harness| {
        harness.query_by_label_contains("GitLab.com or self-managed").is_some()
    });
    harness.get_by_label_contains("GitLab.com or self-managed").click();
    wait_for(&mut harness, "the host step", |harness| harness.query_by_label("Continue").is_some());
    harness.get_by_label("Continue").click();
    wait_for(&mut harness, "the token step", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    type_token(&mut harness, COMPLETE_TOKEN);
    wait_for(&mut harness, "a complete report", |harness| {
        harness.query_by_label_contains("Everything works with this token").is_some()
    });
    harness.get_by_label("Continue").click();
    wait_for(&mut harness, "the groups step", |harness| {
        harness.query_by_label("Save connection").is_some()
    });

    test_support::remove_dir(paths.config_dir());
    std::fs::create_dir_all(paths.config_dir().parent().expect("the config folder's parent"))
        .expect("the temp root");
    std::fs::write(paths.config_dir(), b"not a folder").expect("block the config folder");
    harness.get_by_label("Save connection").click();
    wait_for(&mut harness, "the save error", |harness| {
        harness.query_by_label_contains("The connection could not be saved").is_some()
    });
    assert!(
        harness.query_by_label_contains("connections.json").is_some(),
        "the message names the file"
    );
    assert!(harness.query_by_label_contains("is connected").is_none());

    std::fs::remove_file(paths.config_dir()).expect("unblock the config folder");
    press(&mut harness, "Try again");
    wait_for(&mut harness, "the saved confirmation", |harness| {
        harness.query_by_label_contains("is connected").is_some()
    });
    let store = ConnectionStore::open(&paths).expect("saved store");
    assert_eq!(store.all().first().and_then(Connection::token), Some(COMPLETE_TOKEN));
}

/// A token store that cannot keep tokens (a Linux session without a keyring) shows its fix on the
/// token step and again before the save, which then fails with that fix and offers to save the
/// connection without its token.
#[test]
fn setup_missing_keyring_offers_a_tokenless_save() {
    let problem: &'static VaultProblem = Box::leak(Box::new(VaultProblem {
        title: "No keyring answers on this desktop session".to_owned(),
        fix: "Install GNOME Keyring, start it, then check again.".to_owned(),
        commands: vec!["gnome-keyring-daemon --start --components=secrets".to_owned()],
        help_url: LINUX_HELP_URL,
    }));
    let paths = isolated_paths("missing-keyring").with_token_vault(TokenVault::Refusing(problem));
    let mut harness = harness(paths.clone());
    wait_for(&mut harness, "the welcome screen", |harness| {
        harness.query_by_label_contains("GitLab.com or self-managed").is_some()
    });
    harness.get_by_label_contains("GitLab.com or self-managed").click();
    wait_for(&mut harness, "the host step", |harness| harness.query_by_label("Continue").is_some());
    harness.get_by_label("Continue").click();
    wait_for(&mut harness, "the keyring message", |harness| {
        harness.query_by_label_contains(&problem.title).is_some()
    });
    assert!(harness.query_by_label("Copy commands").is_some());
    assert!(
        harness.query_by_label("Save without the token").is_none(),
        "nothing to save on the token step yet"
    );

    type_token(&mut harness, COMPLETE_TOKEN);
    wait_for(&mut harness, "a complete report", |harness| {
        harness.query_by_label_contains("Everything works with this token").is_some()
    });
    harness.get_by_label("Continue").click();
    wait_for(&mut harness, "the groups step", |harness| {
        harness.query_by_label("Save connection").is_some()
    });
    assert!(
        harness.query_by_label_contains(&problem.title).is_some(),
        "the fix shows before the save"
    );

    press(&mut harness, "Save connection");
    settle(&mut harness);
    assert!(
        harness.query_by_label_contains("is connected").is_none(),
        "the refused token keeps the wizard on its step"
    );
    assert!(
        harness.query_by_label_contains("The connection could not be saved").is_none(),
        "the keyring's fix, not a bare error"
    );

    press(&mut harness, "Save without the token");
    wait_for(&mut harness, "the saved confirmation", |harness| {
        harness.query_by_label_contains("is connected").is_some()
    });
    let store = ConnectionStore::open(&paths).expect("saved store");
    let saved = store.all().first().expect("one connection");
    assert_eq!(saved.token(), None);
}

#[test]
fn setup_token_pages_offer_cli_and_checklists() {
    let mut harness = harness(isolated_paths("token-pages"));

    wait_for(&mut harness, "the welcome screen", |harness| {
        harness.query_by_label_contains("GitHub.com or GitHub").is_some()
    });
    harness.get_by_label_contains("GitHub.com or GitHub").click();
    wait_for(&mut harness, "the host step", |harness| harness.query_by_label("Continue").is_some());
    harness.get_by_label("Continue").click();
    wait_for(&mut harness, "the GitHub CLI option", |harness| {
        harness.query_by_label("Use GitHub CLI sign-in").is_some()
    });
    assert!(harness.query_by_label_contains("Create token on github.com").is_some());

    press(&mut harness, &back());
    wait_for(&mut harness, "the host step", |harness| harness.query_by_label(&back()).is_some());
    press(&mut harness, &back());
    wait_for(&mut harness, "the platforms", |harness| {
        harness.query_by_label_contains("Azure DevOps Services or").is_some()
    });
    harness.get_by_label_contains("Azure DevOps Services or").click();
    wait_for(&mut harness, "the organization field", |harness| {
        harness.query_by_role(Role::TextInput).is_some()
    });
    harness.get_by_role(Role::TextInput).focus();
    harness.get_by_role(Role::TextInput).type_text("https://dev.azure.com/contoso/Web/_git/site");
    settle(&mut harness);
    assert!(harness.query_by_label_contains("Connects to dev.azure.com/contoso").is_some());
    harness.get_by_label("Continue").click();
    wait_for(&mut harness, "the Azure checklist", |harness| {
        harness.query_by_label("Code: Read & write").is_some()
    });
    assert!(harness.query_by_label("Use Azure CLI sign-in").is_some());
}

#[test]
fn setup_fix_token_from_a_denied_request() {
    let paths = isolated_paths("fix-token");
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    store
        .upsert(Connection {
            id: "gl-1".into(),
            name: "Tabletop".into(),
            kind: ProviderKind::GitLab,
            base_url: "https://gitlab.com".into(),
            organizations: vec!["ttrpg".into()],
            token: Some(LIMITED_TOKEN.into()),
        })
        .expect("store the connection");
    let mut harness = harness(paths);

    wait_for(&mut harness, "the access banner", |harness| {
        harness.query_by_label_contains("Tabletop: The fine-grained token").is_some()
    });
    harness.get_by_label_contains("Fix token").click();
    wait_for(&mut harness, "the fix screen", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    assert!(
        harness
            .query_by_label_contains("The fine-grained token lacks Merge Request: Read.")
            .is_some()
    );
}

/// The wizard's Back button (its arrow icon is painted, not part of the name).
fn back() -> String {
    "Back".to_owned()
}

pub(super) fn harness(paths: AppPaths) -> Harness<'static, ReviewApp> {
    Harness::builder().with_size([1280.0, 1000.0]).with_max_steps(100_000).build_eframe(
        |creation| {
            let mut app = ReviewApp::new(creation, paths, fake_provider);
            // The machine's own gh/az never run in tests; both options are shown as if installed.
            // Set before the harness's first frame, which opens the setup with the tools it has.
            app.settings.cli_tools = Some(CliTools { github: true, azure: true });
            app
        },
    )
}

pub(super) fn isolated_paths(name: &str) -> AppPaths {
    let root = std::env::temp_dir().join(format!("review-setup-{name}-{}", std::process::id()));
    test_support::remove_dir(&root);
    AppPaths::new(root.join("config"), root.join("cache"))
}

/// Scrolls the labelled widget into view, then clicks it like a pointer would.
pub(super) fn press(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    harness.get_by_label(label).scroll_to_me();
    settle(harness);
    harness.get_by_label(label).click();
    harness.step();
}

fn type_token(harness: &mut Harness<'_, ReviewApp>, token: &str) {
    harness.get_by_role(Role::PasswordInput).focus();
    harness.step();
    harness.get_by_role(Role::PasswordInput).type_text(token);
    harness.step();
}

pub(super) fn wait_for(
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
        assert!(started.elapsed() < TIMEOUT, "timed out waiting for {what}");
        test_support::let_threads_run(Duration::from_millis(20));
    }
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..4 {
        harness.step();
    }
}

fn fake_provider(connection: Connection, http: reqwest::Client) -> Arc<dyn Provider> {
    Arc::new(ScriptedGitLab {
        gitlab: provider_gitlab::GitLabProvider::new(connection.clone(), http),
        connection,
    })
}

/// GitLab as the user met it: a fine-grained token without "Merge Request: Read", then a token
/// with the api scope. Denials are explained by the real GitLab provider.
struct ScriptedGitLab {
    gitlab: provider_gitlab::GitLabProvider,
    connection: Connection,
}

impl ScriptedGitLab {
    fn is_complete(&self) -> bool {
        self.connection.token() == Some(COMPLETE_TOKEN)
    }

    fn denied(path: &str) -> ProviderError {
        ProviderError::Status {
            status: 403,
            path: path.to_owned(),
            body: r#"{"message":"Access denied: This operation requires a fine-grained personal access token with the following project permissions: [Merge Request: Read]."}"#.to_owned(),
        }
    }

    fn unused<T>() -> Result<T> {
        Err(ProviderError::Unsupported("Not part of the setup walkthrough.".to_owned()))
    }
}

fn user() -> UserIdentity {
    UserIdentity {
        id: "7".into(),
        display_name: "Ada Lovelace".into(),
        login: Some("ada".into()),
        avatar_url: None,
    }
}

static CAPABILITIES: std::sync::LazyLock<Capabilities> =
    std::sync::LazyLock::new(|| Capabilities {
        votes: vec![ReviewVote::Approved, ReviewVote::None],
        merge_strategies: vec![MergeStrategy::Merge],
        has_label_catalog: true,
        running_logs: review_core::RunningLogs::Growing,
    });

#[async_trait]
impl Provider for ScriptedGitLab {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(user())
    }

    async fn list_repositories(&self, _search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        Ok(Vec::new())
    }

    async fn git_remote(&self, _repo: &str) -> Result<GitRemote> {
        Self::unused()
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        if self.is_complete() {
            Ok(Vec::new())
        } else {
            Err(Self::denied("/api/v4/merge_requests"))
        }
    }

    async fn pull_request(&self, _repo: &str, _number: u64) -> Result<PullRequestDetail> {
        Self::unused()
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
        Self::unused()
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
        Self::unused()
    }

    async fn set_labels(&self, _repo: &str, _number: u64, _labels: &[String]) -> Result<()> {
        Self::unused()
    }

    async fn pipeline_runs(&self, _repo: &str, _number: u64) -> Result<Vec<PipelineRun>> {
        Self::unused()
    }

    async fn pipeline_jobs(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        Self::unused()
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

    async fn inspect_access(&self) -> Result<AccessReport> {
        let checks = if self.is_complete() {
            Feature::ALL
                .iter()
                .map(|feature| {
                    FeatureCheck::new(*feature, Access::Granted, "Granted by the api scope.")
                })
                .collect()
        } else {
            vec![
                FeatureCheck::new(
                    Feature::ReadPullRequests,
                    Access::Missing,
                    "Needs Merge Request: Read.",
                ),
                FeatureCheck::new(Feature::Vote, Access::Missing, "Needs Merge Request: Approve."),
                FeatureCheck::new(
                    Feature::Comment,
                    Access::Unverified,
                    "Needs Merge Request: Create and Merge Request: Update. Only a write could confirm it, so make sure it is selected.",
                ),
                FeatureCheck::new(
                    Feature::Labels,
                    Access::Unverified,
                    "Needs Merge Request: Update. Only a write could confirm it, so make sure it is selected.",
                ),
                FeatureCheck::new(Feature::Merge, Access::Missing, "Needs Merge Request: Merge."),
                FeatureCheck::new(
                    Feature::Pipelines,
                    Access::Limited,
                    "Pipelines show, but job logs need Job: Read.",
                ),
                FeatureCheck::new(Feature::GitClone, Access::Granted, "Code: Download granted."),
            ]
        };
        let token_kind = if self.is_complete() {
            "Personal access token"
        } else {
            "Fine-grained personal access token"
        };
        Ok(AccessReport {
            user: user(),
            token_kind: token_kind.to_owned(),
            scopes: Vec::new(),
            expires_at: Some("2027-10-18".to_owned()),
            checks,
            notes: Vec::new(),
        })
    }

    async fn discover_scopes(&self) -> Result<Vec<ScopeCandidate>> {
        Ok(vec![
            ScopeCandidate {
                key: "ttrpg".into(),
                display_name: "TTRPG".into(),
                kind: ScopeKind::Group,
            },
            ScopeCandidate {
                key: "tools".into(),
                display_name: "Tools".into(),
                kind: ScopeKind::Group,
            },
        ])
    }

    fn explain_denial(&self, error: &ProviderError) -> Option<AccessProblem> {
        self.gitlab.explain_denial(error)
    }
}
