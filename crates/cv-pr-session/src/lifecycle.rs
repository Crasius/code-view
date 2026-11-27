//! The pull request's lifecycle writes: votes, draft and state changes, reviewer adds
//! ([`LifecycleAction`]), and why a platform refuses the author's own vote ([`author_refusal`]).
//!
//! The page's header, its vote button, the Overview's reviewer card and the Files tab's Approve
//! all start these writes, and the action runner keeps one to retry it. They live in
//! `cv-pr-session`, beside [`crate::actions`], so the Files tab and the runner reach them without
//! depending on the page.

use cv_services::Services;
use cv_services::self_review::{self, Standing};
use cv_services::writes::{Added, Effect};
use review_core::{ProviderKind, PullRequestState, ReviewVote};

use crate::actions::{ActionRunner, Refresh, Ticket, Write};
use crate::session::PullRequestSession;

/// Why the votes are off on the author's own pull request on GitHub, and what still works.
const AUTHOR_CANNOT_VOTE: &str = "GitHub does not let authors approve their own pull request or \
     request changes on it. Comments and replies still work.";

/// Why the platform refuses any vote of a user standing as `standing`: GitHub, for the author.
pub fn author_refusal(kind: ProviderKind, standing: Standing) -> Option<&'static str> {
    (kind == ProviderKind::GitHub && standing == Standing::Author).then_some(AUTHOR_CANNOT_VOTE)
}

/// Writes the header (and the reviewer card) can trigger directly.
#[derive(Clone)]
pub enum LifecycleAction {
    /// `comment` becomes the review's body on GitHub, a general comment elsewhere.
    Vote {
        vote: ReviewVote,
        comment: Option<String>,
    },
    SetDraft(bool),
    Close,
    Reopen,
    AddReviewer(NewReviewer),
}

/// Who a reviewer add names.
#[derive(Clone)]
pub enum NewReviewer {
    /// A login, username or e-mail as typed in the reviewer card.
    Named(String),
    /// The signed-in user ("Add me as reviewer"), see [`self_review::add_me`].
    Me,
}

impl LifecycleAction {
    /// The write's name in "`<name>` failed: `<reason>`".
    pub(crate) fn name(&self) -> &'static str {
        match self {
            LifecycleAction::Vote { vote: ReviewVote::None, .. } => "Vote reset",
            LifecycleAction::Vote { .. } => "Vote",
            LifecycleAction::SetDraft(true) => "Mark as draft",
            LifecycleAction::SetDraft(false) => "Publish",
            LifecycleAction::Close => "Close",
            LifecycleAction::Reopen => "Reopen",
            LifecycleAction::AddReviewer(_) => "Adding the reviewer",
        }
    }

    /// Starts the write; `None` when another one still runs. A failure offers Retry.
    pub fn start(
        self,
        session: &PullRequestSession,
        services: &Services,
        actions: &mut ActionRunner,
    ) -> Option<Ticket> {
        let retry = Write::Lifecycle(self.clone());
        let ticket = self.start_once(session, services, actions);
        if ticket.is_some() {
            actions.remember(retry);
        }
        ticket
    }

    fn start_once(
        self,
        session: &PullRequestSession,
        services: &Services,
        actions: &mut ActionRunner,
    ) -> Option<Ticket> {
        let provider = session.provider.clone();
        let repository = session.key.repository_key.clone();
        let number = session.key.number;
        let refresh = Refresh::PullRequest;
        let key = &session.key;
        match self {
            LifecycleAction::Vote { vote, comment } => {
                let text = match vote {
                    ReviewVote::None => "Vote reset.",
                    _ => "Vote submitted.",
                };
                actions.run(services, text, refresh, (key, Effect::Vote(vote)), async move {
                    provider.set_vote(&repository, number, vote, comment.as_deref()).await
                })
            }
            LifecycleAction::SetDraft(is_draft) => {
                let text = if is_draft { "Marked as draft." } else { "Published." };
                let effect = (key, Effect::Draft(is_draft));
                actions.run(services, text, refresh, effect, async move {
                    provider.set_draft(&repository, number, is_draft).await
                })
            }
            LifecycleAction::Close => {
                let effect = (key, Effect::State(PullRequestState::Closed));
                actions.run(services, "Pull request closed.", refresh, effect, async move {
                    provider.close(&repository, number).await
                })
            }
            LifecycleAction::Reopen => {
                let effect = (key, Effect::State(PullRequestState::Open));
                actions.run(services, "Pull request reopened.", refresh, effect, async move {
                    provider.reopen(&repository, number).await
                })
            }
            LifecycleAction::AddReviewer(NewReviewer::Named(user)) => {
                let effect = (key, Effect::ReviewerAdded(Added::Named(user.clone())));
                actions.run(services, "Reviewer added.", refresh, effect, async move {
                    provider.add_reviewer(&repository, number, &user).await
                })
            }
            LifecycleAction::AddReviewer(NewReviewer::Me) => {
                let ctx = services.background.ctx();
                let connection_id = session.key.connection_id.clone();
                let future = self_review::add_me(
                    provider,
                    services.identities.clone(),
                    connection_id,
                    repository,
                    number,
                );
                let effect = (key, Effect::ReviewerAdded(Added::Me));
                let ticket = actions.run(services, self_review::DONE, refresh, effect, async {
                    future.await.map(|_| ())
                });
                if let Some(ticket) = ticket {
                    ctx.data_mut(|data| data.insert_temp(self_review_id(session), ticket));
                }
                ticket
            }
        }
    }
}

/// Where the ticket of this pull request's "Add me as reviewer" is kept, for the reviewer card.
pub fn self_review_id(session: &PullRequestSession) -> egui::Id {
    egui::Id::unique(("self_review", &session.key))
}
