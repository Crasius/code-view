//! Reviews: listing, voting and reviewer requests.

use reqwest::Method;
use review_core::{ProviderError, Result, ReviewVote, http};
use serde_json::{Value, json};

use crate::client::{GitHubClient, PER_PAGE, Paging};
use crate::dto::review::ReviewDto;
use crate::dto::user::RequestedReviewersDto;
use crate::mapping::identity::TEAM_ID_PREFIX;
use crate::repo_key::RepoKey;

const MAX_REVIEW_PAGES: usize = 30;

/// Every review of the pull request, oldest first.
pub(crate) async fn list(
    client: &GitHubClient,
    repo: &RepoKey<'_>,
    number: u64,
) -> Result<Vec<ReviewDto>> {
    let path = format!("{}/reviews", repo.pull_path(number));
    let query = [("per_page", PER_PAGE.to_string())];
    client
        .collect_pages(&path, &query, Paging::pages(MAX_REVIEW_PAGES), |page: Vec<ReviewDto>| page)
        .await
}

pub(crate) async fn set_vote(
    client: &GitHubClient,
    key: &str,
    number: u64,
    vote: ReviewVote,
    comment: Option<&str>,
) -> Result<()> {
    client.require_token("Voting")?;
    let repo = RepoKey::parse(key)?;
    let body = vote_body(vote, comment)?;
    let request =
        client.rest(Method::POST, &format!("{}/reviews", repo.pull_path(number))).json(&body);
    http::unit(request).await.map_err(refused_review)
}

/// GitHub answers 422 "Unprocessable Entity" and says why only in `errors` (seen live: "Review Can
/// not approve your own pull request"), so the reason a vote failed names those.
fn refused_review(error: ProviderError) -> ProviderError {
    let ProviderError::Status { status: 422, body, .. } = &error else {
        return error;
    };
    let reasons = review_errors(body);
    if reasons.is_empty() {
        return error;
    }
    ProviderError::Local(format!("GitHub did not take the review: {}.", reasons.join("; ")))
}

/// The `errors` of a GitHub error body: plain strings, or objects with a `message`.
fn review_errors(body: &str) -> Vec<String> {
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let errors = parsed.get("errors").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    errors
        .iter()
        .filter_map(|error| error.as_str().or_else(|| error.get("message")?.as_str()))
        .map(str::to_owned)
        .collect()
}

/// `POST /pulls/{n}/reviews` payload; GitHub has no way to reset an opinion through this endpoint.
fn vote_body(vote: ReviewVote, comment: Option<&str>) -> Result<Value> {
    let event = match vote {
        ReviewVote::Approved => "APPROVE",
        ReviewVote::ChangesRequested => "REQUEST_CHANGES",
        other => {
            return Err(ProviderError::Unsupported(format!(
                "GitHub reviews cannot cast {other:?}."
            )));
        }
    };
    let comment = comment.filter(|comment| !comment.trim().is_empty());
    let mut body = json!({ "event": event });
    match comment {
        Some(comment) => body["body"] = Value::from(comment),
        // GitHub answers 422 when REQUEST_CHANGES comes without a body.
        None if vote == ReviewVote::ChangesRequested => {
            return Err(ProviderError::Local(
                "Requesting changes on GitHub needs a comment.".to_owned(),
            ));
        }
        None => {}
    }
    Ok(body)
}

pub(crate) async fn add_reviewer(
    client: &GitHubClient,
    key: &str,
    number: u64,
    user: &str,
) -> Result<()> {
    client.require_token("Adding reviewers")?;
    let repo = RepoKey::parse(key)?;
    let body = reviewer_request_body(user)?;
    let request = client
        .rest(Method::POST, &format!("{}/requested_reviewers", repo.pull_path(number)))
        .json(&body);
    http::unit(request).await.map_err(|error| refused_request(error, user, key))
}

/// GitHub documents 422 "if user is not a collaborator"
/// (<https://docs.github.com/rest/pulls/review-requests#request-reviewers-for-a-pull-request>); the
/// docs say nothing about asking the author or oneself, so GitHub's own words close the message.
fn refused_request(error: ProviderError, user: &str, repository: &str) -> ProviderError {
    if error.status() != Some(422) {
        return error;
    }
    let who = user.trim().trim_start_matches('@');
    let github_says = error.server_message().unwrap_or_default();
    ProviderError::Local(format!(
        "GitHub did not request a review from {who}: reviewers must be collaborators on \
         {repository}, and an author cannot review their own pull request. GitHub says: \
         {github_says}"
    ))
}

/// `org/team` requests a team by slug, anything else a user by login (a leading `@` is ignored).
fn reviewer_request_body(input: &str) -> Result<Value> {
    let input = input.trim().trim_start_matches('@');
    if input.is_empty() {
        return Err(ProviderError::Local("Enter a login or an org/team name.".to_owned()));
    }
    match input.split_once('/') {
        Some((_, slug)) if !slug.is_empty() => Ok(json!({ "team_reviewers": [slug] })),
        Some(_) => Err(ProviderError::Local(format!("'{input}' is not an org/team name."))),
        None => Ok(json!({ "reviewers": [input] })),
    }
}

/// Only pending requests can be removed; GitHub keeps submitted reviews.
pub(crate) async fn remove_reviewer(
    client: &GitHubClient,
    key: &str,
    number: u64,
    reviewer_id: &str,
) -> Result<()> {
    client.require_token("Removing reviewers")?;
    let repo = RepoKey::parse(key)?;
    let path = format!("{}/requested_reviewers", repo.pull_path(number));
    let requested: RequestedReviewersDto = client.get_json(&path).await?;
    let body = removal_body(&requested, reviewer_id).ok_or_else(|| {
        ProviderError::Local(
            "Only reviewers with a pending request can be removed on GitHub.".to_owned(),
        )
    })?;
    http::unit(client.rest(Method::DELETE, &path).json(&body)).await
}

fn removal_body(requested: &RequestedReviewersDto, reviewer_id: &str) -> Option<Value> {
    if let Some(team_id) = reviewer_id.strip_prefix(TEAM_ID_PREFIX) {
        let team = requested.teams.iter().find(|team| team.id.to_string() == team_id)?;
        return Some(json!({ "reviewers": [], "team_reviewers": [team.slug] }));
    }
    let user = requested.users.iter().find(|user| user.id.to_string() == reviewer_id)?;
    Some(json!({ "reviewers": [user.login] }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::fixtures::parse;

    #[test]
    fn blank_comments_are_dropped_and_changes_need_one() {
        assert_eq!(
            vote_body(ReviewVote::Approved, Some("  ")).expect("body"),
            json!({ "event": "APPROVE" })
        );
        assert!(matches!(
            vote_body(ReviewVote::ChangesRequested, Some(" ")),
            Err(ProviderError::Local(_))
        ));
    }

    #[test]
    fn blank_or_slugless_reviewer_inputs_are_rejected() {
        assert!(reviewer_request_body(" ").is_err());
        assert!(reviewer_request_body("contoso/").is_err());
    }

    #[test]
    fn ids_without_a_pending_request_resolve_to_no_removal() {
        let requested: RequestedReviewersDto = parse(
            r#"{"users":[{"id":3,"login":"cy"}],"teams":[{"id":77,"name":"Maintainers","slug":"maintainers"}]}"#,
        );
        assert_eq!(removal_body(&requested, "4"), None);
        assert_eq!(removal_body(&requested, "team:3"), None);
    }
}
