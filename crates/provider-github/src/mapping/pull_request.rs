//! REST pull request → summary and detail.

use review_core::{
    AutoComplete, DiffStat, LineCounts, MergeStatus, MergeStrategy, ProviderError,
    PullRequestDetail, PullRequestState, PullRequestSummary, Result,
};

use crate::dto::pull_request::PullRequestDto;
use crate::dto::review::ReviewDto;
use crate::mapping::{identity, reviewers};

pub(crate) fn state(dto: &PullRequestDto) -> PullRequestState {
    match (dto.state.as_str(), dto.merged_at.is_some()) {
        (_, true) => PullRequestState::Merged,
        ("closed", false) => PullRequestState::Closed,
        _ => PullRequestState::Open,
    }
}

/// `requested_key` names the repository when the response lacks `base.repo` (never seen, but nullable).
pub(crate) fn summary(
    dto: PullRequestDto,
    reviews: &[ReviewDto],
    requested_key: &str,
) -> PullRequestSummary {
    let state = state(&dto);
    let (repository_key, repository_name) = match &dto.base.repo {
        Some(repo) => (repo.full_name.clone(), repo.name.clone()),
        None => (
            requested_key.to_owned(),
            requested_key.rsplit('/').next().unwrap_or(requested_key).to_owned(),
        ),
    };
    PullRequestSummary {
        repository_key,
        repository_name,
        number: dto.number,
        title: dto.title,
        author: identity::optional_user(dto.user),
        state,
        is_draft: dto.draft,
        source_branch: dto.head.name,
        target_branch: dto.base.name,
        created_at: dto.created_at,
        updated_at: dto.updated_at,
        web_url: dto.html_url,
        labels: dto.labels.into_iter().map(identity::label).collect(),
        reviewers: reviewers::from_rest(dto.requested_reviewers, dto.requested_teams, reviews),
        checks: None,
        // `additions`, `deletions` and `changed_files` come with a single pull request only.
        diff_stat: DiffStat {
            lines: dto
                .additions
                .zip(dto.deletions)
                .map(|(added, deleted)| LineCounts { added, deleted }),
            files: dto.changed_files,
        },
        head_sha: Some(dto.head.sha).filter(|sha| !sha.is_empty()),
        target_sha: Some(dto.base.sha).filter(|sha| !sha.is_empty()),
    }
}

pub(crate) fn detail(
    mut dto: PullRequestDto,
    reviews: &[ReviewDto],
    requested_key: &str,
) -> PullRequestDetail {
    let description = dto.body.take().unwrap_or_default();
    let target_sha = dto.base.sha.clone();
    let head_sha = dto.head.sha.clone();
    let merge_status = merge_status(dto.mergeable, dto.mergeable_state.as_deref());
    let git_fetch_refs =
        vec![format!("refs/pull/{}/head", dto.number), format!("refs/heads/{}", dto.base.name)];
    let closed_at = dto.closed_at;
    let auto_complete = dto.auto_merge.take().map(|auto| AutoComplete {
        set_by: auto.enabled_by.map(identity::user),
        strategy: merge_method_from_github(&auto.merge_method),
    });
    PullRequestDetail {
        summary: summary(dto, reviews, requested_key),
        description,
        target_sha,
        head_sha,
        merge_status,
        git_fetch_refs,
        closed_at,
        auto_complete,
    }
}

/// `merge_method` of merges, auto-merge and branch rules. GitHub has no semi-linear method.
pub(crate) fn merge_method_to_github(strategy: MergeStrategy) -> Option<&'static str> {
    match strategy {
        MergeStrategy::Merge => Some("merge"),
        MergeStrategy::Squash => Some("squash"),
        MergeStrategy::Rebase => Some("rebase"),
        MergeStrategy::SemiLinear => None,
    }
}

/// The `merge_method` a merge or an auto-merge sends; a semi-linear merge is refused before any
/// request.
pub(crate) fn github_method(strategy: MergeStrategy) -> Result<&'static str> {
    merge_method_to_github(strategy).ok_or_else(|| {
        ProviderError::Unsupported(
            "GitHub has no semi-linear merge; pick a merge commit, squash or rebase.".to_owned(),
        )
    })
}

pub(crate) fn merge_method_from_github(method: &str) -> Option<MergeStrategy> {
    MergeStrategy::ALL.into_iter().find(|strategy| {
        merge_method_to_github(*strategy).is_some_and(|name| name.eq_ignore_ascii_case(method))
    })
}

/// `mergeable` is `null` while GitHub computes it; `mergeable_state` then refines the answer.
pub(crate) fn merge_status(mergeable: Option<bool>, mergeable_state: Option<&str>) -> MergeStatus {
    match (mergeable, mergeable_state) {
        (Some(false), _) | (_, Some("dirty")) => MergeStatus::Conflicts,
        (_, Some("blocked" | "behind" | "draft")) => MergeStatus::Blocked,
        (Some(true), _) => MergeStatus::Mergeable,
        _ => MergeStatus::Unknown,
    }
}

/// The head branch may only be deleted when it lives in the base repository (not a fork).
pub(crate) fn head_is_in_base_repository(dto: &PullRequestDto) -> bool {
    match (&dto.head.repo, &dto.base.repo) {
        (Some(head), Some(base)) => head.full_name.eq_ignore_ascii_case(&base.full_name),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::fixtures::parse;
    use review_core::ReviewVote;

    const PULL: &str = include_str!("../../tests/fixtures/pull_request.json");

    #[test]
    fn maps_detail() {
        let reviews: Vec<ReviewDto> = parse(
            r#"[{"id":1,"user":{"id":2,"login":"bob"},"state":"APPROVED","submitted_at":"2024-05-02T08:00:00Z"}]"#,
        );
        let detail = detail(parse(PULL), &reviews, "contoso/app");
        assert_eq!(detail.summary.repository_key, "contoso/app");
        assert_eq!(detail.summary.repository_name, "app");
        assert_eq!(detail.summary.number, 1234);
        assert_eq!(detail.summary.state, PullRequestState::Open);
        assert!(detail.summary.is_draft);
        assert_eq!(detail.summary.source_branch, "feature/x");
        assert_eq!(detail.summary.target_branch, "trunk");
        assert_eq!(detail.summary.author.login.as_deref(), Some("mona"));
        assert_eq!(detail.summary.labels[0].name, "bug");
        assert_eq!(detail.description, "Fixes things");
        assert_eq!(detail.target_sha, "base000");
        assert_eq!(detail.head_sha, "head111");
        assert_eq!(detail.summary.head_sha.as_deref(), Some("head111"));
        assert_eq!(
            detail.summary.diff_stat,
            review_core::DiffStat::new(40, 2, 3),
            "a single pull request carries its counts"
        );
        assert_eq!(detail.git_fetch_refs, vec!["refs/pull/1234/head", "refs/heads/trunk"]);
        assert_eq!(detail.merge_status, MergeStatus::Mergeable);
        let reviewers: Vec<(&str, ReviewVote, bool)> = detail
            .summary
            .reviewers
            .iter()
            .map(|reviewer| (reviewer.identity.id.as_str(), reviewer.vote, reviewer.is_group))
            .collect();
        assert_eq!(
            reviewers,
            vec![
                ("2", ReviewVote::Approved, false),
                ("3", ReviewVote::None, false),
                ("team:77", ReviewVote::None, true)
            ]
        );
    }

    #[test]
    fn closed_and_merged_states() {
        let mut dto: PullRequestDto = parse(PULL);
        dto.state = "closed".to_owned();
        assert_eq!(state(&dto), PullRequestState::Closed);
        dto.merged_at = Some(time::macros::datetime!(2024-05-03 0:00 UTC));
        assert_eq!(state(&dto), PullRequestState::Merged);
    }

    #[test]
    fn merge_status_mapping() {
        assert_eq!(merge_status(None, Some("unknown")), MergeStatus::Unknown);
        assert_eq!(merge_status(Some(false), Some("dirty")), MergeStatus::Conflicts);
        assert_eq!(merge_status(None, Some("dirty")), MergeStatus::Conflicts);
        assert_eq!(merge_status(Some(true), Some("blocked")), MergeStatus::Blocked);
        assert_eq!(merge_status(Some(true), Some("behind")), MergeStatus::Blocked);
        assert_eq!(merge_status(Some(true), Some("unstable")), MergeStatus::Mergeable);
        assert_eq!(merge_status(Some(true), Some("clean")), MergeStatus::Mergeable);
    }

    #[test]
    fn fork_heads_are_not_deletable() {
        let mut dto: PullRequestDto = parse(PULL);
        assert!(head_is_in_base_repository(&dto));
        dto.head.repo = None;
        assert!(!head_is_in_base_repository(&dto));
    }
}
