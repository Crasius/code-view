//! The checks of list rows ([`review_core::Provider::row_statuses`]).
//!
//! Search rows bring theirs: the search query selects the head commit's check rollup with its
//! first page of check runs and statuses, and the review decision, and the client keeps them
//! ([`GitHubClient::listed_status`]), so those rows cost no request. Rows of a repository's REST
//! listing have neither checks nor votes; one GraphQL request per [`ROWS_PER_QUERY`] of them asks
//! each pull request under an alias (`pr7: pullRequest(number: 7)`), which also tells the required
//! checks (`isRequired` takes the pull request as an argument, so only a query that names it can
//! ask) and the reviewers' latest opinions. A number GitHub does not know comes back as `null` with
//! a `NOT_FOUND` error and is left out ([`graphql::execute_found`]). GraphQL needs a token:
//! anonymous rows report nothing.

use std::collections::HashMap;
use std::fmt::Write as _;

use review_core::{Result, RowStatus};
use serde::Deserialize;
use serde_json::json;

use crate::client::GitHubClient;
use crate::dto::search::{HeadCommitNodeDto, OpinionatedReviewDto, ReviewRequestDto};
use crate::graphql::{self, ACTOR_FRAGMENT, Connection};
use crate::mapping::{checks, reviewers};
use crate::repo_key::RepoKey;

/// Pull requests one query asks for; each alias selects a few small connections, so 25 keep the
/// query far below GitHub's node limit and its rate-limit cost at one point.
const ROWS_PER_QUERY: usize = 25;

#[derive(Debug, Deserialize)]
struct Data {
    repository: Option<HashMap<String, Option<RowDto>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RowDto {
    number: u64,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    review_requests: Option<Connection<ReviewRequestDto>>,
    #[serde(default)]
    latest_opinionated_reviews: Option<Connection<OpinionatedReviewDto>>,
    #[serde(default)]
    commits: Option<Connection<HeadCommitNodeDto>>,
}

pub(crate) async fn row_statuses(
    client: &GitHubClient,
    repo: &str,
    numbers: &[u64],
) -> Result<Vec<(u64, RowStatus)>> {
    let mut statuses = Vec::new();
    let mut unlisted = Vec::new();
    for &number in numbers {
        match client.listed_status(repo, number) {
            Some(status) => statuses.push((number, status)),
            None => unlisted.push(number),
        }
    }
    if unlisted.is_empty() || !client.has_token() {
        return Ok(statuses);
    }
    let key = RepoKey::parse(repo)?;
    for chunk in unlisted.chunks(ROWS_PER_QUERY) {
        let document = [query(chunk).as_str(), ACTOR_FRAGMENT].concat();
        let variables = json!({ "owner": key.owner, "name": key.name });
        let data: Data = graphql::execute_found(
            client,
            "Loading the checks of pull requests",
            &document,
            variables,
        )
        .await?;
        let rows = data.repository.into_iter().flat_map(HashMap::into_values).flatten();
        statuses.extend(rows.map(|row| (row.number, row_status(row))));
    }
    Ok(statuses)
}

fn row_status(row: RowDto) -> RowStatus {
    let rollup = row
        .commits
        .and_then(|commits| commits.into_nodes().1.into_iter().next())
        .and_then(|node| node.commit.status_check_rollup);
    let requests = row.review_requests.map(|requests| requests.into_nodes().1).unwrap_or_default();
    let opinions =
        row.latest_opinionated_reviews.map(|reviews| reviews.into_nodes().1).unwrap_or_default();
    RowStatus {
        reviewers: Some(reviewers::from_search(requests, opinions)),
        ..checks::row_status(rollup, row.review_decision.as_deref())
    }
}

/// One alias per pull request of `numbers`.
fn query(numbers: &[u64]) -> String {
    let mut document = String::from(
        "query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) {",
    );
    for number in numbers {
        let _infallible = write!(
            document,
            " pr{number}: pullRequest(number: {number}) {{ number reviewDecision
  reviewRequests(first: 20) {{ nodes {{ requestedReviewer {{ __typename
    ... on User {{ login avatarUrl databaseId name }} ... on Team {{ databaseId name slug }} }} }} }}
  latestOpinionatedReviews(first: 20) {{ nodes {{ state author {{ ...actor }} }} }}
  commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ state
    contexts(first: 50) {{ totalCount nodes {{ __typename
      ... on CheckRun {{ name status conclusion isRequired(pullRequestNumber: {number}) }}
      ... on StatusContext {{ context state description isRequired(pullRequestNumber: {number}) }}
    }} }} }} }} }} }} }}"
        );
    }
    document.push_str(" } }");
    document
}

#[cfg(test)]
mod tests {
    use review_core::{CheckState, ReviewVote};

    use super::*;

    #[test]
    fn each_pull_request_gets_an_alias_that_names_it_for_its_required_checks() {
        let document = query(&[7, 12]);
        assert!(document.contains("pr7: pullRequest(number: 7)"), "{document}");
        assert!(document.contains("isRequired(pullRequestNumber: 12)"), "{document}");
        assert_eq!(document.matches("pullRequest(number:").count(), 2);
    }

    #[test]
    fn maps_aliased_rows_with_checks_and_votes() {
        let row: RowDto = serde_json::from_str(
            r#"{"number": 7, "reviewDecision": "APPROVED",
                "reviewRequests": {"nodes": []},
                "latestOpinionatedReviews": {"nodes": [{"state": "APPROVED", "author": {"login": "ann", "databaseId": 1}}]},
                "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS", "contexts": {"totalCount": 1,
                    "nodes": [{"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS", "isRequired": true}]}}}}]}}"#,
        )
        .expect("fixture");
        let status = row_status(row);
        assert_eq!(status.verdict(), Some(CheckState::Passed));
        assert_eq!(status.checks[0].is_required, Some(true));
        assert_eq!(status.passed_of(), (1, 1), "the review decision is no check");
        assert_eq!(
            status.required_approvals.map(|review| review.state),
            Some(CheckState::Passed),
            "the review decision"
        );
        let votes: Vec<ReviewVote> =
            status.reviewers.expect("votes").iter().map(|reviewer| reviewer.vote).collect();
        assert_eq!(votes, [ReviewVote::Approved]);
    }
}
