//! A scene for walkthroughs that are not about HTTP: one scripted provider that answers as Azure
//! DevOps, GitHub or self-managed GitLab, whichever its connection is, and a local git origin a
//! scripted pull request is fetched from. The provider icons, frame rate and settings walkthroughs
//! start on the script; the navigation and provider icons walkthroughs on the origin.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use review_core::{
    Capabilities, Connection, GitRemote, MergeStatus, Provider, ProviderError, ProviderKind,
    PullRequestDetail, PullRequestQuery, PullRequestState, PullRequestSummary, Result, RunStatus,
    RunningLogs, UserIdentity,
};

use cv_testkit::provider::{Script, Scripted};
use cv_testkit::{ScratchDir, git};

/// (id, name, platform, web root, repository, accessible name of its mark).
pub(super) const HOSTS: [(&str, &str, ProviderKind, &str, &str, &str); 3] = [
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

/// The title of pull request `number` on the platform `kind`.
pub(super) fn title(kind: ProviderKind, number: u64) -> String {
    format!("{} change {number}", kind.label())
}

/// A local repository with one changed README: `target` is `main`, `head` is `feature`.
#[derive(Clone)]
pub(super) struct Origin {
    pub(super) clone_url: String,
    pub(super) target: String,
    pub(super) head: String,
}

/// The [`Origin`], built once per test run: no test pushes to it. The folder stays for the run;
/// the next run's [`ScratchDir::new`] empties it.
pub(super) fn origin() -> Origin {
    static REMOTE: LazyLock<(ScratchDir, Origin)> = LazyLock::new(|| {
        let folder = ScratchDir::new("scripted-origin");
        let origin = create_origin(folder.path());
        (folder, origin)
    });
    REMOTE.1.clone()
}

fn create_origin(origin: &Path) -> Origin {
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
    Origin { clone_url: cv_testkit::file_url(origin), target, head }
}

/// The provider factory of [`HOSTS`]' connections. GitHub's pull requests are fetched from
/// `origin` when there is one; the other platforms have no files to show.
pub(super) fn platforms(
    origin: Option<Origin>,
) -> impl Fn(Connection, reqwest::Client) -> Arc<dyn Provider> + Send + Sync + 'static {
    move |connection, _| {
        let Some(&(_, _, _, _, repository, _)) = HOSTS.iter().find(|(id, ..)| *id == connection.id)
        else {
            panic!("no script for connection {}", connection.id)
        };
        let origin = origin.clone().filter(|_| connection.kind == ProviderKind::GitHub);
        Arc::new(Scripted(PlatformScript { connection, repository, origin }))
    }
}

/// Answers listings and pull requests of its connection's platform.
struct PlatformScript {
    connection: Connection,
    repository: &'static str,
    origin: Option<Origin>,
}

impl PlatformScript {
    fn summary(&self, number: u64) -> PullRequestSummary {
        let kind = self.connection.kind;
        let repository = self.repository;
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
            author: author(),
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
}

/// The author of every pull request and the signed-in user.
fn author() -> UserIdentity {
    UserIdentity {
        id: "1".to_owned(),
        display_name: "Ada Lovelace".to_owned(),
        login: Some("ada lovelace".to_owned()),
        avatar_url: None,
    }
}

#[async_trait]
impl Script for PlatformScript {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(author())
    }

    async fn git_remote(&self, _repo: &str) -> Result<GitRemote> {
        let origin = self.origin.as_ref().ok_or_else(|| {
            ProviderError::Unsupported("No files in this walkthrough.".to_owned())
        })?;
        Ok(GitRemote { clone_url: origin.clone_url.clone(), authorization_header: None })
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        Ok((1..=3).map(|number| self.summary(number)).collect())
    }

    async fn pull_request(&self, _repo: &str, number: u64) -> Result<PullRequestDetail> {
        let (target_sha, head_sha, git_fetch_refs) = match &self.origin {
            Some(origin) => (
                origin.target.clone(),
                origin.head.clone(),
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
}

static CAPABILITIES: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    running_logs: RunningLogs::Growing,
    ..cv_testkit::provider::CAPABILITIES.clone()
});
