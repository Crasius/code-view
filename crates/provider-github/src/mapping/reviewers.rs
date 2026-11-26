//! Reviewer lists: requested users and teams merged with each user's latest review opinion.

use review_core::{ReviewVote, Reviewer, UserIdentity};

use crate::dto::review::ReviewDto;
use crate::dto::search::{OpinionatedReviewDto, RequestedReviewerDto, ReviewRequestDto};
use crate::dto::user::{TeamDto, UserDto};
use crate::mapping::identity;

/// GitHub review state → vote. Only `APPROVED` and `CHANGES_REQUESTED` carry an opinion.
pub(crate) fn vote(state: &str) -> ReviewVote {
    match state {
        "APPROVED" => ReviewVote::Approved,
        "CHANGES_REQUESTED" => ReviewVote::ChangesRequested,
        _ => ReviewVote::None,
    }
}

/// Latest opinion per user, in order of first review. A later `COMMENTED` review keeps an
/// earlier approval (as GitHub does); `DISMISSED` resets it; `PENDING` reviews are invisible to
/// others.
pub(crate) fn latest_votes(reviews: &[ReviewDto]) -> Vec<(UserIdentity, ReviewVote)> {
    let mut votes: Vec<(UserIdentity, ReviewVote)> = Vec::new();
    for review in reviews.iter().filter(|review| review.state != "PENDING") {
        let reviewer = identity::optional_user(review.user.clone());
        let opinion = (review.state != "COMMENTED").then(|| vote(&review.state));
        match votes.iter_mut().find(|(existing, _)| existing.id == reviewer.id) {
            Some((_, current)) => *current = opinion.unwrap_or(*current),
            None => votes.push((reviewer, opinion.unwrap_or(ReviewVote::None))),
        }
    }
    votes
}

/// Reviewers of a REST pull request: everyone who reviewed, then pending user requests, then teams.
pub(crate) fn from_rest(
    requested_users: Vec<UserDto>,
    requested_teams: Vec<TeamDto>,
    reviews: &[ReviewDto],
) -> Vec<Reviewer> {
    let users = requested_users.into_iter().map(identity::user).collect();
    let teams = requested_teams.into_iter().map(identity::team).collect();
    merge(latest_votes(reviews), users, teams)
}

/// Reviewers of a GraphQL search row (`reviewRequests` + `latestOpinionatedReviews`).
pub(crate) fn from_search(
    requests: Vec<ReviewRequestDto>,
    opinions: Vec<OpinionatedReviewDto>,
) -> Vec<Reviewer> {
    let voted = opinions
        .into_iter()
        .map(|review| (identity::actor(review.author), vote(&review.state)))
        .collect();
    let mut users = Vec::new();
    let mut teams = Vec::new();
    for request in requests {
        match request.requested_reviewer {
            Some(RequestedReviewerDto::User(actor)) => users.push(identity::actor(Some(actor))),
            Some(RequestedReviewerDto::Team(team)) => teams.push(identity::team_node(team)),
            Some(RequestedReviewerDto::Other) | None => {}
        }
    }
    merge(voted, users, teams)
}

fn merge(
    voted: Vec<(UserIdentity, ReviewVote)>,
    users: Vec<UserIdentity>,
    teams: Vec<UserIdentity>,
) -> Vec<Reviewer> {
    let mut reviewers: Vec<Reviewer> = voted
        .into_iter()
        .map(|(identity, vote)| Reviewer { identity, vote, is_required: false, is_group: false })
        .collect();
    for user in users {
        if reviewers.iter().all(|reviewer| reviewer.identity.id != user.id) {
            reviewers.push(Reviewer {
                identity: user,
                vote: ReviewVote::None,
                is_required: false,
                is_group: false,
            });
        }
    }
    reviewers.extend(teams.into_iter().map(|identity| Reviewer {
        identity,
        vote: ReviewVote::None,
        is_required: false,
        is_group: true,
    }));
    reviewers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::fixtures::parse;

    const REVIEWS: &str = r#"[
        {"id":1,"user":{"id":1,"login":"ann"},"state":"APPROVED","body":"","submitted_at":"2024-01-01T10:00:00Z"},
        {"id":2,"user":{"id":1,"login":"ann"},"state":"COMMENTED","body":"nit","submitted_at":"2024-01-01T11:00:00Z"},
        {"id":3,"user":{"id":2,"login":"bob"},"state":"CHANGES_REQUESTED","body":"no","submitted_at":"2024-01-01T12:00:00Z"},
        {"id":4,"user":{"id":3,"login":"cy"},"state":"APPROVED","body":"","submitted_at":"2024-01-01T12:00:00Z"},
        {"id":5,"user":{"id":3,"login":"cy"},"state":"DISMISSED","body":"","submitted_at":"2024-01-01T13:00:00Z"},
        {"id":6,"user":{"id":4,"login":"dee"},"state":"PENDING","body":"","submitted_at":null}
    ]"#;

    #[test]
    fn latest_opinion_per_user() {
        let reviews: Vec<ReviewDto> = parse(REVIEWS);
        let votes: Vec<(String, ReviewVote)> =
            latest_votes(&reviews).into_iter().map(|(user, vote)| (user.id, vote)).collect();
        assert_eq!(
            votes,
            vec![
                ("1".to_owned(), ReviewVote::Approved),
                ("2".to_owned(), ReviewVote::ChangesRequested),
                ("3".to_owned(), ReviewVote::None)
            ]
        );
    }

    #[test]
    fn requested_users_without_review_are_appended_once() {
        let reviews: Vec<ReviewDto> = parse(REVIEWS);
        let users: Vec<UserDto> = parse(r#"[{"id":2,"login":"bob"},{"id":5,"login":"eve"}]"#);
        let teams: Vec<TeamDto> = parse(r#"[{"id":10,"name":"Core","slug":"core"}]"#);
        let reviewers = from_rest(users, teams, &reviews);
        let ids: Vec<&str> =
            reviewers.iter().map(|reviewer| reviewer.identity.id.as_str()).collect();
        assert_eq!(ids, vec!["1", "2", "3", "5", "team:10"]);
        assert_eq!(reviewers[1].vote, ReviewVote::ChangesRequested);
        assert!(reviewers[4].is_group);
        assert_eq!(reviewers[4].identity.login.as_deref(), Some("core"), "team login is its slug");
        assert_eq!(reviewers[3].vote, ReviewVote::None);
    }

    #[test]
    fn search_reviewers_merge_requests_and_opinions() {
        let requests: Vec<ReviewRequestDto> = parse(
            r#"[{"requestedReviewer":{"__typename":"User","login":"eve","databaseId":5}},
                {"requestedReviewer":{"__typename":"Team","databaseId":10,"name":"Core","slug":"core"}},
                {"requestedReviewer":{"__typename":"Mannequin","login":"old"}},
                {"requestedReviewer":null}]"#,
        );
        let opinions: Vec<OpinionatedReviewDto> =
            parse(r#"[{"state":"APPROVED","author":{"login":"ann","databaseId":1}}]"#);
        let reviewers = from_search(requests, opinions);
        let summary: Vec<(&str, ReviewVote, bool)> = reviewers
            .iter()
            .map(|reviewer| (reviewer.identity.id.as_str(), reviewer.vote, reviewer.is_group))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("1", ReviewVote::Approved, false),
                ("5", ReviewVote::None, false),
                ("team:10", ReviewVote::None, true)
            ]
        );
        assert_eq!(reviewers[2].identity.login.as_deref(), Some("core"), "team login is its slug");
    }
}
