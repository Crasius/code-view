use super::{CheckState, UserIdentity};
use serde::{Deserialize, Serialize};

/// Azure DevOps vote scale; GitHub and GitLab map onto it.
///
/// * GitHub `APPROVED` → `Approved`, `CHANGES_REQUESTED` → `ChangesRequested`, anything else → `None`.
/// * GitLab approved → `Approved`, reviewer state `requested_changes` → `ChangesRequested`.
/// * Azure DevOps 10 / 5 / 0 / -5 / -10 → `Approved` / `ApprovedWithSuggestions` / `None` / `WaitingForAuthor` / `ChangesRequested` ("Rejected").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ReviewVote {
    #[default]
    None,
    Approved,
    ApprovedWithSuggestions,
    WaitingForAuthor,
    ChangesRequested,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reviewer {
    pub identity: UserIdentity,
    pub vote: ReviewVote,
    /// Azure DevOps required reviewers, or code owners where the platform reports it.
    pub is_required: bool,
    /// Team or group reviewer (GitHub team, Azure DevOps group).
    pub is_group: bool,
}

/// Whether a pull request counts as approved: nobody blocks it (changes requested, rejected,
/// waiting for the author), and the approvals the platform requires are in (`required` passed) or,
/// where it requires none or does not say, someone approved. Missing required approvals keep it
/// unapproved even with approving votes.
pub fn is_approved(reviewers: &[Reviewer], required: Option<CheckState>) -> bool {
    let blocks = reviewers.iter().any(|reviewer| {
        matches!(reviewer.vote, ReviewVote::ChangesRequested | ReviewVote::WaitingForAuthor)
    });
    if blocks {
        return false;
    }
    match required {
        Some(state) => state == CheckState::Passed,
        None => reviewers.iter().any(|reviewer| {
            matches!(reviewer.vote, ReviewVote::Approved | ReviewVote::ApprovedWithSuggestions)
        }),
    }
}

/// A label (GitHub/GitLab) or tag (Azure DevOps). `color` is `RRGGBB` hex without `#` when known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voted(votes: &[ReviewVote]) -> Vec<Reviewer> {
        votes
            .iter()
            .map(|vote| Reviewer {
                identity: UserIdentity::default(),
                vote: *vote,
                is_required: false,
                is_group: false,
            })
            .collect()
    }

    #[test]
    fn approved_needs_the_required_approvals_and_nobody_blocking() {
        use CheckState::{Failed, Passed, Pending};
        use ReviewVote::{
            Approved, ApprovedWithSuggestions, ChangesRequested, None as NoVote, WaitingForAuthor,
        };
        let cases: [(&[ReviewVote], Option<CheckState>, bool); 9] = [
            (&[Approved], None, true),
            (&[ApprovedWithSuggestions, NoVote], None, true),
            (&[NoVote], None, false),
            (&[], None, false),
            (&[Approved, ChangesRequested], None, false),
            (&[Approved, WaitingForAuthor], None, false),
            (&[Approved], Some(Pending), false),
            (&[], Some(Passed), true),
            (&[Approved], Some(Failed), false),
        ];
        for (votes, required, expected) in cases {
            assert_eq!(is_approved(&voted(votes), required), expected, "{votes:?} {required:?}");
        }
    }
}
