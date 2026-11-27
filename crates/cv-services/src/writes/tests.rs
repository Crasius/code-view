use cv_runtime::cache::page_cache::{list_part, pull_request_part};
use cv_runtime::route::PullRequestKey;
use cv_testkit::{ScratchDir, samples};
use review_core::{
    AutoComplete, Label, MergeStrategy, PullRequestDetail, PullRequestScope, PullRequestState,
    PullRequestSummary, ReviewThread, ReviewVote, Reviewer, StateFilter, ThreadStatus,
    UserIdentity,
};

use super::{Added, Answer, Effect, SETTLE_WINDOW, Writes, record};
use crate::{Services, person};

const CONNECTION: &str = "gh";

fn key(number: u64) -> PullRequestKey {
    PullRequestKey { connection_id: CONNECTION.into(), repository_key: "acme/app".into(), number }
}

fn me() -> UserIdentity {
    person("1", "ada")
}

fn reviewer(identity: UserIdentity, vote: ReviewVote) -> Reviewer {
    Reviewer { identity, vote, is_required: false, is_group: false }
}

fn label(name: &str) -> Label {
    Label { name: name.into(), color: None, description: None }
}

fn row(number: u64) -> PullRequestSummary {
    PullRequestSummary {
        author: person("2", "ana"),
        labels: vec![label("bug")],
        reviewers: vec![reviewer(person("3", "bea"), ReviewVote::Approved)],
        ..samples::summary("acme/app", number)
    }
}

fn detail(number: u64) -> PullRequestDetail {
    PullRequestDetail { summary: row(number), ..samples::detail("acme/app", number) }
}

fn thread(id: &str) -> ReviewThread {
    samples::thread(id, ThreadStatus::Active, None, Vec::new())
}

/// A host answer asked for at `asked_at`.
fn host(asked_at: f64) -> Answer {
    Answer::Host { asked_at }
}

fn logged(writes: &mut Writes, number: u64, effect: Effect, at: f64) {
    assert!(writes.logged_for_test(&key(number), effect, Some(me()), at), "logged");
}

/// The open list, the list of every state and the merged list of the connection, and one of
/// another connection, as a page cache key each.
fn lists() -> [String; 4] {
    let list = |stamp: &str, state| list_part(stamp, "*", PullRequestScope::Visible, state);
    [
        list("gh|00", StateFilter::Open),
        list("gh|00", StateFilter::All),
        list("gh|00", StateFilter::Merged),
        list("gl|00", StateFilter::Open),
    ]
}

#[test]
fn a_merge_changes_the_remembered_detail_summary_and_lists_at_once() {
    let dir = ScratchDir::new("writes-merge-cache");
    let paths = dir.app_paths();
    let mut services =
        Services::new(egui::Context::default(), paths, cv_providers::create_provider);
    let storage_key = key(7).storage_key();
    let part = |name: &str| pull_request_part(&storage_key, name);
    services.cache.put(&part("detail"), detail(7), 0.0);
    services.cache.put(&part("summary"), row(7), 0.0);
    services.cache.put(&part("threads"), vec![thread("t1")], 0.0);
    let [open, all, merged, other] = lists();
    for list in [&open, &all, &merged, &other] {
        services.cache.put(list, vec![row(7), row(8)], 0.0);
    }

    assert!(record(&mut services, &key(7), Effect::State(PullRequestState::Merged), None));

    let cache = &mut services.cache;
    let state = |rows: Vec<PullRequestSummary>| -> Vec<(u64, PullRequestState)> {
        rows.into_iter().map(|row| (row.number, row.state)).collect()
    };
    let shown: PullRequestDetail = cache.get(&part("detail")).unwrap();
    assert_eq!(shown.summary.state, PullRequestState::Merged);
    let summary: PullRequestSummary = cache.get(&part("summary")).unwrap();
    assert_eq!(summary.state, PullRequestState::Merged);
    assert!(cache.is_stale(&part("detail"), 0.0), "a reopened page asks the host again");
    assert_eq!(state(cache.get(&open).unwrap()), [(8, PullRequestState::Open)], "dropped");
    assert_eq!(
        state(cache.get(&all).unwrap()),
        [(7, PullRequestState::Merged), (8, PullRequestState::Open)]
    );
    assert!(
        [&open, &all, &merged].iter().all(|list| cache.is_stale(list, 0.0)),
        "every list of the connection asks again on its next visit"
    );
    assert_eq!(state(cache.get(&other).unwrap()).len(), 2, "another connection's list stays");
    assert!(!cache.is_stale(&other, 0.0));

    // A detail asked for before the merge cannot show it and stays out.
    cache.put(&part("detail"), detail(7), -1.0);
    let shown: PullRequestDetail = cache.get(&part("detail")).unwrap();
    assert_eq!(shown.summary.state, PullRequestState::Merged);
}

#[test]
fn a_detail_that_lags_gets_the_merge_until_one_confirms_it() {
    let mut writes = Writes::default();
    logged(&mut writes, 7, Effect::State(PullRequestState::Merged), 10.0);

    let mut stale = detail(7);
    assert!(writes.settle_detail(&key(7), &mut stale, host(11.0), 11.0));
    assert_eq!(stale.summary.state, PullRequestState::Merged, "the host has not caught up");
    let mut other = detail(8);
    assert!(!writes.settle_detail(&key(8), &mut other, host(11.0), 11.0), "another pull request");

    let mut confirmed = detail(7);
    confirmed.summary.state = PullRequestState::Merged;
    assert!(!writes.settle_detail(&key(7), &mut confirmed, host(12.0), 12.0));
    let mut later = detail(7);
    assert!(!writes.settle_detail(&key(7), &mut later, host(13.0), 13.0), "retired once confirmed");
    assert_eq!(later.summary.state, PullRequestState::Open);
}

#[test]
fn lists_drop_a_merged_row_until_the_window_ends_even_after_the_detail_confirmed() {
    let mut writes = Writes::default();
    logged(&mut writes, 7, Effect::State(PullRequestState::Merged), 10.0);
    let mut confirmed = detail(7);
    confirmed.summary.state = PullRequestState::Merged;
    writes.settle_detail(&key(7), &mut confirmed, host(11.0), 11.0);

    let numbers =
        |rows: &[PullRequestSummary]| rows.iter().map(|row| row.number).collect::<Vec<_>>();
    let (mut open, mut dropped) = (vec![row(7), row(8)], Vec::new());
    assert!(writes.settle_rows(CONNECTION, StateFilter::Open, &mut open, &mut dropped, 12.0));
    assert_eq!(numbers(&open), [8], "the search index still lists it open");
    assert_eq!(numbers(&dropped), [7], "the list lets it leave");
    let mut all = vec![row(7)];
    assert!(writes.settle_rows(CONNECTION, StateFilter::All, &mut all, &mut Vec::new(), 12.0));
    assert_eq!(all[0].state, PullRequestState::Merged);
    let mut elsewhere = vec![row(7)];
    assert!(!writes.settle_rows("gl", StateFilter::Open, &mut elsewhere, &mut Vec::new(), 12.0));

    let end = 10.0 + SETTLE_WINDOW.as_secs_f64();
    let mut late = vec![row(7)];
    assert!(
        !writes.settle_rows(CONNECTION, StateFilter::Open, &mut late, &mut Vec::new(), end),
        "window over"
    );

    let mut writes = Writes::default();
    logged(&mut writes, 7, Effect::State(PullRequestState::Merged), 10.0);
    writes.lists_reloaded_by_hand();
    let mut reloaded = vec![row(7)];
    assert!(!writes.settle_rows(
        CONNECTION,
        StateFilter::Open,
        &mut reloaded,
        &mut Vec::new(),
        12.0
    ));
    assert_eq!(numbers(&reloaded), [7], "Refresh takes the host's word");
}

#[test]
fn a_reload_by_hand_takes_the_hosts_word_for_the_page() {
    let mut writes = Writes::default();
    logged(&mut writes, 7, Effect::Draft(true), 10.0);
    writes.reloaded_by_hand(&key(7));
    let mut answer = detail(7);
    assert!(!writes.settle_detail(&key(7), &mut answer, host(11.0), 11.0));
    assert!(!answer.summary.is_draft);
}

#[test]
fn a_later_write_of_the_same_thing_replaces_the_earlier_one() {
    let mut writes = Writes::default();
    logged(&mut writes, 7, Effect::State(PullRequestState::Closed), 10.0);
    logged(&mut writes, 7, Effect::State(PullRequestState::Open), 11.0);
    logged(&mut writes, 7, Effect::Draft(true), 12.0);
    let mut answer = detail(7);
    answer.summary.state = PullRequestState::Closed;
    writes.settle_detail(&key(7), &mut answer, host(13.0), 13.0);
    assert_eq!(answer.summary.state, PullRequestState::Open, "reopened after the close");
    assert!(answer.summary.is_draft, "a draft is no state: both apply");
}

#[test]
fn my_vote_shows_as_my_reviewer_entry_and_needs_to_know_me() {
    let mut writes = Writes::default();
    let vote = Effect::Vote(ReviewVote::Approved);
    assert!(!writes.logged_for_test(&key(7), vote.clone(), None, 10.0), "who is me is unknown");
    logged(&mut writes, 7, vote, 10.0);

    let mut answer = detail(7);
    assert!(writes.settle_detail(&key(7), &mut answer, host(11.0), 11.0));
    let mine: Vec<(&str, ReviewVote)> = answer
        .summary
        .reviewers
        .iter()
        .map(|reviewer| (reviewer.identity.id.as_str(), reviewer.vote))
        .collect();
    assert_eq!(mine, [("3", ReviewVote::Approved), ("1", ReviewVote::Approved)], "voting adds me");

    logged(&mut writes, 7, Effect::Vote(ReviewVote::ChangesRequested), 12.0);
    let mut listed = vec![row(7)];
    listed[0].reviewers.push(reviewer(person("other-org", "ADA"), ReviewVote::Approved));
    assert!(writes.settle_rows(CONNECTION, StateFilter::Open, &mut listed, &mut Vec::new(), 13.0));
    assert_eq!(listed[0].reviewers.len(), 2, "the same person by login, not a second entry");
    assert_eq!(listed[0].reviewers[1].vote, ReviewVote::ChangesRequested, "the newer vote");

    logged(&mut writes, 8, Effect::Vote(ReviewVote::None), 14.0);
    let mut unvoted = detail(8);
    assert!(!writes.settle_detail(&key(8), &mut unvoted, host(15.0), 15.0), "nothing to reset");
}

#[test]
fn labels_apply_as_a_set_and_confirm_in_any_order() {
    let mut writes = Writes::default();
    logged(&mut writes, 7, Effect::Labels(vec![label("ui"), label("bug")]), 10.0);
    let mut stale = detail(7);
    assert!(writes.settle_detail(&key(7), &mut stale, host(11.0), 11.0));
    let names: Vec<&str> = stale.summary.labels.iter().map(|label| label.name.as_str()).collect();
    assert_eq!(names, ["ui", "bug"]);

    let mut confirmed = detail(7);
    confirmed.summary.labels = vec![label("bug"), label("ui")];
    assert!(!writes.settle_detail(&key(7), &mut confirmed, host(12.0), 12.0), "the host's order");
    let mut later = detail(7);
    assert!(!writes.settle_detail(&key(7), &mut later, host(13.0), 13.0), "retired");
}

#[test]
fn a_removed_reviewer_stays_removed_from_detail_and_rows() {
    let mut writes = Writes::default();
    logged(&mut writes, 7, Effect::ReviewerRemoved("3".into()), 10.0);
    let mut stale = detail(7);
    assert!(writes.settle_detail(&key(7), &mut stale, host(11.0), 11.0));
    assert_eq!(stale.summary.reviewers, [] as [Reviewer; 0]);
    let mut rows = vec![row(7)];
    assert!(writes.settle_rows(CONNECTION, StateFilter::Open, &mut rows, &mut Vec::new(), 11.0));
    assert!(rows[0].reviewers.is_empty(), "the row is kept, without the reviewer");
    let mut votes = vec![reviewer(person("3", "bea"), ReviewVote::Approved)];
    assert!(writes.settle_reviewers(&key(7).storage_key(), &mut votes, 11.0));
    assert!(votes.is_empty(), "the votes a row's checks brought too");
}

#[test]
fn a_thread_status_applies_to_threads_only() {
    let mut writes = Writes::default();
    let status = ThreadStatus::Resolved;
    logged(&mut writes, 7, Effect::ThreadStatus { thread_id: "t1".into(), status }, 10.0);
    let mut stale = vec![thread("t1"), thread("t2")];
    assert!(writes.settle_threads(&key(7), &mut stale, host(11.0), 11.0));
    let statuses: Vec<ThreadStatus> = stale.iter().map(|thread| thread.status).collect();
    assert_eq!(statuses, [ThreadStatus::Resolved, ThreadStatus::Active]);
    let mut answer = detail(7);
    assert!(!writes.settle_detail(&key(7), &mut answer, host(11.0), 11.0));

    // What is on screen catches up without confirming: the next stale answer still gets it.
    let mut shown = vec![thread("t1")];
    assert!(writes.settle_threads(&key(7), &mut shown, Answer::Shown, 11.0));
    assert!(writes.settle_threads(&key(7), &mut [thread("t1")], host(12.0), 12.0));
}

#[test]
fn a_typed_reviewer_shows_until_the_first_detail_asked_after_the_add() {
    let mut writes = Writes::default();
    logged(&mut writes, 7, Effect::ReviewerAdded(Added::Named("cy@example.com".into())), 10.0);
    let mut early = detail(7);
    assert!(writes.settle_detail(&key(7), &mut early, host(9.0), 11.0), "asked before the add");
    assert_eq!(early.summary.reviewers.len(), 2);

    let mut asked_after = detail(7);
    assert!(!writes.settle_detail(&key(7), &mut asked_after, host(10.5), 11.0));
    assert_eq!(asked_after.summary.reviewers.len(), 1, "the platform's own resolution");
    let mut rows = vec![row(7)];
    assert!(
        !writes.settle_rows(CONNECTION, StateFilter::Open, &mut rows, &mut Vec::new(), 11.0),
        "rows skip it"
    );
}

/// Auto-complete shows on the detail at once, named after the signed-in user, until a detail
/// shows it; a cancel takes it away again.
#[test]
fn an_auto_complete_shows_on_the_detail_as_mine_until_the_host_shows_it() {
    let mut writes = Writes::default();
    let squash = AutoComplete { set_by: None, strategy: Some(MergeStrategy::Squash) };
    logged(&mut writes, 7, Effect::AutoComplete(Some(squash)), 10.0);
    let mut stale = detail(7);
    assert!(writes.settle_detail(&key(7), &mut stale, host(11.0), 11.0));
    let shown = stale.auto_complete.expect("set at once");
    assert_eq!((shown.set_by, shown.strategy), (Some(me()), Some(MergeStrategy::Squash)));

    let mut confirmed = detail(7);
    confirmed.auto_complete = Some(AutoComplete {
        set_by: Some(person("9", "host")),
        strategy: Some(MergeStrategy::Squash),
    });
    assert!(!writes.settle_detail(&key(7), &mut confirmed, host(12.0), 12.0));
    assert!(!writes.settle_detail(&key(7), &mut detail(7), host(13.0), 13.0), "retired");

    logged(&mut writes, 7, Effect::AutoComplete(None), 14.0);
    let mut lagging = confirmed;
    assert!(writes.settle_detail(&key(7), &mut lagging, host(15.0), 15.0));
    assert_eq!(lagging.auto_complete, None, "cancelled");
}
