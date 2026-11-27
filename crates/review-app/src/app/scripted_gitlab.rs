//! GitLab as the setup walkthroughs meet it, scripted: a fine-grained token without "Merge
//! Request: Read" and a token with the api scope, the denials explained by the real GitLab
//! provider; and the app started on it with both sign-in tools installed.

use std::sync::Arc;

use async_trait::async_trait;
use egui_kittest::Harness;
use review_core::setup::{Access, Feature, FeatureCheck, ScopeKind};
use review_core::{
    AccessProblem, AccessReport, Capabilities, Connection, MergeStrategy, Provider, ProviderError,
    PullRequestQuery, PullRequestSummary, Result, ReviewVote, ScopeCandidate, UserIdentity,
};
use review_store::AppPaths;

use super::ReviewApp;
use cv_settings::onboarding::CliTools;
use cv_testkit::provider::{Script, Scripted};

/// Scripted tokens; they only ever reach the fake provider below.
pub(super) const LIMITED_TOKEN: &str = "glpat-walkthrough-fine-grained";
pub(super) const COMPLETE_TOKEN: &str = "glpat-walkthrough-complete";

/// The app on `paths` with every connection's provider the scripted GitLab, both sign-in tools as
/// if installed.
pub(super) fn app_on_scripted_gitlab(paths: AppPaths) -> Harness<'static, ReviewApp> {
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

/// The real GitLab provider behind the scripted answers of the module doc; the connection's token
/// picks the limited or the complete script.
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
