//! Pull request listing (REST per repository, GraphQL search across the owners' repositories)
//! and detail. The submodules act on one pull request: its lifecycle, reviews, labels, merge
//! policy and change detection.

pub(crate) mod auto_merge;
pub(crate) mod labels;
pub(crate) mod lifecycle;
pub(crate) mod merge_policy;
pub(crate) mod reviews;
pub(crate) mod revision;
pub(crate) mod statuses;

use std::collections::{HashMap, HashSet};

use review_core::{
    DiffStat, ProviderError, PullRequestDetail, PullRequestQuery, PullRequestScope,
    PullRequestSummary, Result, StateFilter,
};
use serde_json::json;

use crate::client::{GitHubClient, Paging, pages_for, per_page};
use crate::dto::pull_request::PullRequestDto;
use crate::dto::search::SearchData;
use crate::graphql::{self, ACTOR_FRAGMENT, Partial};
use crate::mapping::{pull_request, search};
use crate::owners;
use crate::repo_key::RepoKey;
use crate::sso;

/// Extra pages scanned when merged/closed rows are filtered out of `state=closed`.
const MAX_FILTER_PAGES: usize = 10;

/// GitHub search returns at most 1000 results.
const MAX_SEARCH_RESULTS: usize = 1000;

/// The smallest search page a timed-out search is cut down to before its error is shown.
const MIN_SEARCH_PAGE: usize = 10;

const SEARCH_QUERY: &str = "query($q: String!, $first: Int!, $cursor: String) {
  search(type: ISSUE, query: $q, first: $first, after: $cursor) {
    pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest {
      number title url state isDraft createdAt updatedAt headRefName baseRefName
      author { ...actor }
      repository { nameWithOwner name }
      labels(first: 20) { nodes { name color description } }
      reviewRequests(first: 20) { nodes { requestedReviewer {
        __typename
        ... on User { login avatarUrl databaseId name }
        ... on Team { databaseId name slug }
      } } }
      latestOpinionatedReviews(first: 20) { nodes { state author { ...actor } } }
      reviewDecision
      commits(last: 1) { nodes { commit { statusCheckRollup { state
        contexts(first: 50) { totalCount nodes { __typename
          ... on CheckRun { name status conclusion }
          ... on StatusContext { context state description }
        } }
      } } } }
      additions deletions changedFiles
    } }
  }
}";

pub(crate) async fn list(
    client: &GitHubClient,
    query: &PullRequestQuery,
) -> Result<Vec<PullRequestSummary>> {
    if query.top == 0 {
        return Ok(Vec::new());
    }
    match (query.scope, &query.repository_key) {
        (PullRequestScope::Repository, _) => list_repository(client, query).await,
        // The user's part of one repository: the search for several, narrowed to it.
        (PullRequestScope::CreatedByMe | PullRequestScope::ReviewRequested, Some(key)) => {
            list_repositories(
                client,
                std::slice::from_ref(key),
                query.scope,
                query.state,
                query.top,
            )
            .await
        }
        (
            PullRequestScope::Visible
            | PullRequestScope::CreatedByMe
            | PullRequestScope::ReviewRequested,
            _,
        ) => list_search(client, query).await,
    }
}

async fn list_repository(
    client: &GitHubClient,
    query: &PullRequestQuery,
) -> Result<Vec<PullRequestSummary>> {
    let key = query.repository_key.as_deref().ok_or_else(|| {
        ProviderError::Local("Listing a repository's pull requests needs a repository.".to_owned())
    })?;
    let repo = RepoKey::parse(key)?;
    let (api_state, keep): (&str, fn(&PullRequestDto) -> bool) = match query.state {
        StateFilter::Open => ("open", |_| true),
        StateFilter::All => ("all", |_| true),
        StateFilter::Merged => ("closed", |dto| dto.merged_at.is_some()),
        StateFilter::Closed => ("closed", |dto| dto.merged_at.is_none()),
    };
    let is_filtered = matches!(query.state, StateFilter::Merged | StateFilter::Closed);
    let max_pages = pages_for(query.top) + if is_filtered { MAX_FILTER_PAGES } else { 0 };
    let parameters = [
        ("state", api_state.to_owned()),
        ("sort", "created".to_owned()),
        ("direction", "desc".to_owned()),
        ("per_page", per_page(query.top)),
    ];
    let paging = Paging { limit: query.top, max_pages };
    let rows = client
        .collect_pages(
            &format!("{}/pulls", repo.api_path()),
            &parameters,
            paging,
            |page: Vec<PullRequestDto>| {
                page.into_iter()
                    .filter(keep)
                    .map(|dto| pull_request::summary(dto, &[], key))
                    .collect()
            },
        )
        .await?;
    client.forget_statuses(rows.iter().map(|row| (row.repository_key.as_str(), row.number)));
    Ok(rows)
}

async fn list_search(
    client: &GitHubClient,
    query: &PullRequestQuery,
) -> Result<Vec<PullRequestSummary>> {
    client.require_token("Listing pull requests across repositories")?;
    let (owners, mut blocked) = owners::search_owners(client).await?;
    let limit = query.top.min(MAX_SEARCH_RESULTS);
    let mut summaries: Vec<PullRequestSummary> = Vec::new();
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    let mut left_out = LeftOut::default();
    for search_query in search::queries(query.scope, query.state, &owners) {
        for summary in search_pages(client, &search_query, limit, &mut left_out).await?.rows {
            if seen.insert((summary.repository_key.clone(), summary.number)) {
                summaries.push(summary);
            }
        }
    }
    for entry in sso::name_organizations(client, &left_out.organization_ids).await {
        if !blocked.contains(&entry) {
            blocked.push(entry);
        }
    }
    if left_out.is_saml_denied && blocked.is_empty() {
        blocked.push(sso::Blocked { login: None });
    }
    client.set_partial(blocked);
    summaries.sort_by_key(|summary| std::cmp::Reverse(summary.created_at));
    summaries.truncate(limit);
    Ok(summaries)
}

/// The pull requests of several repositories through one search per
/// [`search::REPOSITORIES_PER_SEARCH`] of them (needs a token), kept to the user's part by `role`
/// (see [`search::repository_queries`]), most recently updated first, at most `top` per
/// repository. A shared search ranks all its repositories together, so one busy repository can
/// fill the page: when the search had more, the repositories that came back with fewer than `top`
/// are searched again without the full ones. Each round fills at least one of them (`top` rows
/// for each searched repository arrived), so a chunk ends after at most one round per repository.
/// What SAML single sign-on hides is not reported here: the overview's own listing reports it for
/// the connection.
pub(crate) async fn list_repositories(
    client: &GitHubClient,
    repositories: &[String],
    role: PullRequestScope,
    state: StateFilter,
    top: usize,
) -> Result<Vec<PullRequestSummary>> {
    client.require_token("Listing pull requests of several repositories at once")?;
    if top == 0 {
        return Ok(Vec::new());
    }
    let mut shares = Shares::new(top);
    let mut left_out = LeftOut::default();
    // At most MAX_SEARCH_RESULTS rows per search, so a search covers no more repositories than
    // that many rows can fill.
    let per_search = search::REPOSITORIES_PER_SEARCH.min(MAX_SEARCH_RESULTS / top).max(1);
    for chunk in repositories.chunks(per_search) {
        let mut pending = chunk.to_vec();
        while let Some(search_query) =
            search::repository_queries(&pending, role, state).into_iter().next()
        {
            let limit = (top * pending.len()).min(MAX_SEARCH_RESULTS);
            let found = search_pages(client, &search_query, limit, &mut left_out).await?;
            for summary in found.rows {
                shares.add(summary);
            }
            if !found.has_more {
                break;
            }
            let short: Vec<String> =
                pending.iter().filter(|repository| !shares.is_full(repository)).cloned().collect();
            // Only a single repository past MAX_SEARCH_RESULTS stays short: there is no more.
            if short.len() == pending.len() {
                break;
            }
            pending = short;
        }
    }
    let mut summaries = shares.rows;
    summaries
        .sort_by_key(|summary| std::cmp::Reverse(summary.updated_at.unwrap_or(summary.created_at)));
    Ok(summaries)
}

/// The rows of [`list_repositories`]: each pull request once, at most `top` per repository.
/// GitHub matches `owner/name` without regard to case, so the counts do too.
struct Shares {
    top: usize,
    rows: Vec<PullRequestSummary>,
    seen: HashSet<(String, u64)>,
    counts: HashMap<String, usize>,
}

impl Shares {
    fn new(top: usize) -> Self {
        Self { top, rows: Vec::new(), seen: HashSet::new(), counts: HashMap::new() }
    }

    fn add(&mut self, summary: PullRequestSummary) {
        let repository = summary.repository_key.to_lowercase();
        let count = self.counts.entry(repository.clone()).or_default();
        if *count < self.top && self.seen.insert((repository, summary.number)) {
            *count += 1;
            self.rows.push(summary);
        }
    }

    fn is_full(&self, repository: &str) -> bool {
        self.counts.get(&repository.to_lowercase()).is_some_and(|count| *count >= self.top)
    }
}

/// What the searches reported as left out for SAML single sign-on.
#[derive(Default)]
struct LeftOut {
    organization_ids: Vec<u64>,
    is_saml_denied: bool,
}

/// The rows of one search and whether it had more than its limit let in.
struct Found {
    rows: Vec<PullRequestSummary>,
    has_more: bool,
}

/// One search, paged with the cursor until `limit` rows arrived or the results end.
///
/// GitHub ends a GraphQL request that takes longer than ten seconds
/// (docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api, Timeouts);
/// its gateway then answers 502 or 504 with an HTML page. A fine-grained token makes each row
/// costlier to check, so a full page can time out where a smaller one passes: such a page is asked
/// again at half its size, down to [`MIN_SEARCH_PAGE`].
async fn search_pages(
    client: &GitHubClient,
    search_query: &str,
    limit: usize,
    left_out: &mut LeftOut,
) -> Result<Found> {
    let document = [SEARCH_QUERY, ACTOR_FRAGMENT].concat();
    let mut summaries = Vec::new();
    let mut cursor: Option<String> = None;
    let mut page_size = crate::client::PER_PAGE;
    while summaries.len() < limit {
        let first = (limit - summaries.len()).min(page_size);
        let variables = json!({ "q": search_query, "first": first, "cursor": cursor });
        let answer: Partial<SearchData> =
            match graphql::execute_partial(client, "Searching pull requests", &document, variables)
                .await
            {
                Ok(answer) => answer,
                Err(error) if is_gateway_timeout(&error) && first / 2 >= MIN_SEARCH_PAGE => {
                    page_size = first / 2;
                    continue;
                }
                Err(error) => return Err(error),
            };
        for id in answer.blocked_ids {
            if !left_out.organization_ids.contains(&id) {
                left_out.organization_ids.push(id);
            }
        }
        left_out.is_saml_denied |= answer.is_saml_denied;
        let (page, rows) = answer.data.search.into_nodes();
        let (rows, statuses): (Vec<PullRequestSummary>, Vec<_>) = rows
            .into_iter()
            .map(search::summary)
            .map(|(row, status)| {
                let key = (row.repository_key.clone(), row.number);
                (row, (key, status))
            })
            .unzip();
        client.remember_statuses(statuses);
        summaries.extend(rows);
        cursor = page.end_cursor.filter(|_| page.has_next_page);
        if cursor.is_none() {
            break;
        }
    }
    Ok(Found { has_more: cursor.is_some(), rows: summaries })
}

/// 502 Bad Gateway or 504 Gateway Timeout: the request ran out of time behind GitHub's gateway.
fn is_gateway_timeout(error: &ProviderError) -> bool {
    matches!(error.status(), Some(502 | 504))
}

pub(crate) async fn fetch(
    client: &GitHubClient,
    repo: &RepoKey<'_>,
    number: u64,
) -> Result<PullRequestDto> {
    client.get_json(&repo.pull_path(number)).await
}

/// Line and file counts of rows the REST listing brought, which leaves them out: one
/// `GET /repos/{o}/{r}/pulls/{n}` each. Search rows carry them already.
pub(crate) async fn diff_stats(
    client: &GitHubClient,
    key: &str,
    numbers: &[u64],
) -> Result<Vec<(u64, DiffStat)>> {
    let repo = RepoKey::parse(key)?;
    let mut stats = Vec::new();
    for &number in numbers {
        let dto = fetch(client, &repo, number).await?;
        stats.push((number, pull_request::summary(dto, &[], key).diff_stat));
    }
    Ok(stats)
}

pub(crate) async fn detail(
    client: &GitHubClient,
    key: &str,
    number: u64,
) -> Result<PullRequestDetail> {
    let repo = RepoKey::parse(key)?;
    let (dto, reviews) =
        tokio::try_join!(fetch(client, &repo, number), reviews::list(client, &repo, number))?;
    Ok(pull_request::detail(dto, &reviews, key))
}
