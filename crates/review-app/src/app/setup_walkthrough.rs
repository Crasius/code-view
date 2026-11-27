//! Walks the connection wizard and the "Fix token" path like a user, against a scripted provider:
//! no network, no real tokens, no command-line tools. Runs with `cargo test`.

use std::sync::Arc;

use async_trait::async_trait;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::setup::{Access, Feature, FeatureCheck, ScopeKind};
use review_core::{
    AccessProblem, AccessReport, Capabilities, Connection, MergeStrategy, Provider, ProviderError,
    ProviderKind, PullRequestQuery, PullRequestSummary, Result, ReviewVote, ScopeCandidate,
    UserIdentity,
};
use review_store::{AppPaths, ConnectionStore, LINUX_HELP_URL, TokenVault, VaultProblem};

use super::ReviewApp;
use super::walkthrough::{press, saved_paths, settle_background, wait_until};
use cv_settings::onboarding::CliTools;
use cv_testkit::provider::{Script, Scripted};
use cv_testkit::{Clicks, ScratchDir, samples};

/// Scripted tokens; they only ever reach the fake provider below.
const LIMITED_TOKEN: &str = "glpat-walkthrough-fine-grained";
const COMPLETE_TOKEN: &str = "glpat-walkthrough-complete";

#[test]
fn setup_wizard_first_run_to_saved_connection() {
    let scratch = ScratchDir::new("setup-first-run");
    let paths = scratch.app_paths();
    let mut harness = harness(paths.clone());

    to_gitlab_token_step(&mut harness);

    wait_until(&mut harness, "the token step", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    type_token(&mut harness, LIMITED_TOKEN);
    wait_until(&mut harness, "the access report", |harness| {
        harness.query_by_label_contains("Some features won't work").is_some()
    });
    assert!(harness.query_by_label_contains("Needs Merge Request: Read.").is_some());

    harness.click_at(harness.get_by_label("Paste a new token").rect());
    wait_until(&mut harness, "the token step again", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    complete_token_to_groups(&mut harness);
    harness.click_at(harness.get_by_label_contains("Only selected groups").rect());
    wait_until(&mut harness, "discovered groups", |harness| {
        harness.query_by_label("TTRPG").is_some()
    });
    harness.click_at(harness.get_by_label("TTRPG").rect());
    harness.run_steps(4);

    harness.click_at(harness.get_by_label("Save connection").rect());
    wait_until(&mut harness, "the saved confirmation", |harness| {
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
    let scratch = ScratchDir::new("setup-save-failure");
    let paths = scratch.app_paths();
    let mut harness = harness(paths.clone());
    to_gitlab_token_step(&mut harness);
    wait_until(&mut harness, "the token step", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    complete_token_to_groups(&mut harness);

    cv_testkit::remove_dir(paths.config_dir());
    std::fs::create_dir_all(paths.config_dir().parent().expect("the config folder's parent"))
        .expect("the temp root");
    std::fs::write(paths.config_dir(), b"not a folder").expect("block the config folder");
    harness.click_at(harness.get_by_label("Save connection").rect());
    wait_until(&mut harness, "the save error", |harness| {
        harness.query_by_label_contains("The connection could not be saved").is_some()
    });
    assert!(
        harness.query_by_label_contains("connections.json").is_some(),
        "the message names the file"
    );
    assert!(harness.query_by_label_contains("is connected").is_none());

    std::fs::remove_file(paths.config_dir()).expect("unblock the config folder");
    press(&mut harness, "Try again");
    wait_until(&mut harness, "the saved confirmation", |harness| {
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
    let scratch = ScratchDir::new("setup-missing-keyring");
    let paths = scratch.app_paths().with_token_vault(TokenVault::Refusing(problem));
    let mut harness = harness(paths.clone());
    to_gitlab_token_step(&mut harness);
    wait_until(&mut harness, "the keyring message", |harness| {
        harness.query_by_label_contains(&problem.title).is_some()
    });
    assert!(harness.query_by_label("Copy commands").is_some());
    assert!(
        harness.query_by_label("Save without the token").is_none(),
        "nothing to save on the token step yet"
    );

    complete_token_to_groups(&mut harness);
    assert!(
        harness.query_by_label_contains(&problem.title).is_some(),
        "the fix shows before the save"
    );

    press(&mut harness, "Save connection");
    settle_background(&mut harness);
    assert!(
        harness.query_by_label_contains("is connected").is_none(),
        "the refused token keeps the wizard on its step"
    );
    assert!(
        harness.query_by_label_contains("The connection could not be saved").is_none(),
        "the keyring's fix, not a bare error"
    );

    press(&mut harness, "Save without the token");
    wait_until(&mut harness, "the saved confirmation", |harness| {
        harness.query_by_label_contains("is connected").is_some()
    });
    let store = ConnectionStore::open(&paths).expect("saved store");
    let saved = store.all().first().expect("one connection");
    assert_eq!(saved.token(), None);
}

#[test]
fn setup_token_pages_offer_cli_and_checklists() {
    let scratch = ScratchDir::new("setup-token-pages");
    let mut harness = harness(scratch.app_paths());

    wait_until(&mut harness, "the welcome screen", |harness| {
        harness.query_by_label_contains("GitHub.com or GitHub").is_some()
    });
    harness.click_at(harness.get_by_label_contains("GitHub.com or GitHub").rect());
    wait_until(&mut harness, "the host step", |harness| {
        harness.query_by_label("Continue").is_some()
    });
    harness.click_at(harness.get_by_label("Continue").rect());
    wait_until(&mut harness, "the GitHub CLI option", |harness| {
        harness.query_by_label("Use GitHub CLI sign-in").is_some()
    });
    assert!(harness.query_by_label_contains("Create token on github.com").is_some());

    press(&mut harness, BACK);
    wait_until(&mut harness, "the host step", |harness| harness.query_by_label(BACK).is_some());
    press(&mut harness, BACK);
    wait_until(&mut harness, "the platforms", |harness| {
        harness.query_by_label_contains("Azure DevOps Services or").is_some()
    });
    harness.click_at(harness.get_by_label_contains("Azure DevOps Services or").rect());
    wait_until(&mut harness, "the organization field", |harness| {
        harness.query_by_role(Role::TextInput).is_some()
    });
    harness.get_by_role(Role::TextInput).focus();
    harness.get_by_role(Role::TextInput).type_text("https://dev.azure.com/contoso/Web/_git/site");
    harness.run_steps(4);
    assert!(harness.query_by_label_contains("Connects to dev.azure.com/contoso").is_some());
    harness.click_at(harness.get_by_label("Continue").rect());
    wait_until(&mut harness, "the Azure checklist", |harness| {
        harness.query_by_label("Code: Read & write").is_some()
    });
    assert!(harness.query_by_label("Use Azure CLI sign-in").is_some());
}

#[test]
fn setup_fix_token_from_a_denied_request() {
    let scratch = ScratchDir::new("setup-fix-token");
    let connection = Connection {
        name: "Tabletop".into(),
        organizations: vec!["ttrpg".into()],
        token: Some(LIMITED_TOKEN.into()),
        ..samples::connection("gl-1", ProviderKind::GitLab, "https://gitlab.com")
    };
    let mut harness = harness(saved_paths(&scratch, &[connection]));

    wait_until(&mut harness, "the access banner", |harness| {
        harness.query_by_label_contains("Tabletop: The fine-grained token").is_some()
    });
    harness.click_at(harness.get_by_label_contains("Fix token").rect());
    wait_until(&mut harness, "the fix screen", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    assert!(
        harness
            .query_by_label_contains("The fine-grained token lacks Merge Request: Read.")
            .is_some()
    );
}

/// The wizard's Back button (its arrow icon is painted, not part of the name).
const BACK: &str = "Back";

pub(super) fn harness(paths: AppPaths) -> Harness<'static, ReviewApp> {
    super::startup::start(Harness::builder().with_size([1280.0, 1000.0]), |creation| {
        let mut app = ReviewApp::new(creation, paths, |connection: Connection, http| {
            let gitlab = provider_gitlab::GitLabProvider::new(connection.clone(), http);
            Arc::new(Scripted(ScriptedGitLab { gitlab, connection })) as Arc<dyn Provider>
        });
        // Both sign-in tools as if installed, before the harness's first frame, which opens the
        // setup with the tools it has.
        app.settings.set_cli_tools(CliTools { github: true, azure: true });
        app
    })
}

fn type_token(harness: &mut Harness<'_, ReviewApp>, token: &str) {
    harness.get_by_role(Role::PasswordInput).focus();
    harness.step();
    harness.get_by_role(Role::PasswordInput).type_text(token);
    harness.step();
}

/// From the welcome screen through GitLab's host step to its token step.
fn to_gitlab_token_step(harness: &mut Harness<'_, ReviewApp>) {
    wait_until(harness, "the welcome screen", |harness| {
        harness.query_by_label_contains("GitLab.com or self-managed").is_some()
    });
    harness.click_at(harness.get_by_label_contains("GitLab.com or self-managed").rect());
    wait_until(harness, "the host step", |harness| harness.query_by_label("Continue").is_some());
    harness.click_at(harness.get_by_label("Continue").rect());
}

/// Types the complete token on the token step and goes on to the groups step.
fn complete_token_to_groups(harness: &mut Harness<'_, ReviewApp>) {
    type_token(harness, COMPLETE_TOKEN);
    wait_until(harness, "a complete report", |harness| {
        harness.query_by_label_contains("Everything works with this token").is_some()
    });
    harness.click_at(harness.get_by_label("Continue").rect());
    wait_until(harness, "the groups step", |harness| {
        harness.query_by_label("Save connection").is_some()
    });
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
impl Script for ScriptedGitLab {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(user())
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
