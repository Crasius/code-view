//! The user's writes on their way: logged when they start, landed when the host answered.
//!
//! A write runs on the background runtime and ends there whether or not its page is still shown
//! (the page may be hidden behind the overview, or closed). Its outcome therefore lands here, in a
//! slot the task itself fills before it hands its result to the page, and not in the page: any view
//! that polls the log ([`super::land`]) records a success, so the overview shows a merge done on a
//! page the user left at once. Until then the views show the write as pending
//! ([`Flights::pending_on`]).

use std::sync::{Arc, Mutex, PoisonError};

use super::Effect;
use cv_runtime::route::PullRequestKey;

/// The writes on their way and those landed since the last [`Flights::take_landed`].
#[derive(Default)]
pub struct Flights {
    shared: Arc<Mutex<Board>>,
    /// The writes on their way as of the last [`Flights::take_landed`], which every view showing
    /// them calls first in its frame; read without the lock.
    on_the_way: Vec<OnTheWay>,
}

/// What the tasks and the UI thread share.
#[derive(Default)]
struct Board {
    next_id: u64,
    flights: Vec<Flight>,
}

struct Flight {
    id: u64,
    key: PullRequestKey,
    effect: Effect,
    /// `Some(true)` once the host took the write, `Some(false)` once it failed.
    landed: Option<bool>,
}

/// A write on its way, as a list row shows it.
struct OnTheWay {
    key: PullRequestKey,
    doing: &'static str,
}

/// One write on its way, held by its task; [`Self::land`] says how it ended. Dropped without that
/// (the task was cancelled or panicked), it counts as failed, so no row stays pending for ever.
pub struct Ticket {
    shared: Arc<Mutex<Board>>,
    id: u64,
    is_landed: bool,
}

impl Flights {
    /// Logs a write of `effect` on `key` that starts now; its task lands it.
    pub fn start(&self, key: &PullRequestKey, effect: &Effect) -> Ticket {
        let mut board = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        board.next_id += 1;
        let id = board.next_id;
        board.flights.push(Flight { id, key: key.clone(), effect: effect.clone(), landed: None });
        drop(board);
        Ticket { shared: self.shared.clone(), id, is_landed: false }
    }

    /// The writes that succeeded since the last call, in the order they started; failed ones leave
    /// the log quietly (their page's toast says why). Refreshes what [`Self::pending_on`] reads.
    pub(crate) fn take_landed(&mut self) -> Vec<(PullRequestKey, Effect)> {
        let mut board = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        let (landed, on_the_way): (Vec<Flight>, Vec<Flight>) = std::mem::take(&mut board.flights)
            .into_iter()
            .partition(|flight| flight.landed.is_some());
        board.flights = on_the_way;
        let succeeded = landed
            .into_iter()
            .filter(|flight| flight.landed == Some(true))
            .map(|flight| (flight.key, flight.effect))
            .collect();
        self.on_the_way = board
            .flights
            .iter()
            .map(|flight| OnTheWay { key: flight.key.clone(), doing: doing(&flight.effect) })
            .collect();
        succeeded
    }

    /// What the latest write on its way to the pull request `number` of `repository_key` on
    /// `connection_id` does, in words ("Merging…").
    pub fn pending_on(
        &self,
        connection_id: &str,
        repository_key: &str,
        number: u64,
    ) -> Option<&'static str> {
        self.on_the_way
            .iter()
            .rev()
            .find(|flight| {
                flight.key.number == number
                    && flight.key.repository_key == repository_key
                    && flight.key.connection_id == connection_id
            })
            .map(|flight| flight.doing)
    }
}

impl Ticket {
    /// The host answered: `is_ok` when it took the write.
    pub fn land(mut self, is_ok: bool) {
        self.mark(is_ok);
    }

    fn mark(&mut self, is_ok: bool) {
        self.is_landed = true;
        let mut board = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(flight) = board.flights.iter_mut().find(|flight| flight.id == self.id) {
            flight.landed = Some(is_ok);
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.is_landed {
            self.mark(false);
        }
    }
}

/// The write in progress, in words, as a pending row names it.
fn doing(effect: &Effect) -> &'static str {
    use review_core::{PullRequestState, ReviewVote};
    match effect {
        Effect::State(PullRequestState::Merged) => "Merging…",
        Effect::State(PullRequestState::Closed) => "Closing…",
        Effect::State(PullRequestState::Open) => "Reopening…",
        Effect::Draft(true) => "Drafting…",
        Effect::Draft(false) => "Publishing…",
        Effect::Vote(ReviewVote::None) => "Resetting vote…",
        Effect::Vote(_) => "Voting…",
        Effect::ReviewerAdded(_) => "Adding reviewer…",
        Effect::ReviewerRemoved(_) => "Removing reviewer…",
        Effect::Labels(_) => "Saving labels…",
        Effect::ThreadStatus { .. } => "Saving thread…",
        Effect::AutoComplete(Some(_)) => "Setting auto-complete…",
        Effect::AutoComplete(None) => "Cancelling auto-complete…",
    }
}

#[cfg(test)]
mod tests {
    use review_core::{PullRequestState, ReviewVote};

    use cv_runtime::route::PullRequestKey;

    use super::Flights;
    use crate::writes::Effect;

    fn key(number: u64) -> PullRequestKey {
        PullRequestKey { connection_id: "gh".into(), repository_key: "acme/app".into(), number }
    }

    #[test]
    fn a_write_is_pending_until_it_lands_and_only_a_success_is_handed_on() {
        let mut flights = Flights::default();
        let merge = flights.start(&key(7), &Effect::State(PullRequestState::Merged));
        let vote = flights.start(&key(8), &Effect::Vote(ReviewVote::Approved));
        assert_eq!(flights.take_landed(), []);
        assert_eq!(flights.pending_on("gh", "acme/app", 7), Some("Merging…"), "from its start");
        assert_eq!(flights.pending_on("gh", "acme/app", 9), None);
        assert_eq!(flights.pending_on("gl", "acme/app", 7), None, "another connection");
        merge.land(true);
        vote.land(false);
        assert_eq!(flights.pending_on("gh", "acme/app", 7), Some("Merging…"), "until taken");
        let landed = flights.take_landed();
        assert_eq!(landed, [(key(7), Effect::State(PullRequestState::Merged))]);
        assert_eq!(flights.pending_on("gh", "acme/app", 7), None);
        assert_eq!(flights.pending_on("gh", "acme/app", 8), None, "a failure returns to normal");
        assert_eq!(flights.take_landed(), [], "each landing is handed on once");
    }

    #[test]
    fn a_task_that_ends_without_an_answer_counts_as_failed() {
        let mut flights = Flights::default();
        let cancelled = flights.start(&key(7), &Effect::Draft(true));
        drop(cancelled);
        assert_eq!(flights.take_landed(), []);
        assert_eq!(flights.pending_on("gh", "acme/app", 7), None);
    }
}
