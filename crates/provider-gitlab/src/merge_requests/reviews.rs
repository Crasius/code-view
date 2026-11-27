//! Approvals, reviewer votes and reviewer assignment.

use reqwest::Method;
use review_core::{ProviderError, Result, ReviewVote, Reviewer};
use serde_json::json;

use crate::client::{ApiClient, merge_request_path};
use crate::dto::{ApprovalsDto, ReviewerDto, UserDto};
use crate::merge_requests::fetch;

/// Approve and unapprove only: GitLab's REST API has no "request changes" endpoint
/// (the reviewer state `requested_changes` is set through the UI or GraphQL).
pub(crate) async fn set_vote(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    vote: ReviewVote,
    comment: Option<&str>,
) -> Result<()> {
    client.require_token("Voting")?;
    let action = match vote {
        ReviewVote::Approved => "approve",
        ReviewVote::None => "unapprove",
        other => {
            return Err(ProviderError::Unsupported(format!(
                "GitLab's API cannot cast the vote {other:?}."
            )));
        }
    };
    let path = merge_request_path(repo, iid);
    client.unit(client.request(Method::POST, &format!("{path}/{action}"))).await?;
    if let Some(body) = comment.map(str::trim).filter(|body| !body.is_empty()) {
        let request =
            client.request(Method::POST, &format!("{path}/notes")).json(&json!({ "body": body }));
        client.unit(request).await?;
    }
    Ok(())
}

pub(crate) async fn add_reviewer(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    user: &str,
) -> Result<()> {
    client.require_token("Adding a reviewer")?;
    let (user, merge_request) =
        tokio::try_join!(resolve_user(client, user), fetch(client, repo, iid))?;
    let mut ids: Vec<u64> = merge_request.reviewers.iter().map(|reviewer| reviewer.id).collect();
    if ids.contains(&user.id) {
        return Ok(());
    }
    ids.push(user.id);
    put_reviewer_ids(client, repo, iid, &ids).await
}

pub(crate) async fn remove_reviewer(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    reviewer_id: &str,
) -> Result<()> {
    client.require_token("Removing a reviewer")?;
    let reviewer_id: u64 = reviewer_id
        .parse()
        .map_err(|_| ProviderError::Local(format!("'{reviewer_id}' is not a GitLab user id.")))?;
    let merge_request = fetch(client, repo, iid).await?;
    let ids: Vec<u64> = merge_request
        .reviewers
        .iter()
        .map(|reviewer| reviewer.id)
        .filter(|id| *id != reviewer_id)
        .collect();
    put_reviewer_ids(client, repo, iid, &ids).await
}

async fn put_reviewer_ids(client: &ApiClient, repo: &str, iid: u64, ids: &[u64]) -> Result<()> {
    let request = client
        .request(Method::PUT, &merge_request_path(repo, iid))
        .json(&json!({ "reviewer_ids": ids }));
    client.unit(request).await
}

/// Exact username first (`@` stripped), then a search that must match one user by username or e-mail.
async fn resolve_user(client: &ApiClient, input: &str) -> Result<UserDto> {
    let wanted = input.trim().trim_start_matches('@');
    if wanted.is_empty() {
        return Err(ProviderError::Local("Enter a GitLab username.".to_owned()));
    }
    let exact: Vec<UserDto> = client.get("/users", &[("username", wanted.to_owned())]).await?;
    if let Some(user) = exact.into_iter().next() {
        return Ok(user);
    }
    let found: Vec<UserDto> = client
        .get("/users", &[("search", wanted.to_owned()), ("per_page", "20".to_owned())])
        .await?;
    pick_user(found, wanted)
        .ok_or_else(|| ProviderError::Local(format!("No unique GitLab user matches '{wanted}'.")))
}

fn pick_user(candidates: Vec<UserDto>, wanted: &str) -> Option<UserDto> {
    let matches_exactly = |user: &UserDto| {
        user.username.eq_ignore_ascii_case(wanted)
            || user.public_email.as_deref().is_some_and(|email| email.eq_ignore_ascii_case(wanted))
    };
    if let Some(user) = candidates.iter().find(|user| matches_exactly(user)) {
        return Some(user.clone());
    }
    match <[UserDto; 1]>::try_from(candidates) {
        Ok([only]) => Some(only),
        Err(_) => None,
    }
}

/// Reviewers with votes. `reviewers` (from `/reviewers`) carries review states; when it is empty
/// the merge request's plain `reviewers` list stands in.
pub(crate) fn combine_reviewers(
    reviewers: &[ReviewerDto],
    approvals: &ApprovalsDto,
    listed: &[UserDto],
) -> Vec<Reviewer> {
    let mut combined: Vec<Reviewer> = if reviewers.is_empty() {
        listed.iter().map(|user| reviewer(user, ReviewVote::None)).collect()
    } else {
        reviewers
            .iter()
            .map(|entry| reviewer(&entry.user, state_vote(entry.state.as_deref())))
            .collect()
    };
    let approvers =
        approvals.approved_by.iter().map(|approval| reviewer(&approval.user, ReviewVote::Approved));
    add_approvers(&mut combined, approvers);
    combined
}

/// Approvers who are not reviewers are added, since an approval is a vote; a reviewer who
/// approved gets the vote and, when it has none, the approval's avatar. The detail and the list
/// rows show the same votes through this one rule.
pub(crate) fn add_approvers(
    voted: &mut Vec<Reviewer>,
    approvers: impl IntoIterator<Item = Reviewer>,
) {
    for approved in approvers {
        match voted.iter_mut().find(|known| known.identity.id == approved.identity.id) {
            Some(known) => {
                known.vote = ReviewVote::Approved;
                if known.identity.avatar_url.is_none() {
                    known.identity.avatar_url = approved.identity.avatar_url;
                }
            }
            None => voted.push(approved),
        }
    }
}

fn reviewer(user: &UserDto, vote: ReviewVote) -> Reviewer {
    Reviewer { identity: user.to_identity(), vote, is_required: false, is_group: false }
}

fn state_vote(state: Option<&str>) -> ReviewVote {
    match state {
        Some("approved") => ReviewVote::Approved,
        Some("requested_changes") => ReviewVote::ChangesRequested,
        _ => ReviewVote::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: u64, username: &str) -> UserDto {
        UserDto {
            id,
            username: username.to_owned(),
            name: None,
            avatar_url: None,
            public_email: None,
        }
    }

    #[test]
    fn maps_reviewer_states_and_adds_approvers() {
        let reviewers: Vec<ReviewerDto> = serde_json::from_str(
            r#"[{"user":{"id":1,"username":"ann","name":"Ann"},"state":"requested_changes"},
                {"user":{"id":2,"username":"bob","name":"Bob"},"state":"unreviewed"},
                {"user":{"id":3,"username":"cy","name":"Cy"},"state":"approved"}]"#,
        )
        .expect("fixture");
        let approvals: ApprovalsDto =
            serde_json::from_str(r#"{"approved_by":[{"user":{"id":2,"username":"bob"}},{"user":{"id":9,"username":"dee"}}]}"#)
                .expect("fixture");
        let combined = combine_reviewers(&reviewers, &approvals, &[]);
        let votes: Vec<(&str, ReviewVote)> = combined
            .iter()
            .map(|reviewer| (reviewer.identity.display_name.as_str(), reviewer.vote))
            .collect();
        assert_eq!(
            votes,
            vec![
                ("Ann", ReviewVote::ChangesRequested),
                ("Bob", ReviewVote::Approved),
                ("Cy", ReviewVote::Approved),
                ("dee", ReviewVote::Approved),
            ]
        );
    }

    #[test]
    fn a_reviewer_without_an_avatar_gets_the_approvals() {
        let reviewers: Vec<ReviewerDto> =
            serde_json::from_str(r#"[{"user":{"id":2,"username":"bob"},"state":"unreviewed"}]"#)
                .expect("fixture");
        let approvals: ApprovalsDto = serde_json::from_str(
            r#"{"approved_by":[{"user":{"id":2,"username":"bob","avatar_url":"https://gitlab.example.com/bob.png"}}]}"#,
        )
        .expect("fixture");
        let combined = combine_reviewers(&reviewers, &approvals, &[]);
        assert_eq!(combined.len(), 1);
        assert_eq!(combined[0].vote, ReviewVote::Approved);
        assert_eq!(
            combined[0].identity.avatar_url.as_deref(),
            Some("https://gitlab.example.com/bob.png")
        );
    }

    #[test]
    fn falls_back_to_listed_reviewers() {
        let combined = combine_reviewers(&[], &ApprovalsDto::default(), &[user(5, "eve")]);
        assert_eq!(combined.len(), 1);
        assert_eq!(combined[0].identity.id, "5");
        assert_eq!(combined[0].vote, ReviewVote::None);
    }

    #[test]
    fn picks_exact_or_single_user() {
        let candidates = vec![user(1, "anna"), user(2, "ann")];
        assert_eq!(pick_user(candidates, "ANN").map(|user| user.id), Some(2));
        assert_eq!(pick_user(vec![user(3, "annabel")], "ann").map(|user| user.id), Some(3));
        assert!(pick_user(vec![user(1, "anna"), user(3, "annabel")], "ann").is_none());
    }
}
