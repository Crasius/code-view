//! How likely the user opens a listed pull request, from what its row knows: its state, draft or
//! ready, its approvals and the user's own part in it. The list reads pull requests ahead by it
//! ([`super::warm`]): the likelier the open, the deeper.
//!
//! Our own judgement, not a measurement: a review asked of the user and not given is what they
//! came for; an open pull request is looked at; a draft, one approved already (the user's
//! approval or everyone's) is looked at less; a merged or closed one rarely.

use cv_services::self_review::is_same_person;
use review_core::{
    Check, PullRequestState, PullRequestSummary, ReviewVote, Reviewer, UserIdentity, is_approved,
};

/// How likely the user opens a pull request, likeliest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interest {
    /// The user's review is asked for and not given.
    Waiting,
    /// Open and ready, or the user's own.
    Open,
    /// A draft, or approved: by the user, or as far as the platform asks.
    Settled,
    /// Merged or closed.
    Done,
}

/// How much of a pull request's page is read ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Depth {
    Nothing,
    /// The detail (description and all) and the images its description shows.
    Meta,
    /// [`Depth::Meta`], the commits in the mirror and, for small pull requests, the counted files.
    Full,
}

impl Interest {
    /// The interest in `summary`, with `reviewers` (the listing's or the votes read since),
    /// the approvals the platform requires, and `me`, the signed-in user once known.
    pub fn of(
        summary: &PullRequestSummary,
        reviewers: &[Reviewer],
        required: Option<&Check>,
        me: Option<&UserIdentity>,
    ) -> Self {
        if summary.state != PullRequestState::Open {
            return Self::Done;
        }
        let is_mine = me.is_some_and(|me| is_same_person(&summary.author, me));
        if is_mine {
            return Self::Open;
        }
        if summary.is_draft {
            return Self::Settled;
        }
        let my_vote = me.and_then(|me| {
            let reviewer = reviewers
                .iter()
                .find(|reviewer| !reviewer.is_group && is_same_person(&reviewer.identity, me))?;
            Some(reviewer.vote)
        });
        match my_vote {
            Some(ReviewVote::None) => Self::Waiting,
            Some(ReviewVote::Approved | ReviewVote::ApprovedWithSuggestions) => Self::Settled,
            _ if is_approved(reviewers, required.map(|check| check.state)) => Self::Settled,
            _ => Self::Open,
        }
    }

    /// How deep to read ahead: on the row the pointer rests on (`is_focus`), or on a row next to
    /// it. Git work (a fetch, blobs) only where an open is likely; nothing for a merged or closed
    /// pull request beside the pointer.
    pub fn depth(self, is_focus: bool) -> Depth {
        match (self, is_focus) {
            (Self::Waiting, _) | (Self::Open, true) => Depth::Full,
            (Self::Open | Self::Settled, _) | (Self::Done, true) => Depth::Meta,
            (Self::Done, false) => Depth::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use review_core::{
        Check, CheckState, PullRequestState, PullRequestSummary, ReviewVote, Reviewer, UserIdentity,
    };

    use super::{Depth, Interest};

    fn person(login: &str) -> UserIdentity {
        UserIdentity {
            id: login.to_owned(),
            login: Some(login.to_owned()),
            ..UserIdentity::default()
        }
    }

    fn reviewer(login: &str, vote: ReviewVote) -> Reviewer {
        Reviewer { identity: person(login), vote, is_required: false, is_group: false }
    }

    fn summary(state: PullRequestState, is_draft: bool) -> PullRequestSummary {
        let row = crate::pull_request_list::list_rows::tests::row;
        PullRequestSummary { state, is_draft, author: person("author"), ..row(7) }
    }

    #[test]
    fn a_review_asked_of_the_user_is_the_likeliest_open() {
        let me = person("me");
        let open = summary(PullRequestState::Open, false);
        let asked = [reviewer("me", ReviewVote::None), reviewer("other", ReviewVote::Approved)];
        assert_eq!(Interest::of(&open, &asked, None, Some(&me)), Interest::Waiting);
        let given = [reviewer("me", ReviewVote::Approved)];
        assert_eq!(Interest::of(&open, &given, None, Some(&me)), Interest::Settled);
        let others = [reviewer("other", ReviewVote::ChangesRequested)];
        assert_eq!(Interest::of(&open, &others, None, Some(&me)), Interest::Open);
        let approved = [reviewer("other", ReviewVote::Approved)];
        let pending = Check::new("Approvals", CheckState::Pending);
        assert_eq!(Interest::of(&open, &approved, None, Some(&me)), Interest::Settled);
        assert_eq!(
            Interest::of(&open, &approved, Some(&pending), Some(&me)),
            Interest::Open,
            "required approvals are still missing"
        );
        let mine = [reviewer("me", ReviewVote::None)];
        assert_eq!(Interest::of(&open, &mine, None, None), Interest::Open, "me not known yet");
    }

    #[test]
    fn drafts_merged_and_own_pull_requests() {
        let me = person("me");
        let asked = [reviewer("me", ReviewVote::None)];
        let draft = summary(PullRequestState::Open, true);
        assert_eq!(Interest::of(&draft, &asked, None, Some(&me)), Interest::Settled);
        let merged = summary(PullRequestState::Merged, false);
        assert_eq!(Interest::of(&merged, &asked, None, Some(&me)), Interest::Done);
        let own = PullRequestSummary { author: person("me"), ..draft };
        assert_eq!(Interest::of(&own, &[], None, Some(&me)), Interest::Open);
    }

    #[test]
    fn git_work_only_where_an_open_is_likely() {
        assert_eq!(Interest::Waiting.depth(false), Depth::Full);
        assert_eq!(Interest::Open.depth(true), Depth::Full);
        assert_eq!(Interest::Open.depth(false), Depth::Meta);
        assert_eq!(Interest::Settled.depth(true), Depth::Meta);
        assert_eq!(Interest::Done.depth(true), Depth::Meta);
        assert_eq!(Interest::Done.depth(false), Depth::Nothing);
    }
}
