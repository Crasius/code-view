//! GraphQL pull request search: query string and row mapping.

use review_core::{
    DiffStat, LineCounts, PullRequestScope, PullRequestState, PullRequestSummary, RowStatus,
    RunStatus, StateFilter,
};

use crate::dto::search::SearchPullRequestDto;
use crate::mapping::{checks, identity, reviewers, run_status};
use crate::owners::SearchOwners;

/// Issue-search queries for the cross-repository scopes, newest first; their results together
/// make up the scope. Search includes drafts.
///
/// [`PullRequestScope::Visible`] is every pull request in the owners' repositories, whoever wrote
/// it, like Azure DevOps' active pull requests of the user's projects. When the owners are
/// everything the token sees, pull requests elsewhere that involve the user (`involves:` covers
/// author, assignee, mentions and comments; `review-requested:` is not part of it) belong in as
/// well; those searches exclude the owners, whose pull requests the first search already brings.
/// "Created by me" and "Review requested" are narrowed to chosen owners only.
/// Returns nothing for [`PullRequestScope::Repository`], which lists through REST.
pub(crate) fn queries(
    scope: PullRequestScope,
    state: StateFilter,
    owners: &SearchOwners,
) -> Vec<String> {
    let within = owners.qualifiers.join(" ");
    let outside = owners
        .qualifiers
        .iter()
        .map(|qualifier| format!("-{qualifier}"))
        .collect::<Vec<_>>()
        .join(" ");
    let narrowed = if owners.is_everything { "" } else { within.as_str() };
    let searches: Vec<(&str, &str)> = match scope {
        PullRequestScope::Visible if owners.is_everything => vec![
            ("", within.as_str()),
            ("involves:@me", outside.as_str()),
            ("review-requested:@me", outside.as_str()),
        ],
        PullRequestScope::Visible => vec![("", within.as_str())],
        PullRequestScope::CreatedByMe | PullRequestScope::ReviewRequested => {
            vec![(role_qualifier(scope), narrowed)]
        }
        PullRequestScope::Repository => return Vec::new(),
    };
    let state = state_qualifier(state);
    searches
        .into_iter()
        // Neither a role nor an owner would search every pull request on the host.
        .filter(|(role, owners)| !role.is_empty() || !owners.is_empty())
        .map(|(role, owners)| {
            ["is:pr", role, state, owners, "sort:created-desc"]
                .iter()
                .filter(|part| !part.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Repositories one search names at most. Several `repo:` qualifiers in one search list the pull
/// requests of all of them; qualifiers do not count towards the search's 256-character limit, and
/// ten keeps every query well inside GitHub's other search limits.
pub(crate) const REPOSITORIES_PER_SEARCH: usize = 10;

/// Issue-search queries for the pull requests of `repositories` in `state` that `role` keeps
/// (every one for [`PullRequestScope::Visible`]), most recently updated first: one per
/// [`REPOSITORIES_PER_SEARCH`] repositories.
pub(crate) fn repository_queries(
    repositories: &[String],
    role: PullRequestScope,
    state: StateFilter,
) -> Vec<String> {
    repositories
        .chunks(REPOSITORIES_PER_SEARCH)
        .map(|chunk| {
            let repos: Vec<String> =
                chunk.iter().map(|repository| format!("repo:{repository}")).collect();
            [
                "is:pr",
                role_qualifier(role),
                state_qualifier(state),
                &repos.join(" "),
                "sort:updated-desc",
            ]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
        })
        .collect()
}

/// The user's part of a scope: what they wrote or are asked to review; nothing for the others.
fn role_qualifier(scope: PullRequestScope) -> &'static str {
    match scope {
        PullRequestScope::CreatedByMe => "author:@me",
        PullRequestScope::ReviewRequested => "review-requested:@me",
        PullRequestScope::Visible | PullRequestScope::Repository => "",
    }
}

fn state_qualifier(state: StateFilter) -> &'static str {
    match state {
        StateFilter::Open => "is:open",
        StateFilter::Merged => "is:merged",
        StateFilter::Closed => "is:closed is:unmerged",
        StateFilter::All => "",
    }
}

/// The row and its checks: the search brings the head commit's check rollup and the review
/// decision, so the row's checks need no request of their own.
pub(crate) fn summary(mut dto: SearchPullRequestDto) -> (PullRequestSummary, RowStatus) {
    let rollup = dto
        .commits
        .take()
        .and_then(|commits| commits.into_nodes().1.into_iter().next())
        .and_then(|node| node.commit.status_check_rollup);
    let checks = rollup.as_ref().map(|rollup| run_status::from_rollup_state(&rollup.state));
    let status = checks::row_status(rollup, dto.review_decision.as_deref());
    (row(dto, checks), status)
}

fn row(dto: SearchPullRequestDto, checks: Option<RunStatus>) -> PullRequestSummary {
    let state = match dto.state.as_str() {
        "MERGED" => PullRequestState::Merged,
        "CLOSED" => PullRequestState::Closed,
        _ => PullRequestState::Open,
    };
    let labels = dto.labels.map(|labels| labels.into_nodes().1).unwrap_or_default();
    let requests = dto.review_requests.map(|requests| requests.into_nodes().1).unwrap_or_default();
    let opinions =
        dto.latest_opinionated_reviews.map(|reviews| reviews.into_nodes().1).unwrap_or_default();
    PullRequestSummary {
        repository_key: dto.repository.name_with_owner,
        repository_name: dto.repository.name,
        number: dto.number,
        title: dto.title,
        author: identity::actor(dto.author),
        state,
        is_draft: dto.is_draft,
        source_branch: dto.head_ref_name,
        target_branch: dto.base_ref_name,
        created_at: dto.created_at,
        updated_at: dto.updated_at,
        web_url: dto.url,
        labels: labels.into_iter().map(identity::label).collect(),
        reviewers: reviewers::from_search(requests, opinions),
        checks,
        diff_stat: DiffStat {
            lines: dto
                .additions
                .zip(dto.deletions)
                .map(|(added, deleted)| LineCounts { added, deleted }),
            files: dto.changed_files,
        },
        head_sha: None,
        target_sha: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::search::SearchData;
    use crate::mapping::fixtures::parse;

    fn owners(qualifiers: &[&str], is_everything: bool) -> SearchOwners {
        SearchOwners {
            qualifiers: qualifiers.iter().map(|qualifier| (*qualifier).to_owned()).collect(),
            is_everything,
        }
    }

    #[test]
    fn everything_lists_all_pull_requests_of_the_user_and_their_organizations() {
        let everything = owners(&["user:mona", "org:contoso", "org:acme"], true);
        assert_eq!(
            queries(PullRequestScope::Visible, StateFilter::Open, &everything),
            [
                "is:pr is:open user:mona org:contoso org:acme sort:created-desc",
                "is:pr involves:@me is:open -user:mona -org:contoso -org:acme sort:created-desc",
                "is:pr review-requested:@me is:open -user:mona -org:contoso -org:acme sort:created-desc",
            ]
        );
        assert_eq!(
            queries(PullRequestScope::CreatedByMe, StateFilter::All, &everything),
            ["is:pr author:@me sort:created-desc"],
            "mine is not narrowed to the owners"
        );
        assert_eq!(
            queries(PullRequestScope::ReviewRequested, StateFilter::Closed, &everything),
            ["is:pr review-requested:@me is:closed is:unmerged sort:created-desc"]
        );
    }

    #[test]
    fn chosen_owners_narrow_every_scope() {
        let chosen = owners(&["org:contoso", "user:mona"], false);
        assert_eq!(
            queries(PullRequestScope::Visible, StateFilter::Open, &chosen),
            ["is:pr is:open org:contoso user:mona sort:created-desc"]
        );
        assert_eq!(
            queries(PullRequestScope::CreatedByMe, StateFilter::Open, &chosen),
            ["is:pr author:@me is:open org:contoso user:mona sort:created-desc"]
        );
        assert_eq!(
            queries(PullRequestScope::ReviewRequested, StateFilter::Merged, &chosen),
            ["is:pr review-requested:@me is:merged org:contoso user:mona sort:created-desc"]
        );
        let repository = queries(PullRequestScope::Repository, StateFilter::All, &chosen);
        assert!(repository.is_empty(), "{repository:?}");
    }

    #[test]
    fn never_searches_the_whole_host() {
        let nobody = owners(&[], true);
        assert_eq!(
            queries(PullRequestScope::Visible, StateFilter::Open, &nobody),
            [
                "is:pr involves:@me is:open sort:created-desc",
                "is:pr review-requested:@me is:open sort:created-desc"
            ]
        );
    }

    #[test]
    fn several_repositories_share_one_search() {
        let repositories: Vec<String> = (1..=12).map(|index| format!("acme/r{index}")).collect();
        let searches =
            repository_queries(&repositories, PullRequestScope::Visible, StateFilter::Open);
        assert_eq!(searches.len(), 2, "ten repositories per search");
        assert_eq!(searches[1], "is:pr is:open repo:acme/r11 repo:acme/r12 sort:updated-desc");
        assert!(searches[0].starts_with("is:pr is:open repo:acme/r1 repo:acme/r2 "));
        assert_eq!(
            repository_queries(&repositories[..1], PullRequestScope::Visible, StateFilter::All),
            ["is:pr repo:acme/r1 sort:updated-desc"]
        );
        let none = repository_queries(&[], PullRequestScope::Visible, StateFilter::Open);
        assert!(none.is_empty(), "{none:?}");
    }

    #[test]
    fn repository_searches_keep_the_users_part() {
        let repositories = vec!["acme/app".to_owned(), "acme/api".to_owned()];
        assert_eq!(
            repository_queries(&repositories, PullRequestScope::CreatedByMe, StateFilter::Open),
            ["is:pr author:@me is:open repo:acme/app repo:acme/api sort:updated-desc"]
        );
        assert_eq!(
            repository_queries(
                &repositories[..1],
                PullRequestScope::ReviewRequested,
                StateFilter::Merged
            ),
            ["is:pr review-requested:@me is:merged repo:acme/app sort:updated-desc"]
        );
    }

    #[test]
    fn maps_search_rows() {
        let data: SearchData = parse(include_str!("../../tests/fixtures/search.json"));
        let (page, rows) = data.search.into_nodes();
        assert!(!page.has_next_page);
        let summaries: Vec<(PullRequestSummary, RowStatus)> =
            rows.into_iter().map(summary).collect();
        assert_eq!(summaries.len(), 1);
        let (row, status) = &summaries[0];
        assert_eq!(status.reported, Some(review_core::CheckState::Failed));
        assert_eq!(row.repository_key, "contoso/app");
        assert_eq!(row.state, PullRequestState::Merged);
        assert_eq!(row.source_branch, "feature/y");
        assert_eq!(row.target_branch, "trunk");
        assert_eq!(row.labels[0].name, "enhancement");
        assert_eq!(row.reviewers.len(), 2);
        assert_eq!(row.author.id, "1");
        assert_eq!(row.checks, Some(RunStatus::Failed));
        assert_eq!(
            row.diff_stat,
            DiffStat::new(1240, 312, 18),
            "line and file counts come with the search"
        );
    }
}
