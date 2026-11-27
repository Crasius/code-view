//! What a link in a description, comment or commit message points at: a URL on a saved
//! connection, or a shorthand reference (`#12`, `owner/repo#12`, `!12`) read in the context of the
//! pull request it appears in.

// RS-DONT-04: parses link targets from pull request text, so indexing, string slicing and narrowing
// casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing, clippy::cast_possible_truncation)]

use review_core::{Connection, ItemKind, ItemRef, ProviderKind};

use super::pull_request_link;
use crate::route::PullRequestKey;

/// The repository a text belongs to; shorthand references are relative to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Here {
    pub connection_id: String,
    pub kind: ProviderKind,
    pub repository_key: String,
}

/// An item on a saved connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Target {
    pub connection_id: String,
    pub item: ItemRef,
    /// False for Azure DevOps `!12`: the id is organization-wide, `item.repository_key` only the
    /// current repository as a guess until the platform answers.
    pub is_exact: bool,
}

impl Target {
    /// The pull request page this target opens without asking the platform first.
    pub fn pull_request_key(&self) -> Option<PullRequestKey> {
        (self.item.kind == ItemKind::PullRequest && self.is_exact).then(|| PullRequestKey {
            connection_id: self.connection_id.clone(),
            repository_key: self.item.repository_key.clone(),
            number: self.item.number,
        })
    }

    /// How the platform writes the reference (`#12`, `owner/repo#12`, `!12`), relative to `here`.
    pub fn label(&self, here: Option<&Here>) -> String {
        let marker = match self.item.kind {
            ItemKind::PullRequest if here.is_none_or(|here| here.kind != ProviderKind::GitHub) => {
                "!"
            }
            _ => "#",
        };
        let is_local = here.is_some_and(|here| {
            here.connection_id == self.connection_id
                && (here.repository_key == self.item.repository_key || !self.is_exact)
        });
        let is_work_item = self.item.kind == ItemKind::WorkItem;
        if is_local || is_work_item {
            format!("{marker}{}", self.item.number)
        } else {
            format!("{}{marker}{}", self.item.repository_key, self.item.number)
        }
    }
}

/// Resolves a link target: a URL any connection serves, or a reference relative to `here`.
pub fn classify(target: &str, connections: &[Connection], here: Option<&Here>) -> Option<Target> {
    if target.starts_with("https://") || target.starts_with("http://") {
        let (connection_id, item) = pull_request_link::resolve_item(target, connections)?;
        return Some(Target { connection_id, item, is_exact: true });
    }
    let here = here?;
    let (path, reference) = target.split_at_checked(target.rfind(['#', '!'])?)?;
    let is_bang = reference.starts_with('!');
    let number: u64 = reference.get(1..)?.parse().ok()?;
    let repository = if path.is_empty() { here.repository_key.clone() } else { path.to_owned() };
    let (kind, repository_key, is_exact) = match (here.kind, is_bang) {
        (ProviderKind::GitHub, false) => (ItemKind::IssueOrPullRequest, repository, true),
        (ProviderKind::GitHub, true) => return None,
        (ProviderKind::GitLab, false) => (ItemKind::Issue, repository, true),
        (ProviderKind::GitLab, true) => (ItemKind::PullRequest, repository, true),
        (ProviderKind::AzureDevOps, _) if !path.is_empty() => return None,
        (ProviderKind::AzureDevOps, false) => {
            (ItemKind::WorkItem, project_of(&here.repository_key), true)
        }
        (ProviderKind::AzureDevOps, true) => {
            (ItemKind::PullRequest, here.repository_key.clone(), false)
        }
    };
    Some(Target {
        connection_id: here.connection_id.clone(),
        item: ItemRef { kind, repository_key, number },
        is_exact,
    })
}

/// `organization/project` of an Azure DevOps repository key.
fn project_of(repository_key: &str) -> String {
    let mut parts = repository_key.splitn(3, '/');
    match (parts.next(), parts.next()) {
        (Some(organization), Some(project)) => format!("{organization}/{project}"),
        _ => repository_key.to_owned(),
    }
}

/// A ticket in an external tracker: Jira-style `/browse/KEY-12` or `/issue/KEY-12` URLs.
/// Returns the key and the tracker's URL prefix (everything before the key).
pub fn ticket(url: &str) -> Option<(String, String)> {
    let address = url.split(['?', '#']).next()?.trim_end_matches('/');
    let (prefix, key) = address.rsplit_once('/')?;
    let is_tracker_path = prefix.ends_with("/browse") || prefix.ends_with("/issue");
    let keys = super::references::ticket_keys(key);
    (is_tracker_path && keys.first() == Some(&key)).then(|| (key.to_owned(), format!("{prefix}/")))
}

#[cfg(test)]
mod tests {
    use cv_testkit::samples::connection;
    use review_core::{Connection, ItemKind, ProviderKind};

    use super::{Here, Target, classify, ticket};

    fn connections() -> Vec<Connection> {
        vec![
            connection("gh", ProviderKind::GitHub, "https://github.com"),
            connection("ghe", ProviderKind::GitHub, "https://git.corp.example"),
            connection("gl", ProviderKind::GitLab, "https://gitlab.com"),
            connection("gl-self", ProviderKind::GitLab, "https://gitlab.corp.example"),
            connection("ado", ProviderKind::AzureDevOps, "https://dev.azure.com"),
            connection("ado-server", ProviderKind::AzureDevOps, "https://tfs.corp.example/tfs"),
        ]
    }

    fn here(connection_id: &str, kind: ProviderKind, repository_key: &str) -> Here {
        Here {
            connection_id: connection_id.to_owned(),
            kind,
            repository_key: repository_key.to_owned(),
        }
    }

    fn resolved(
        target: &str,
        here: Option<&Here>,
    ) -> Option<(String, ItemKind, String, u64, bool)> {
        let found = classify(target, &connections(), here)?;
        Some((
            found.connection_id,
            found.item.kind,
            found.item.repository_key,
            found.item.number,
            found.is_exact,
        ))
    }

    #[test]
    fn urls_resolve_exactly_on_their_connection() {
        assert_eq!(
            resolved("https://gitlab.corp.example/a/b/c/-/merge_requests/4/diffs", None),
            Some(("gl-self".to_owned(), ItemKind::PullRequest, "a/b/c".to_owned(), 4, true))
        );
        assert_eq!(resolved("https://example.org/a/b/pull/1", None), None);
    }

    #[test]
    fn shorthand_references_follow_the_platform() {
        let github = here("gh", ProviderKind::GitHub, "cli/cli");
        assert_eq!(
            resolved("#59", Some(&github)),
            Some(("gh".into(), ItemKind::IssueOrPullRequest, "cli/cli".into(), 59, true))
        );
        assert_eq!(
            resolved("owner/repo#3", Some(&github)),
            Some(("gh".into(), ItemKind::IssueOrPullRequest, "owner/repo".into(), 3, true))
        );
        assert_eq!(resolved("!3", Some(&github)), None);

        let gitlab = here("gl-self", ProviderKind::GitLab, "group/app");
        assert_eq!(
            resolved("!12", Some(&gitlab)),
            Some(("gl-self".into(), ItemKind::PullRequest, "group/app".into(), 12, true))
        );
        assert_eq!(
            resolved("#12", Some(&gitlab)),
            Some(("gl-self".into(), ItemKind::Issue, "group/app".into(), 12, true))
        );
        assert_eq!(
            resolved("other/sub/lib!2", Some(&gitlab)),
            Some(("gl-self".into(), ItemKind::PullRequest, "other/sub/lib".into(), 2, true))
        );

        let azure = here("ado", ProviderKind::AzureDevOps, "org/Proj/repo");
        assert_eq!(
            resolved("!12", Some(&azure)),
            Some(("ado".into(), ItemKind::PullRequest, "org/Proj/repo".into(), 12, false))
        );
        assert_eq!(
            resolved("#12", Some(&azure)),
            Some(("ado".into(), ItemKind::WorkItem, "org/Proj".into(), 12, true))
        );
        assert_eq!(resolved("#12", None), None, "references need a repository");
    }

    #[test]
    fn exact_pull_requests_open_without_asking() {
        let azure = here("ado", ProviderKind::AzureDevOps, "org/Proj/repo");
        let by_id = classify("!12", &connections(), Some(&azure)).expect("resolves");
        assert!(by_id.pull_request_key().is_none(), "the repository is a guess");
        let by_url = classify(
            "https://dev.azure.com/org/Proj/_git/repo/pullrequest/12",
            &connections(),
            Some(&azure),
        )
        .expect("resolves");
        assert_eq!(by_url.pull_request_key().map(|key| key.number), Some(12));
    }

    #[test]
    fn labels_are_relative_to_the_current_repository() {
        let github = here("gh", ProviderKind::GitHub, "cli/cli");
        let label = |target: &str| {
            classify(target, &connections(), Some(&github))
                .map(|found: Target| found.label(Some(&github)))
        };
        assert_eq!(label("#5").as_deref(), Some("#5"));
        assert_eq!(
            label("https://github.com/rust-lang/rust/pull/7").as_deref(),
            Some("rust-lang/rust#7")
        );
        let gitlab = here("gl", ProviderKind::GitLab, "group/app");
        let found = classify("!4", &connections(), Some(&gitlab)).expect("resolves");
        assert_eq!(found.label(Some(&gitlab)), "!4");
    }

    #[test]
    fn jira_style_ticket_urls() {
        assert_eq!(
            ticket("https://acme.atlassian.net/browse/PAY-142?focusedCommentId=1"),
            Some(("PAY-142".to_owned(), "https://acme.atlassian.net/browse/".to_owned()))
        );
        assert_eq!(
            ticket("https://youtrack.example.com/issue/WEB-7"),
            Some(("WEB-7".to_owned(), "https://youtrack.example.com/issue/".to_owned()))
        );
        assert_eq!(ticket("https://example.com/docs/UTF-8"), None);
    }
}
