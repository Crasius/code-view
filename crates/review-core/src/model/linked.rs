use super::RunStatus;

/// What a reference points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ItemKind {
    PullRequest,
    Issue,
    /// GitHub's `#12`: issues and pull requests share one number sequence, so only the platform
    /// knows which one it is.
    IssueOrPullRequest,
    /// Azure DevOps work item; `repository_key` is `organization/project` (or just the organization).
    WorkItem,
}

/// A pull request, issue or work item another text points at.
///
/// `repository_key` is a [`super::RepositoryInfo::key`] for pull requests and issues. Azure DevOps
/// pull request ids are unique per organization, so a `!12` reference carries the current
/// repository only as a hint; the provider looks it up by id and reports where it really lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemRef {
    pub kind: ItemKind,
    pub repository_key: String,
    pub number: u64,
}

/// Where a linked item stands, in the categories the overview colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkedState {
    Open,
    Draft,
    Merged,
    /// An issue or work item that was finished (closed as completed, resolved, done).
    Done,
    /// Abandoned pull requests, issues closed as not planned, removed work items.
    Closed,
}

/// The current title and state of a referenced item.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedItem {
    /// The reference exactly as it was requested, so callers can match answers to questions.
    pub reference: ItemRef,
    /// What the item turned out to be; never [`ItemKind::IssueOrPullRequest`].
    pub kind: ItemKind,
    /// Where the item lives (an Azure DevOps pull request's real repository).
    pub repository_key: String,
    pub title: String,
    pub state: LinkedState,
    /// The platform's own word for the state ("Active", "Resolved", "Closed").
    pub state_label: String,
    /// Combined checks of a pull request's head commit, where the platform reports it cheaply.
    pub checks: Option<RunStatus>,
    /// Work item type ("Bug", "User Story").
    pub item_type: Option<String>,
    pub web_url: String,
}
