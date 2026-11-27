use review_core::{
    AutoComplete, DiffStat, Label, MergeStatus, MergeStrategy, ProviderError, PullRequestDetail,
    PullRequestState, PullRequestSummary, Result, StateFilter, Timestamp,
};
use serde::Deserialize;

use super::identity::{IdentityRef, ReviewerRef};
use super::repository::Repository;
use super::{CommitRef, branch_name};
use crate::client::RepoKey;
use crate::pull_requests::NO_IDENTITY;

/// `WebApiTagDefinition`: a pull request tag ("label").
#[derive(Debug, Deserialize)]
pub(crate) struct Tag {
    pub name: String,
    #[serde(default = "crate::dto::default_true")]
    pub active: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(clippy::struct_field_names, reason = "fields mirror the GitPullRequest JSON keys")]
pub(crate) struct PullRequest {
    pub pull_request_id: u64,
    #[serde(default)]
    pub repository: Repository,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub created_by: IdentityRef,
    #[serde(deserialize_with = "super::date::required")]
    pub creation_date: Timestamp,
    #[serde(default, deserialize_with = "super::date::optional")]
    pub closed_date: Option<Timestamp>,
    #[serde(default)]
    pub title: String,
    pub description: Option<String>,
    #[serde(default)]
    pub source_ref_name: String,
    #[serde(default)]
    pub target_ref_name: String,
    pub merge_status: Option<String>,
    #[serde(default)]
    pub is_draft: bool,
    pub last_merge_source_commit: Option<CommitRef>,
    pub last_merge_target_commit: Option<CommitRef>,
    #[serde(default)]
    pub reviewers: Vec<ReviewerRef>,
    #[serde(default)]
    pub labels: Vec<Tag>,
    /// Set when auto-complete is enabled: who enabled it. A cancelled one may come back as the
    /// empty identity.
    pub auto_complete_set_by: Option<IdentityRef>,
    pub completion_options: Option<CompletionOptions>,
}

/// `GitPullRequestCompletionOptions`, only the fields that pick the merge strategy.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompletionOptions {
    pub merge_strategy: Option<String>,
    /// Deprecated: without `mergeStrategy`, true squashes and false merges without fast-forward.
    pub squash_merge: Option<bool>,
}

impl PullRequest {
    /// Who set auto-complete and the strategy it will use, when auto-complete is enabled.
    pub(crate) fn auto_complete(&self) -> Option<AutoComplete> {
        let set_by = self
            .auto_complete_set_by
            .as_ref()
            .filter(|identity| !identity.id.is_empty() && identity.id != NO_IDENTITY)?;
        let options = self.completion_options.as_ref();
        let explicit = options
            .and_then(|options| options.merge_strategy.as_deref())
            .and_then(merge_strategy_from_ado);
        let legacy = || {
            if options.and_then(|options| options.squash_merge) == Some(true) {
                MergeStrategy::Squash
            } else {
                MergeStrategy::Merge
            }
        };
        Some(AutoComplete {
            set_by: Some(set_by.to_user()),
            strategy: Some(explicit.unwrap_or_else(legacy)),
        })
    }

    /// `organization` completes the repository key (the API only names project and repository);
    /// `web_url` builds the browser link for a key and pull request id.
    pub(crate) fn to_summary(
        &self,
        organization: &str,
        web_url: impl Fn(RepoKey<'_>, u64) -> String,
    ) -> PullRequestSummary {
        let key = RepoKey {
            organization,
            project: &self.repository.project.name,
            repository: &self.repository.name,
        };
        PullRequestSummary {
            repository_key: self.repository.key(organization),
            repository_name: self.repository.name.clone(),
            number: self.pull_request_id,
            title: self.title.clone(),
            author: self.created_by.to_user(),
            state: state_from_ado(&self.status),
            is_draft: self.is_draft,
            source_branch: branch_name(&self.source_ref_name),
            target_branch: branch_name(&self.target_ref_name),
            created_at: self.creation_date,
            // Azure DevOps reports no "last updated" instant for pull requests.
            updated_at: None,
            web_url: web_url(key, self.pull_request_id),
            labels: active_labels(&self.labels),
            reviewers: self.reviewers.iter().map(ReviewerRef::to_reviewer).collect(),
            // Build and status checks need a request per pull request; the list leaves them out.
            checks: None,
            // No line counts anywhere in the API; the changed files are a request per pull request.
            diff_stat: DiffStat::default(),
            head_sha: self.last_merge_source_commit.as_ref().map(|commit| commit.commit_id.clone()),
            target_sha: self
                .last_merge_target_commit
                .as_ref()
                .map(|commit| commit.commit_id.clone()),
        }
    }

    pub(crate) fn to_detail(
        &self,
        organization: &str,
        web_url: impl Fn(RepoKey<'_>, u64) -> String,
    ) -> Result<PullRequestDetail> {
        let commit = |commit: &Option<CommitRef>, field: &str| {
            commit.as_ref().map(|commit| commit.commit_id.clone()).ok_or_else(|| {
                ProviderError::Decode(format!(
                    "pull request {} has no {field} yet",
                    self.pull_request_id
                ))
            })
        };
        Ok(PullRequestDetail {
            summary: self.to_summary(organization, web_url),
            description: self.description.clone().unwrap_or_default(),
            target_sha: commit(&self.last_merge_target_commit, "lastMergeTargetCommit")?,
            head_sha: commit(&self.last_merge_source_commit, "lastMergeSourceCommit")?,
            merge_status: merge_status_from_ado(self.merge_status.as_deref()),
            // The source may live in a fork; the merge ref keeps the source commit reachable regardless.
            git_fetch_refs: vec![
                self.source_ref_name.clone(),
                self.target_ref_name.clone(),
                format!("refs/pull/{}/merge", self.pull_request_id),
            ],
            closed_at: self.closed_date,
            auto_complete: self.auto_complete(),
        })
    }
}

pub(crate) fn active_labels(tags: &[Tag]) -> Vec<Label> {
    tags.iter()
        .filter(|tag| tag.active)
        .map(|tag| Label { name: tag.name.clone(), color: None, description: None })
        .collect()
}

pub(crate) fn state_from_ado(status: &str) -> PullRequestState {
    match status {
        "completed" => PullRequestState::Merged,
        "abandoned" => PullRequestState::Closed,
        _ => PullRequestState::Open,
    }
}

pub(crate) fn state_filter_to_ado(state: StateFilter) -> &'static str {
    match state {
        StateFilter::Open => "active",
        StateFilter::Merged => "completed",
        StateFilter::Closed => "abandoned",
        StateFilter::All => "all",
    }
}

pub(crate) fn merge_status_from_ado(status: Option<&str>) -> MergeStatus {
    match status {
        Some("succeeded") => MergeStatus::Mergeable,
        Some("conflicts") => MergeStatus::Conflicts,
        Some("rejectedByPolicy" | "failure") => MergeStatus::Blocked,
        _ => MergeStatus::Unknown,
    }
}

/// `GitPullRequestMergeStrategy`: Merge is a no-fast-forward merge commit, Rebase rebases and
/// fast-forwards, `SemiLinear` rebases and then merges without fast-forward (`rebaseMerge`).
pub(crate) fn merge_strategy_to_ado(strategy: MergeStrategy) -> &'static str {
    match strategy {
        MergeStrategy::Merge => "noFastForward",
        MergeStrategy::Squash => "squash",
        MergeStrategy::Rebase => "rebase",
        MergeStrategy::SemiLinear => "rebaseMerge",
    }
}

pub(crate) fn merge_strategy_from_ado(strategy: &str) -> Option<MergeStrategy> {
    MergeStrategy::ALL
        .into_iter()
        .find(|candidate| merge_strategy_to_ado(*candidate).eq_ignore_ascii_case(strategy))
}

#[cfg(test)]
mod tests {
    use review_core::ReviewVote;

    use super::*;

    const FIXTURE: &str = r#"{
        "repository": {"id": "r", "name": "web", "project": {"id": "p", "name": "My Project"}},
        "pullRequestId": 22, "status": "active",
        "createdBy": {"id": "u1", "displayName": "Normal Paulk", "uniqueName": "normal@fabrikam.com"},
        "creationDate": "2026-02-18T04:35:01.9643061Z",
        "title": "Add feature", "description": "Body",
        "sourceRefName": "refs/heads/feature/x", "targetRefName": "refs/heads/main",
        "mergeStatus": "conflicts", "isDraft": true,
        "lastMergeSourceCommit": {"commitId": "aaa"}, "lastMergeTargetCommit": {"commitId": "bbb"},
        "reviewers": [{"id": "u2", "displayName": "R", "vote": 5, "isRequired": true}],
        "labels": [{"id": "t1", "name": "bug", "active": true}, {"id": "t2", "name": "old", "active": false}]
    }"#;

    fn web_url(key: RepoKey<'_>, number: u64) -> String {
        format!("{}|{}|{}|{number}", key.organization, key.project, key.repository)
    }

    #[test]
    fn maps_summary() {
        let pull_request: PullRequest = serde_json::from_str(FIXTURE).unwrap();
        let summary = pull_request.to_summary("fabrikam", web_url);
        assert_eq!(summary.repository_key, "fabrikam/My Project/web");
        assert_eq!(summary.number, 22);
        assert_eq!(summary.state, PullRequestState::Open);
        assert!(summary.is_draft);
        assert_eq!(summary.source_branch, "feature/x");
        assert_eq!(summary.target_branch, "main");
        assert_eq!(summary.web_url, "fabrikam|My Project|web|22");
        assert_eq!(
            summary.labels.iter().map(|label| label.name.as_str()).collect::<Vec<_>>(),
            ["bug"]
        );
        assert_eq!(summary.reviewers[0].vote, ReviewVote::ApprovedWithSuggestions);
        assert!(summary.reviewers[0].is_required);
        assert_eq!(summary.author.login.as_deref(), Some("normal@fabrikam.com"));
        assert_eq!(
            (summary.head_sha.as_deref(), summary.target_sha.as_deref()),
            (Some("aaa"), Some("bbb"))
        );
        assert_eq!(summary.diff_stat, DiffStat::default(), "the listing counts nothing");
    }

    #[test]
    fn maps_detail_commits_and_fetch_refs() {
        let pull_request: PullRequest = serde_json::from_str(FIXTURE).unwrap();
        let detail = pull_request.to_detail("fabrikam", web_url).unwrap();
        assert_eq!(detail.head_sha, "aaa");
        assert_eq!(detail.target_sha, "bbb");
        assert_eq!(detail.merge_status, MergeStatus::Conflicts);
        assert_eq!(
            detail.git_fetch_refs,
            ["refs/heads/feature/x", "refs/heads/main", "refs/pull/22/merge"]
        );
        assert_eq!(detail.description, "Body");
    }

    #[test]
    fn detail_without_merge_commits_is_a_decode_error() {
        let mut value: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        value.as_object_mut().unwrap().remove("lastMergeTargetCommit");
        let pull_request: PullRequest = serde_json::from_value(value).unwrap();
        assert!(matches!(pull_request.to_detail("o", web_url), Err(ProviderError::Decode(_))));
    }

    #[test]
    fn maps_states_and_merge_status() {
        assert_eq!(state_from_ado("completed"), PullRequestState::Merged);
        assert_eq!(state_from_ado("abandoned"), PullRequestState::Closed);
        assert_eq!(merge_status_from_ado(Some("rejectedByPolicy")), MergeStatus::Blocked);
        assert_eq!(merge_status_from_ado(Some("failure")), MergeStatus::Blocked);
        assert_eq!(merge_status_from_ado(Some("queued")), MergeStatus::Unknown);
        assert_eq!(merge_status_from_ado(None), MergeStatus::Unknown);
        assert_eq!(state_filter_to_ado(StateFilter::Closed), "abandoned");
        assert_eq!(merge_strategy_to_ado(MergeStrategy::Merge), "noFastForward");
        assert_eq!(merge_strategy_to_ado(MergeStrategy::SemiLinear), "rebaseMerge");
        for strategy in MergeStrategy::ALL {
            assert_eq!(merge_strategy_from_ado(merge_strategy_to_ado(strategy)), Some(strategy));
        }
        assert_eq!(merge_strategy_from_ado("fastForward"), None);
    }

    #[test]
    fn auto_complete_needs_someone_who_set_it() {
        let with = |extra: serde_json::Value| -> PullRequest {
            let mut value: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
            value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            serde_json::from_value(value).unwrap()
        };
        let strategy = |pull_request: &PullRequest| {
            pull_request.auto_complete().and_then(|auto_complete| auto_complete.strategy)
        };
        let set_by = serde_json::json!({"id": "u1", "displayName": "Normal Paulk"});
        let squash = with(
            serde_json::json!({"autoCompleteSetBy": set_by, "completionOptions": {"mergeStrategy": "squash"}}),
        );
        let auto_complete = squash.to_detail("o", web_url).unwrap().auto_complete.unwrap();
        assert_eq!(auto_complete.strategy, Some(MergeStrategy::Squash));
        assert_eq!(auto_complete.set_by.unwrap().display_name, "Normal Paulk");
        let semi = with(
            serde_json::json!({"autoCompleteSetBy": set_by, "completionOptions": {"mergeStrategy": "rebaseMerge"}}),
        );
        assert_eq!(strategy(&semi), Some(MergeStrategy::SemiLinear));
        let legacy = with(
            serde_json::json!({"autoCompleteSetBy": set_by, "completionOptions": {"squashMerge": true}}),
        );
        assert_eq!(strategy(&legacy), Some(MergeStrategy::Squash));
        let plain = with(serde_json::json!({"autoCompleteSetBy": set_by}));
        assert_eq!(strategy(&plain), Some(MergeStrategy::Merge), "no-fast-forward is the default");
        let without = with(serde_json::json!({"completionOptions": {"mergeStrategy": "squash"}}));
        assert_eq!(without.auto_complete(), None, "completion options alone are no auto-complete");
        let cancelled =
            with(serde_json::json!({"autoCompleteSetBy": {"id": NO_IDENTITY, "displayName": ""}}));
        assert_eq!(cancelled.auto_complete(), None, "the empty identity is a cancelled one");
    }
}
