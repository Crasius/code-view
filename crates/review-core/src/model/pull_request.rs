use serde::{Deserialize, Serialize};

use super::{Label, Reviewer, RunStatus, Timestamp, UserIdentity};

/// Draft is a flag on the summary, not a state. Azure DevOps "completed" → `Merged`, "abandoned" → `Closed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PullRequestState {
    Open,
    Merged,
    Closed,
}

/// A pull request as a list row. `number` is the per-repository number users see
/// (GitHub number, GitLab iid, Azure DevOps pull request id).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequestSummary {
    pub repository_key: String,
    pub repository_name: String,
    pub number: u64,
    pub title: String,
    pub author: UserIdentity,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub source_branch: String,
    pub target_branch: String,
    pub created_at: Timestamp,
    pub updated_at: Option<Timestamp>,
    pub web_url: String,
    pub labels: Vec<Label>,
    /// May be empty in listings where the platform does not return reviewers cheaply.
    pub reviewers: Vec<Reviewer>,
    /// Combined status of the head commit's checks, where the listing returns it without extra
    /// requests (GitHub search); `None` when unknown.
    pub checks: Option<RunStatus>,
    /// Lines and files the pull request changes, as far as the listing reports them (GitHub
    /// search); [`Provider::diff_stats`](crate::Provider::diff_stats) fills in the rest.
    pub diff_stat: DiffStat,
    /// Latest source commit, where the listing names it: counts kept for a row are only shown
    /// while it is the same.
    pub head_sha: Option<String>,
    /// Target-branch commit the platform diffs against, where the listing names it (Azure DevOps
    /// `lastMergeTargetCommit`, GitHub `base.sha`).
    pub target_sha: Option<String>,
}

impl PullRequestSummary {
    /// The last update, or the creation where the host reports none.
    pub fn last_activity(&self) -> Timestamp {
        self.updated_at.unwrap_or(self.created_at)
    }
}

/// What a pull request changes, each part `None` while unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiffStat {
    pub lines: Option<LineCounts>,
    pub files: Option<u64>,
}

impl DiffStat {
    pub fn new(added: u64, deleted: u64, files: u64) -> Self {
        Self { lines: Some(LineCounts { added, deleted }), files: Some(files) }
    }

    /// Each part this one does not know, from `other`.
    #[must_use]
    pub fn or(self, other: DiffStat) -> DiffStat {
        DiffStat { lines: self.lines.or(other.lines), files: self.files.or(other.files) }
    }
}

/// Added and deleted lines; binary files count none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LineCounts {
    pub added: u64,
    pub deleted: u64,
}

impl LineCounts {
    pub fn total(self) -> u64 {
        self.added.saturating_add(self.deleted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MergeStatus {
    Unknown,
    Mergeable,
    Conflicts,
    Blocked,
}

/// Everything the overview needs plus what the local git mirror needs to compute diffs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequestDetail {
    pub summary: PullRequestSummary,
    /// Markdown as the author wrote it.
    pub description: String,
    /// Target-branch commit the platform diffs against
    /// (GitHub `base.sha`, GitLab `diff_refs.base_sha`, Azure DevOps `lastMergeTargetCommit`).
    /// The diff engine computes the merge base of this and `head_sha` itself.
    pub target_sha: String,
    /// Latest source commit.
    pub head_sha: String,
    pub merge_status: MergeStatus,
    /// Remote ref names whose fetch makes `target_sha` and `head_sha` reachable,
    /// e.g. `refs/pull/12/head` and `refs/heads/main`.
    pub git_fetch_refs: Vec<String>,
    pub closed_at: Option<Timestamp>,
    /// The auto-complete (Azure DevOps), auto-merge (GitHub) or "set to auto-merge" (GitLab) on
    /// this pull request; `None` when none is set.
    pub auto_complete: Option<AutoComplete>,
}

/// A pull request the platform completes by itself once its policies pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoComplete {
    /// Who turned it on, where the platform says.
    pub set_by: Option<UserIdentity>,
    /// The strategy it will merge with; `None` where the platform picks it only when it merges
    /// (GitLab without squash merges with the project's merge method).
    pub strategy: Option<MergeStrategy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PullRequestScope {
    /// Every pull request the user can see within the connection's organizations. Connections
    /// without organizations have no bounded "everything", so there it means every pull request
    /// that involves the user (created, assigned or review requested).
    Visible,
    /// Authored by the token's user, across the connection's organizations, or in
    /// [`PullRequestQuery::repository_key`] when it is set.
    CreatedByMe,
    /// The token's user is a requested reviewer (assignee where the platform has no reviewers),
    /// across the connection's organizations, or in [`PullRequestQuery::repository_key`] when it is set.
    ReviewRequested,
    /// All pull requests of [`PullRequestQuery::repository_key`].
    Repository,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StateFilter {
    Open,
    Merged,
    Closed,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestQuery {
    pub scope: PullRequestScope,
    pub state: StateFilter,
    /// Required for [`PullRequestScope::Repository`]; narrows [`PullRequestScope::CreatedByMe`]
    /// and [`PullRequestScope::ReviewRequested`] to this one repository; ignored for
    /// [`PullRequestScope::Visible`].
    pub repository_key: Option<String>,
    /// Maximum rows; providers may stop paging once reached.
    pub top: usize,
}

impl PullRequestQuery {
    /// The pull requests of one repository that `role` keeps: every one for
    /// [`PullRequestScope::Visible`] (and [`PullRequestScope::Repository`]), else the user's part.
    pub fn in_repository(
        repository_key: String,
        role: PullRequestScope,
        state: StateFilter,
        top: usize,
    ) -> Self {
        let scope = match role {
            PullRequestScope::CreatedByMe | PullRequestScope::ReviewRequested => role,
            PullRequestScope::Visible | PullRequestScope::Repository => {
                PullRequestScope::Repository
            }
        };
        Self { scope, state, repository_key: Some(repository_key), top }
    }
}

/// How a pull request lands on its target branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MergeStrategy {
    /// A two-parent merge commit; the source branch stays as it is (no fast-forward).
    Merge,
    /// All changes in one single-parent commit.
    Squash,
    /// The commits replayed onto the target, which then fast-forwards: a straight line.
    Rebase,
    /// Rebase first, then a two-parent merge commit: semi-linear history (Azure DevOps
    /// `rebaseMerge`, GitLab's "merge commit with semi-linear history").
    SemiLinear,
}

impl MergeStrategy {
    pub const ALL: [MergeStrategy; 4] = [
        MergeStrategy::Merge,
        MergeStrategy::Squash,
        MergeStrategy::Rebase,
        MergeStrategy::SemiLinear,
    ];

    /// The strategy writes a commit message: merge and squash commits do, rebasing does not.
    pub fn takes_message(self) -> bool {
        matches!(self, MergeStrategy::Merge | MergeStrategy::Squash | MergeStrategy::SemiLinear)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOptions {
    pub strategy: MergeStrategy,
    pub delete_source_branch: bool,
    /// Merge or squash commit message; `None` lets the platform choose.
    pub commit_message: Option<String>,
    /// The source commit the user reviewed: [`PullRequestDetail::head_sha`] of the detail the page
    /// showed when the merge was confirmed. Providers send it as the platform's guard (GitHub and
    /// GitLab `sha`, Azure DevOps `lastMergeSourceCommit`), never a head fetched just before, so a
    /// push after the review makes the platform refuse instead of merging unseen commits.
    /// A refusal for this reason is [`crate::ProviderError::HeadMoved`].
    pub head_sha: String,
}

/// Characters of a commit id the app shows: git's own default abbreviation.
pub const SHORT_SHA_LEN: usize = 7;

/// The first [`SHORT_SHA_LEN`] characters of `sha`, or all of a shorter one.
pub fn short_sha(sha: &str) -> &str {
    sha.get(..SHORT_SHA_LEN).unwrap_or(sha)
}

#[cfg(test)]
mod tests {
    use super::short_sha;

    #[test]
    fn cuts_a_commit_id_to_seven_characters() {
        assert_eq!(short_sha("0123456789abcdef0123456789abcdef01234567"), "0123456");
        assert_eq!(short_sha("abc12"), "abc12", "a shorter id stays whole");
        assert_eq!(short_sha(""), "");
        assert_eq!(short_sha("ääääääää"), "ääääääää", "no cut inside a character");
    }
}
