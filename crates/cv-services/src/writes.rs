//! The user's own writes that succeeded (merge, close, reopen, draft, vote, reviewers, labels,
//! thread status, auto-complete), as an app-wide log, so everything that shows the pull request shows what the
//! write did at once instead of waiting for the host.
//!
//! - A write is logged as on its way when it starts ([`flights`]); lists show its row as pending.
//!   Its task lands it, and [`land`] records the success wherever a view polls the log first, also
//!   when the write's page is hidden or gone.
//! - [`record`] logs a write where it succeeded, with its [`Effect`], and changes the page cache
//!   at once: the pull request's detail, summary and threads, and every remembered list that has
//!   its row. Those answers are marked outdated, so the next visit still asks the host for what the
//!   effect does not model (the merge commit, the timeline, policies).
//! - Views catch up by sequence number ([`Writes::last`]): an open page's session and every list
//!   apply the writes they have not seen yet when they next poll, also after they were hidden.
//! - Hosts lag: a detail or list answered after the write may still show the old state (GitHub's
//!   search index trails its REST API). Until an answer confirms an effect, every newly arrived
//!   detail, thread list and list answer for that pull request gets it applied again
//!   ([`Writes::settle_detail`], [`Writes::settle_threads`], [`Writes::settle_rows`]). A detail or
//!   thread list that shows it retires it for its kind; lists come from several endpoints that lag
//!   apart, so for them only `SETTLE_WINDOW` or a reload by hand retires it.
//! - A later write of the same thing (a second vote, reopening after a close) replaces the earlier
//!   entry. A reload by hand (F5, Refresh) retires the entries it reloads: the user asked for the
//!   host's word.
//!
//! "Me" (the vote's reviewer, "Add me as reviewer") is the signed-in user as
//! [`self_review::Identities`] knows them; a write whose "me" is unknown is not logged and only
//! the refresh after it shows it.

pub mod flights;
#[cfg(test)]
mod tests;

use std::time::Duration;

use review_core::{
    AutoComplete, Label, PullRequestDetail, PullRequestState, PullRequestSummary, ReviewThread,
    ReviewVote, Reviewer, StateFilter, ThreadStatus, UserIdentity,
};

use cv_runtime::cache::page_cache::{self, pull_request_part};
use cv_runtime::route::PullRequestKey;

use crate::{Services, self_review};

/// How long an effect is applied to answers that do not show it yet: our number. GitHub's search
/// listing has been seen to trail a merge by tens of seconds; two minutes covers that with room,
/// and is short enough that a change made on the platform meanwhile shows soon after.
pub(crate) const SETTLE_WINDOW: Duration = Duration::from_secs(120);

/// What a write changes, as the caller knows it when it starts.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    State(PullRequestState),
    Draft(bool),
    /// The signed-in user's vote.
    Vote(ReviewVote),
    ReviewerAdded(Added),
    /// The reviewer with this [`UserIdentity::id`].
    ReviewerRemoved(String),
    /// Exactly these labels.
    Labels(Vec<Label>),
    ThreadStatus {
        thread_id: String,
        status: ThreadStatus,
    },
    /// The auto-complete now set (`None`: cancelled). One without [`AutoComplete::set_by`] was set
    /// by the signed-in user.
    AutoComplete(Option<AutoComplete>),
}

/// Who a reviewer add names.
#[derive(Debug, Clone, PartialEq)]
pub enum Added {
    /// The signed-in user.
    Me,
    /// A login, username or e-mail as typed; the platform resolves it its own way.
    Named(String),
}

/// An answer an effect is applied to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Answer {
    /// Just arrived from the host, asked for at input time `asked_at`: it may confirm effects.
    Host { asked_at: f64 },
    /// Already on screen, catching up with writes logged since: it confirms nothing.
    Shown,
}

/// What applying a change to an answer did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Changed,
    /// The answer shows it already.
    Shown,
    /// The answer has nothing the change touches.
    Unrelated,
}

/// A change with "me" resolved.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    State(PullRequestState),
    Draft(bool),
    Vote { me: UserIdentity, vote: ReviewVote },
    ReviewerAdded { who: UserIdentity, is_typed: bool },
    ReviewerRemoved(String),
    Labels(Vec<Label>),
    ThreadStatus { thread_id: String, status: ThreadStatus },
    AutoComplete(Option<AutoComplete>),
}

/// One write in the log.
#[derive(Debug, Clone)]
struct Written {
    key: PullRequestKey,
    change: Change,
    /// Input time the write succeeded.
    at: f64,
    /// A detail (resp. thread list) confirmed it, or the user reloaded the page by hand.
    is_detail_settled: bool,
    is_threads_settled: bool,
    /// The user reloaded the lists by hand.
    is_lists_settled: bool,
}

#[derive(Default)]
pub struct Writes {
    log: Vec<Written>,
    last: u64,
    /// The writes on their way.
    pub flights: flights::Flights,
}

impl Writes {
    /// The sequence number of the latest write; views that saw it have nothing to catch up.
    pub fn last(&self) -> u64 {
        self.last
    }

    /// Logs `effect` on `key`, succeeded at input time `now`; `me` is the signed-in user where
    /// known. `None` when the effect needs "me" and it is unknown.
    fn record(
        &mut self,
        key: &PullRequestKey,
        effect: Effect,
        me: Option<UserIdentity>,
        now: f64,
    ) -> Option<&Written> {
        self.log.retain(|written| written.is_active(now));
        let change = Change::of(effect, me)?;
        self.log.retain(|written| !(written.key == *key && written.change.replaces(&change)));
        self.last += 1;
        self.log.push(Written {
            key: key.clone(),
            change,
            at: now,
            is_detail_settled: false,
            is_threads_settled: false,
            is_lists_settled: false,
        });
        self.log.last()
    }

    /// Applies the unconfirmed effects on `key` to `detail`; true when it changed. A host answer
    /// that shows an effect confirms it for the detail.
    pub fn settle_detail(
        &mut self,
        key: &PullRequestKey,
        detail: &mut PullRequestDetail,
        answer: Answer,
        now: f64,
    ) -> bool {
        let mut is_changed = false;
        for written in self.log.iter_mut().filter(|written| written.key == *key) {
            if written.is_detail_settled || !written.is_active(now) {
                continue;
            }
            if written.is_taken_as_answered(answer) {
                written.is_detail_settled = true;
                continue;
            }
            match written.change.apply_detail(detail) {
                Outcome::Changed => is_changed = true,
                Outcome::Shown => written.is_detail_settled |= answer != Answer::Shown,
                Outcome::Unrelated => {}
            }
        }
        is_changed
    }

    /// [`Self::settle_detail`] for the thread list of `key`.
    pub fn settle_threads(
        &mut self,
        key: &PullRequestKey,
        threads: &mut [ReviewThread],
        answer: Answer,
        now: f64,
    ) -> bool {
        let mut is_changed = false;
        for written in self.log.iter_mut().filter(|written| written.key == *key) {
            if written.is_threads_settled || !written.is_active(now) {
                continue;
            }
            match written.change.apply_threads(threads) {
                Outcome::Changed => is_changed = true,
                Outcome::Shown => written.is_threads_settled |= answer != Answer::Shown,
                Outcome::Unrelated => {}
            }
        }
        is_changed
    }

    /// Applies the effects to the rows of a list of `connection_id` filtered by `state`, and moves
    /// a row an effect moved out of the filter (a merged one from the open list) to `dropped`; true
    /// when the rows changed.
    pub fn settle_rows(
        &self,
        connection_id: &str,
        state: StateFilter,
        rows: &mut Vec<PullRequestSummary>,
        dropped: &mut Vec<PullRequestSummary>,
        now: f64,
    ) -> bool {
        let active = || {
            self.log.iter().filter(|written| {
                written.is_for_lists(now) && written.key.connection_id == connection_id
            })
        };
        if active().next().is_none() {
            return false;
        }
        let mut is_changed = false;
        rows.retain_mut(|row| {
            let mut is_touched = false;
            for written in active() {
                if written.is_row(row) {
                    is_touched |= written.change.apply_summary(row) == Outcome::Changed;
                }
            }
            is_changed |= is_touched;
            let is_kept = !is_touched || is_listed_in(row.state, state);
            if !is_kept {
                dropped.push(row.clone());
            }
            is_changed |= !is_kept;
            is_kept
        });
        is_changed
    }

    /// Applies the reviewer effects on the pull request `storage_key` names to `reviewers`, a
    /// list row's votes that its checks brought; true when they changed.
    pub fn settle_reviewers(
        &self,
        storage_key: &str,
        reviewers: &mut Vec<Reviewer>,
        now: f64,
    ) -> bool {
        let mut is_changed = false;
        for written in self
            .log
            .iter()
            .filter(|written| written.is_for_lists(now) && written.key.storage_key() == storage_key)
        {
            is_changed |= written.change.apply_reviewers(reviewers) == Outcome::Changed;
        }
        is_changed
    }

    /// The user reloaded the page of `key` by hand: its detail and threads take the host's word.
    pub fn reloaded_by_hand(&mut self, key: &PullRequestKey) {
        for written in self.log.iter_mut().filter(|written| written.key == *key) {
            written.is_detail_settled = true;
            written.is_threads_settled = true;
        }
    }

    /// The user reloaded the lists by hand: their rows take the host's word.
    pub fn lists_reloaded_by_hand(&mut self) {
        for written in &mut self.log {
            written.is_lists_settled = true;
        }
    }

    /// Logs `effect` without touching a page cache, for tests of what reads the log.
    #[cfg(any(test, feature = "test-support"))]
    pub fn logged_for_test(
        &mut self,
        key: &PullRequestKey,
        effect: Effect,
        me: Option<UserIdentity>,
        now: f64,
    ) -> bool {
        self.record(key, effect, me, now).is_some()
    }
}

impl Written {
    fn is_active(&self, now: f64) -> bool {
        now - self.at < SETTLE_WINDOW.as_secs_f64()
    }

    /// Lists still apply it. A typed reviewer name is left out of them: the platform resolves it
    /// its own way, and a row would show the name next to the reviewer it became.
    fn is_for_lists(&self, now: f64) -> bool {
        let is_typed = matches!(self.change, Change::ReviewerAdded { is_typed: true, .. });
        !self.is_lists_settled && !is_typed && self.is_active(now)
    }

    fn is_row(&self, row: &PullRequestSummary) -> bool {
        row.number == self.key.number && row.repository_key == self.key.repository_key
    }

    /// A typed reviewer name resolves on the platform in ways the app cannot tell (an e-mail to a
    /// login), so the first answer asked for after the write is taken as it is.
    fn is_taken_as_answered(&self, answer: Answer) -> bool {
        let is_typed = matches!(self.change, Change::ReviewerAdded { is_typed: true, .. });
        matches!(answer, Answer::Host { asked_at } if is_typed && asked_at >= self.at)
    }
}

impl Change {
    fn of(effect: Effect, me: Option<UserIdentity>) -> Option<Self> {
        let change = match effect {
            Effect::State(state) => Change::State(state),
            Effect::Draft(is_draft) => Change::Draft(is_draft),
            Effect::Vote(vote) => Change::Vote { me: me?, vote },
            Effect::ReviewerAdded(Added::Me) => Change::ReviewerAdded { who: me?, is_typed: false },
            Effect::ReviewerAdded(Added::Named(name)) => Change::ReviewerAdded {
                who: UserIdentity {
                    id: String::new(),
                    display_name: name.clone(),
                    login: Some(name),
                    avatar_url: None,
                },
                is_typed: true,
            },
            Effect::ReviewerRemoved(id) => Change::ReviewerRemoved(id),
            Effect::Labels(labels) => Change::Labels(labels),
            Effect::ThreadStatus { thread_id, status } => {
                Change::ThreadStatus { thread_id, status }
            }
            Effect::AutoComplete(auto_complete) => {
                Change::AutoComplete(auto_complete.map(|auto_complete| AutoComplete {
                    set_by: auto_complete.set_by.or(me),
                    ..auto_complete
                }))
            }
        };
        Some(change)
    }

    /// Whether `newer` sets what this change set, so it takes its place.
    fn replaces(&self, newer: &Change) -> bool {
        let same_person = |a: &UserIdentity, b: &UserIdentity| self_review::is_same_person(a, b);
        match (self, newer) {
            (Change::State(_), Change::State(_))
            | (Change::Draft(_), Change::Draft(_))
            | (Change::Vote { .. }, Change::Vote { .. })
            | (Change::Labels(_), Change::Labels(_))
            | (Change::AutoComplete(_), Change::AutoComplete(_)) => true,
            (Change::ReviewerAdded { who, .. }, Change::ReviewerRemoved(id))
            | (Change::ReviewerRemoved(id), Change::ReviewerAdded { who, .. }) => who.id == *id,
            (Change::ReviewerAdded { who, .. }, Change::ReviewerAdded { who: newer, .. }) => {
                same_person(newer, who)
            }
            (Change::ReviewerRemoved(id), Change::ReviewerRemoved(newer)) => id == newer,
            (
                Change::ThreadStatus { thread_id, .. },
                Change::ThreadStatus { thread_id: newer, .. },
            ) => thread_id == newer,
            _ => false,
        }
    }

    fn apply_summary(&self, summary: &mut PullRequestSummary) -> Outcome {
        match self {
            Change::State(state) => set(&mut summary.state, *state),
            Change::Draft(is_draft) => set(&mut summary.is_draft, *is_draft),
            Change::Labels(labels) => {
                if label_names(&summary.labels) == label_names(labels) {
                    return Outcome::Shown;
                }
                summary.labels.clone_from(labels);
                Outcome::Changed
            }
            Change::Vote { .. } | Change::ReviewerAdded { .. } | Change::ReviewerRemoved(_) => {
                self.apply_reviewers(&mut summary.reviewers)
            }
            Change::ThreadStatus { .. } | Change::AutoComplete(_) => Outcome::Unrelated,
        }
    }

    /// [`Self::apply_summary`] for a detail, which also shows the auto-complete. An auto-complete
    /// the host shows counts as shown whoever it names: the host knows who set it.
    fn apply_detail(&self, detail: &mut PullRequestDetail) -> Outcome {
        let Change::AutoComplete(auto_complete) = self else {
            return self.apply_summary(&mut detail.summary);
        };
        let is_shown = match (&detail.auto_complete, auto_complete) {
            (Some(shown), Some(written)) => shown.strategy == written.strategy,
            (shown, written) => shown.is_none() && written.is_none(),
        };
        if is_shown {
            return Outcome::Shown;
        }
        detail.auto_complete.clone_from(auto_complete);
        Outcome::Changed
    }

    fn apply_reviewers(&self, reviewers: &mut Vec<Reviewer>) -> Outcome {
        let position = |who: &UserIdentity, reviewers: &[Reviewer]| {
            reviewers.iter().position(|reviewer| {
                !reviewer.is_group && self_review::is_same_person(&reviewer.identity, who)
            })
        };
        match self {
            Change::Vote { me, vote } => match position(me, reviewers) {
                Some(index) => set(&mut reviewers[index].vote, *vote),
                // Voting makes the user a reviewer; a reset of a vote never cast changes nothing.
                None if *vote == ReviewVote::None => Outcome::Shown,
                None => {
                    reviewers.push(reviewer(me, *vote));
                    Outcome::Changed
                }
            },
            Change::ReviewerAdded { who, .. } => {
                if position(who, reviewers).is_some() {
                    return Outcome::Shown;
                }
                reviewers.push(reviewer(who, ReviewVote::None));
                Outcome::Changed
            }
            Change::ReviewerRemoved(id) => {
                let count = reviewers.len();
                reviewers.retain(|reviewer| reviewer.identity.id != *id);
                if reviewers.len() == count { Outcome::Shown } else { Outcome::Changed }
            }
            Change::State(_)
            | Change::Draft(_)
            | Change::Labels(_)
            | Change::ThreadStatus { .. }
            | Change::AutoComplete(_) => Outcome::Unrelated,
        }
    }

    fn apply_threads(&self, threads: &mut [ReviewThread]) -> Outcome {
        let Change::ThreadStatus { thread_id, status } = self else {
            return Outcome::Unrelated;
        };
        threads
            .iter_mut()
            .find(|thread| thread.id == *thread_id)
            .map_or(Outcome::Unrelated, |thread| set(&mut thread.status, *status))
    }
}

fn set<T: PartialEq>(field: &mut T, value: T) -> Outcome {
    if *field == value {
        return Outcome::Shown;
    }
    *field = value;
    Outcome::Changed
}

/// The names of `labels`, sorted: the platforms keep labels in an order of their own.
fn label_names(labels: &[Label]) -> Vec<&str> {
    let mut names: Vec<&str> = labels.iter().map(|label| label.name.as_str()).collect();
    names.sort_unstable();
    names
}

fn reviewer(identity: &UserIdentity, vote: ReviewVote) -> Reviewer {
    Reviewer { identity: identity.clone(), vote, is_required: false, is_group: false }
}

/// Whether a list filtered by `filter` lists a pull request in `state`.
pub fn is_listed_in(state: PullRequestState, filter: StateFilter) -> bool {
    match filter {
        StateFilter::All => true,
        StateFilter::Open => state == PullRequestState::Open,
        StateFilter::Merged => state == PullRequestState::Merged,
        StateFilter::Closed => state == PullRequestState::Closed,
    }
}

/// Records the writes that landed successfully since the last call ([`flights`]), with "me" as
/// [`self_review::Identities`] knows them; true when one did. Every view that shows pull requests
/// calls it before it polls its answers.
pub fn land(services: &mut Services) -> bool {
    let landed = services.writes.flights.take_landed();
    let is_landed = !landed.is_empty();
    for (key, effect) in landed {
        let me = services.identities.known(&key.connection_id);
        record(services, &key, effect, me);
    }
    is_landed
}

/// Logs a write of the user's on `key` that just succeeded, and changes what the page cache
/// remembers of the pull request: its detail, summary and threads, and the rows of every list of
/// its connection. All of them, and every other list of the connection, count as outdated from
/// now on. False when the write was not logged ("me" unknown).
pub fn record(
    services: &mut Services,
    key: &PullRequestKey,
    effect: Effect,
    me: Option<UserIdentity>,
) -> bool {
    let now = services.now();
    let Some(written) = services.writes.record(key, effect, me, now) else {
        return false;
    };
    let cache = &mut services.cache;
    let storage_key = key.storage_key();
    let change = &written.change;
    cache.update(
        &pull_request_part(&storage_key, "detail"),
        now,
        |detail: &mut PullRequestDetail| change.apply_detail(detail) == Outcome::Changed,
    );
    cache.update(&pull_request_part(&storage_key, "summary"), now, |summary| {
        change.apply_summary(summary) == Outcome::Changed
    });
    cache.update(
        &pull_request_part(&storage_key, "threads"),
        now,
        |threads: &mut Vec<ReviewThread>| change.apply_threads(threads) == Outcome::Changed,
    );
    cache.mark_outdated(&pull_request_part(&storage_key, ""));
    let lists = page_cache::lists_of(&key.connection_id);
    for list in cache.keys_with_prefix(&lists) {
        let Some(state) = page_cache::list_state(&list) else {
            continue;
        };
        cache.update(&list, now, |rows: &mut Vec<PullRequestSummary>| {
            let mut is_changed = false;
            rows.retain_mut(|row| {
                let is_touched = written.is_for_lists(now)
                    && written.is_row(row)
                    && change.apply_summary(row) == Outcome::Changed;
                let is_kept = !is_touched || is_listed_in(row.state, state);
                is_changed |= is_touched;
                is_kept
            });
            is_changed
        });
    }
    cache.mark_outdated(&lists);
    true
}
