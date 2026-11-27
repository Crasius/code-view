//! Checks, branch policies and votes of the list rows: what their checks and approvals indicators
//! show ([`review_core::Provider::row_statuses`]).
//!
//! Asked for the rows on screen only, while the window is active, at most [`MAX_RUNNING`] requests
//! at a time, and kept per pull request:
//! - GitHub: search rows came with theirs, so the provider answers from the listing without a
//!   request; rows of a repository's REST listing are asked together, one GraphQL request per
//!   repository (with a token; GraphQL is closed without one).
//! - GitLab: one GraphQL request for the rows on screen of a project (with a token: without one
//!   GitLab allows few GraphQL requests an hour, which the line counts already spend).
//! - Azure DevOps: one request per pull request for its policy evaluations (with a token: policies
//!   need sign-in even on public projects).
//!
//! An answer stays until the row's listing is answered again (Refresh, a revisit): then a row asks
//! again only when the listing shows it changed (head, update time, state, votes), its checks were
//! still running or the last attempt failed. So a running check shows its outcome with the list's
//! next refresh, and settled rows cost nothing. The previous answer stays on screen meanwhile.
//!
//! A vote does not always change the listing: GitHub's REST listing and GitLab's list name only
//! who is asked to review, and an approval need not move the update time the listing shows
//! (GitLab's, say). So a refresh of the page (the scheduler's beat, a return to the window, a warm)
//! renews the answers ([`RowStatuses::renew`]): rows of those platforms ask again once their
//! listing answered after it. Azure DevOps lists the votes, so its rows keep the rule above.
//!
//! A request the host's rate-limit pause holds back ([`RowStatuses::is_held`]) waits at the
//! budget until the pause ends and stays running meanwhile; the row shows it as waiting.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use cv_base::platform::is_window_active;
use cv_runtime::load::Load;
use cv_services::Services;
use cv_services::writes::Writes;
use review_core::http::Api;
use review_core::{
    Check, CheckState, ProviderKind, PullRequestSummary, Reviewer, RowStatus, RunStatus,
};

use super::pauses::Pauses;

/// Requests in flight at once; further rows wait their turn.
const MAX_RUNNING: usize = 4;

/// A list row, as far as its checks and votes go.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StatusRow {
    pub connection_id: String,
    pub kind: ProviderKind,
    pub has_token: bool,
    pub repository_key: String,
    pub number: u64,
    /// The pull request's storage key: answers are kept by it.
    pub storage_key: String,
    /// When the listing that shows the row last answered ([`cv_services::Services::now`]).
    pub listed_at: f64,
    /// What the listing tells about the pull request: head, update time, state and votes.
    pub fingerprint: u64,
}

impl StatusRow {
    pub(crate) fn new(
        connection_id: &str,
        kind: ProviderKind,
        has_token: bool,
        storage_key: String,
        listed_at: f64,
        summary: &PullRequestSummary,
    ) -> Self {
        Self {
            connection_id: connection_id.to_owned(),
            kind,
            has_token,
            repository_key: summary.repository_key.clone(),
            number: summary.number,
            storage_key,
            listed_at,
            fingerprint: fingerprint(summary),
        }
    }
}

/// What the listing tells about a pull request, as one number: two rows with the same one show
/// the same head, update time, state, draft flag and votes.
pub(super) fn fingerprint(summary: &PullRequestSummary) -> u64 {
    let mut hasher = DefaultHasher::new();
    summary.head_sha.hash(&mut hasher);
    summary.updated_at.hash(&mut hasher);
    summary.state.hash(&mut hasher);
    summary.is_draft.hash(&mut hasher);
    for reviewer in &summary.reviewers {
        reviewer.identity.id.hash(&mut hasher);
        reviewer.vote.hash(&mut hasher);
    }
    hasher.finish()
}

/// What a row's checks indicator shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ChecksShown<'a> {
    /// Asked for (or about to be), nothing known yet.
    Loading,
    /// Only the listing's combined state; the checks themselves are on their way.
    Rollup(CheckState),
    Known(&'a RowStatus),
    /// Not known, and why: not offered without a token, or the request failed.
    Unknown(&'a str),
}

/// How a row's checks come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// One request for the rows of a repository.
    Batch,
    /// One request per pull request.
    Single,
    /// Not asked for; the reason shows instead.
    NotOffered(&'static str),
}

fn plan(row: &StatusRow) -> Plan {
    match (row.kind, row.has_token) {
        (ProviderKind::AzureDevOps, true) => Plan::Single,
        (ProviderKind::GitHub | ProviderKind::GitLab, true) => Plan::Batch,
        (ProviderKind::AzureDevOps, false) => Plan::NotOffered(
            "Azure DevOps shows branch policies only to signed-in users. Add a token to see them here.",
        ),
        (ProviderKind::GitHub, false) => Plan::NotOffered(
            "Without a token GitHub lists a repository's pull requests without their checks. Add a token to see them here.",
        ),
        (ProviderKind::GitLab, false) => Plan::NotOffered(
            "Without a token GitLab allows only a few GraphQL requests an hour, so pipelines and approvals are not loaded here. Add a token to see them.",
        ),
    }
}

impl Plan {
    /// The kind of request the row's checks go out with (see the module doc), whose rate-limit
    /// pause holds them back.
    fn api(self) -> Option<Api> {
        match self {
            Plan::Batch => Some(Api::GraphQl),
            Plan::Single => Some(Api::Rest),
            Plan::NotOffered(_) => None,
        }
    }
}

/// The platform's listing carries every reviewer's vote, so a vote changes the row's fingerprint.
fn lists_votes(kind: ProviderKind) -> bool {
    match kind {
        ProviderKind::AzureDevOps => true,
        ProviderKind::GitHub | ProviderKind::GitLab => false,
    }
}

#[derive(Default)]
pub(crate) struct RowStatuses {
    /// By storage key.
    entries: HashMap<String, Entry>,
    running: Vec<Running>,
    /// The last write of the user's the answers' reviewers show ([`Writes::last`]).
    writes_seen: u64,
    /// When the page last refreshed ([`Self::renew`]): older answers of rows whose listing cannot
    /// show a vote are asked again.
    renewed_at: f64,
}

struct Entry {
    /// When the latest request for the row started.
    requested_at: f64,
    /// The row's [`StatusRow::fingerprint`] then.
    fingerprint: u64,
    phase: Phase,
    /// The latest answer, shown while a newer one loads.
    latest: Option<RowStatus>,
}

enum Phase {
    Running,
    Done,
    Failed(String),
}

/// One request and the rows it asks for: (storage key, number).
struct Running {
    rows: Vec<(String, u64)>,
    load: Load<Vec<(u64, RowStatus)>>,
}

impl RowStatuses {
    /// What `row`'s checks indicator shows; `rollup` is the listing's combined state, if any.
    pub(crate) fn checks<'a>(
        &'a self,
        row: &StatusRow,
        rollup: Option<RunStatus>,
    ) -> ChecksShown<'a> {
        if let Plan::NotOffered(reason) = plan(row) {
            return rollup
                .map_or(ChecksShown::Unknown(reason), |run| ChecksShown::Rollup(run.into()));
        }
        let entry = self.entries.get(&row.storage_key);
        match entry.map(|entry| (&entry.phase, entry.latest.as_ref())) {
            Some((Phase::Failed(message), _)) => ChecksShown::Unknown(message),
            Some((_, Some(status))) => ChecksShown::Known(status),
            Some((Phase::Done, None)) => {
                ChecksShown::Unknown("The platform did not report checks for this pull request.")
            }
            Some((Phase::Running, None)) | None => {
                rollup.map_or(ChecksShown::Loading, |run| ChecksShown::Rollup(run.into()))
            }
        }
    }

    /// The reviewers to show: the answer's where it brought votes the listing lacks, else `listed`.
    pub(crate) fn reviewers<'a>(
        &'a self,
        row: &StatusRow,
        listed: &'a [Reviewer],
    ) -> &'a [Reviewer] {
        self.entries
            .get(&row.storage_key)
            .and_then(|entry| entry.latest.as_ref())
            .and_then(|status| status.reviewers.as_deref())
            .unwrap_or(listed)
    }

    /// The approvals the platform requires of the row, once its answer is in.
    pub(crate) fn required_approvals(&self, row: &StatusRow) -> Option<&Check> {
        self.entries
            .get(&row.storage_key)
            .and_then(|entry| entry.latest.as_ref())
            .and_then(|status| status.required_approvals.as_ref())
    }

    /// A request for `row`'s checks and votes is on its way.
    pub(crate) fn is_running(&self, row: &StatusRow) -> bool {
        self.entries
            .get(&row.storage_key)
            .is_some_and(|entry| matches!(entry.phase, Phase::Running))
    }

    /// A request for `row`'s checks and votes is on its way, but a rate-limit pause on its kind of
    /// request holds it back: it waits, for up to an hour, and goes out on its own after.
    pub(crate) fn is_held(&self, row: &StatusRow, pauses: &Pauses) -> bool {
        self.is_running(row)
            && plan(row).api().is_some_and(|api| pauses.holds(&row.connection_id, api))
    }

    /// Why the last request for `row`'s checks and votes failed.
    pub(crate) fn failure(&self, row: &StatusRow) -> Option<&str> {
        match &self.entries.get(&row.storage_key)?.phase {
            Phase::Failed(message) => Some(message),
            Phase::Running | Phase::Done => None,
        }
    }

    /// The page refreshed at `now`: answers older than that are asked again where the listing
    /// cannot show a vote, once their listing answered after this.
    pub(crate) fn renew(&mut self, now: f64) {
        self.renewed_at = now;
    }

    /// Nothing asked for yet, or the listing answered since the last request and the row changed,
    /// still had checks running, failed, or was renewed since; never while a request for it runs.
    fn is_due(&self, row: &StatusRow) -> bool {
        let Some(entry) = self.entries.get(&row.storage_key) else {
            return true;
        };
        let is_relisted = row.listed_at > entry.requested_at;
        let is_unsettled = match &entry.phase {
            Phase::Running => return false,
            Phase::Failed(_) => true,
            Phase::Done => entry
                .latest
                .as_ref()
                .is_some_and(|status| status.verdict() == Some(CheckState::Pending)),
        };
        let is_renewed = entry.requested_at < self.renewed_at && !lists_votes(row.kind);
        is_relisted && (is_unsettled || is_renewed || entry.fingerprint != row.fingerprint)
    }

    fn start(&mut self, rows: &[&StatusRow], now: f64, load: Load<Vec<(u64, RowStatus)>>) {
        for row in rows {
            let latest = self.entries.remove(&row.storage_key).and_then(|entry| entry.latest);
            self.entries.insert(
                row.storage_key.clone(),
                Entry {
                    requested_at: now,
                    fingerprint: row.fingerprint,
                    phase: Phase::Running,
                    latest,
                },
            );
        }
        let rows = rows.iter().map(|row| (row.storage_key.clone(), row.number)).collect();
        self.running.push(Running { rows, load });
    }

    /// Moves finished answers in. Where an answer brought the reviewers (shown instead of the
    /// listing's, see [`Self::reviewers`]), the user's writes on them apply as they do to the
    /// listing's rows: to every answer that arrives, and to all once a write was logged since.
    pub(crate) fn poll(&mut self, writes: &Writes, now: f64) {
        let mut answered = Vec::new();
        let mut index = 0;
        while index < self.running.len() {
            if !self.running[index].load.poll() {
                index += 1;
                continue;
            }
            let Running { rows, load } = self.running.swap_remove(index);
            for (storage_key, number) in rows {
                let Some(entry) = self.entries.get_mut(&storage_key) else {
                    continue;
                };
                match &load {
                    Load::Ready(answers) => {
                        entry.phase = Phase::Done;
                        entry.latest = answers
                            .iter()
                            .find(|(answered, _)| *answered == number)
                            .map(|(_, status)| status.clone());
                        answered.push(storage_key);
                    }
                    Load::Failed(message) => entry.phase = Phase::Failed(message.clone()),
                    Load::Idle | Load::Loading(_) => {}
                }
            }
        }
        let is_behind = self.writes_seen != writes.last();
        self.writes_seen = writes.last();
        for (storage_key, entry) in &mut self.entries {
            if !is_behind && !answered.contains(storage_key) {
                continue;
            }
            let reviewers = entry.latest.as_mut().and_then(|status| status.reviewers.as_mut());
            if let Some(reviewers) = reviewers {
                writes.settle_reviewers(storage_key, reviewers, now);
            }
        }
    }

    /// `row`'s answer as if it had arrived.
    #[cfg(test)]
    pub fn answered_for_test(&mut self, row: &StatusRow, status: RowStatus) {
        self.insert_for_test(row, Phase::Done, Some(status));
    }

    /// `row`'s request as if it had just left.
    #[cfg(test)]
    pub fn running_for_test(&mut self, row: &StatusRow) {
        self.insert_for_test(row, Phase::Running, None);
    }

    /// `row`'s request as if it had failed with `message`.
    #[cfg(test)]
    pub fn failed_for_test(&mut self, row: &StatusRow, message: &str) {
        self.insert_for_test(row, Phase::Failed(message.to_owned()), None);
    }

    /// `row`'s entry in `phase` with the answer `latest`, asked for when its listing answered.
    #[cfg(test)]
    fn insert_for_test(&mut self, row: &StatusRow, phase: Phase, latest: Option<RowStatus>) {
        let entry =
            Entry { requested_at: row.listed_at, fingerprint: row.fingerprint, phase, latest };
        self.entries.insert(row.storage_key.clone(), entry);
    }
}

/// Asks for the checks of the rows on screen that need them, unless the window is inactive (then
/// they wait). Rows of one repository that a platform batches go out in one request.
pub(crate) fn want(statuses: &mut RowStatuses, services: &mut Services, visible: &[StatusRow]) {
    let is_active = services.background.ctx().input(|input| is_window_active(input.viewport()));
    if !is_active {
        return;
    }
    let now = services.now();
    for row in visible {
        if statuses.running.len() >= MAX_RUNNING || !statuses.is_due(row) {
            continue;
        }
        let rows: Vec<&StatusRow> = match plan(row) {
            Plan::NotOffered(_) => continue,
            Plan::Single => vec![row],
            Plan::Batch => visible
                .iter()
                .filter(|other| {
                    other.connection_id == row.connection_id
                        && other.repository_key == row.repository_key
                        && (other.storage_key == row.storage_key || statuses.is_due(other))
                })
                .fold(Vec::new(), |mut rows, other| {
                    if rows.iter().all(|known: &&StatusRow| known.number != other.number) {
                        rows.push(other);
                    }
                    rows
                }),
        };
        let Some(provider) = services.provider(&row.connection_id) else {
            continue;
        };
        let repository = row.repository_key.clone();
        let numbers: Vec<u64> = rows.iter().map(|row| row.number).collect();
        let mut load = Load::Idle;
        load.start(&services.background, async move {
            provider.row_statuses(&repository, &numbers).await
        });
        statuses.start(&rows, now, load);
    }
}

#[cfg(test)]
mod tests {
    use review_core::Check;

    use super::*;

    fn row(kind: ProviderKind, has_token: bool) -> StatusRow {
        StatusRow {
            connection_id: "c".into(),
            kind,
            has_token,
            repository_key: "group/app".into(),
            number: 7,
            storage_key: "c|group/app|7".into(),
            listed_at: 10.0,
            fingerprint: 1,
        }
    }

    fn status(state: CheckState) -> RowStatus {
        RowStatus { checks: vec![Check::new("build", state)], ..RowStatus::default() }
    }

    #[test]
    fn a_settled_row_asks_again_only_when_the_listing_shows_it_changed() {
        let mut statuses = RowStatuses::default();
        let listed = row(ProviderKind::AzureDevOps, true);
        assert!(statuses.is_due(&listed), "never asked");
        statuses.answered_for_test(&listed, status(CheckState::Passed));
        assert!(!statuses.is_due(&listed));
        let relisted = StatusRow { listed_at: 30.0, ..listed.clone() };
        assert!(!statuses.is_due(&relisted), "listed again, unchanged and settled");
        let pushed = StatusRow { fingerprint: 2, ..relisted };
        assert!(statuses.is_due(&pushed), "a push, an update or a vote");
        assert!(
            !statuses.is_due(&StatusRow { listed_at: 10.0, ..pushed }),
            "the listing has not answered since the request"
        );
    }

    /// A GitLab approval may leave the listing as it was: after a refresh of the page its rows ask
    /// again once their listing answered. Azure DevOps lists the votes, so its rows do not.
    #[test]
    fn a_refresh_renews_the_votes_the_listing_cannot_show() {
        for (kind, asks_again) in [
            (ProviderKind::GitLab, true),
            (ProviderKind::GitHub, true),
            (ProviderKind::AzureDevOps, false),
        ] {
            let mut statuses = RowStatuses::default();
            let listed = row(kind, true);
            statuses.answered_for_test(&listed, status(CheckState::Passed));
            let relisted = StatusRow { listed_at: 30.0, ..listed.clone() };
            assert!(!statuses.is_due(&relisted), "{kind:?}: unchanged and settled");
            statuses.renew(20.0);
            assert!(!statuses.is_due(&listed), "{kind:?}: the listing has not answered since");
            assert_eq!(statuses.is_due(&relisted), asks_again, "{kind:?}");
        }
    }

    #[test]
    fn running_checks_and_failures_ask_again_with_the_next_listing() {
        let mut statuses = RowStatuses::default();
        let listed = row(ProviderKind::GitLab, true);
        statuses.answered_for_test(&listed, status(CheckState::Pending));
        assert!(!statuses.is_due(&listed));
        let relisted = StatusRow { listed_at: 30.0, ..listed.clone() };
        assert!(statuses.is_due(&relisted), "a running check shows its outcome after a refresh");
        statuses.failed_for_test(&listed, "403 Forbidden");
        assert_eq!(statuses.checks(&listed, None), ChecksShown::Unknown("403 Forbidden"));
        assert!(statuses.is_due(&relisted), "a failure is tried again with the next listing");
    }

    #[test]
    fn adding_me_as_reviewer_shows_me_where_the_answer_brought_the_reviewers() {
        use review_core::{ReviewVote, UserIdentity};

        use cv_runtime::route::PullRequestKey;
        use cv_services::writes::{Added, Effect};

        let me = UserIdentity {
            id: "1".into(),
            display_name: "Ada".into(),
            login: Some("ada".into()),
            avatar_url: None,
        };
        let key = |number| PullRequestKey {
            connection_id: "c".into(),
            repository_key: "group/app".into(),
            number,
        };
        let mut statuses = RowStatuses::default();
        let mut writes = Writes::default();
        let listed = row(ProviderKind::GitLab, true);
        let with_votes = RowStatus { reviewers: Some(Vec::new()), ..status(CheckState::Passed) };
        statuses.answered_for_test(&listed, with_votes);
        let added = Effect::ReviewerAdded(Added::Me);
        assert!(writes.logged_for_test(&key(7), added.clone(), Some(me.clone()), 1.0));
        statuses.poll(&writes, 1.0);
        statuses.poll(&writes, 1.0);
        let shown = statuses.reviewers(&listed, &[]);
        assert_eq!(shown.len(), 1, "listed once: {shown:?}");
        assert_eq!(shown[0].identity, me);
        assert_eq!(shown[0].vote, ReviewVote::None, "pending, no vote");

        // An answer without reviewers keeps showing the listing's, which the page updates itself.
        let other = StatusRow { storage_key: "c|group/app|8".into(), number: 8, ..listed };
        statuses.answered_for_test(&other, status(CheckState::Passed));
        assert!(writes.logged_for_test(&key(8), added, Some(me), 2.0));
        statuses.poll(&writes, 2.0);
        assert_eq!(statuses.reviewers(&other, &[]), []);
    }

    /// A pause holds a row's request back only where it pauses the kind the request goes out with:
    /// GraphQL for the batched platforms, REST for Azure DevOps' policy evaluations.
    #[test]
    fn a_running_request_is_held_by_the_pause_of_its_kind_only() {
        for (kind, held_by, goes_on_under) in [
            (ProviderKind::GitLab, Api::GraphQl, Api::Rest),
            (ProviderKind::GitHub, Api::GraphQl, Api::Rest),
            (ProviderKind::AzureDevOps, Api::Rest, Api::GraphQl),
        ] {
            let mut statuses = RowStatuses::default();
            let listed = row(kind, true);
            assert!(!statuses.is_held(&listed, &Pauses::of_test("c", held_by)), "{kind:?}: idle");
            statuses.running_for_test(&listed);
            assert!(statuses.is_held(&listed, &Pauses::of_test("c", held_by)), "{kind:?}");
            assert!(!statuses.is_held(&listed, &Pauses::of_test("c", goes_on_under)), "{kind:?}");
            assert!(
                !statuses.is_held(&listed, &Pauses::of_test("other", held_by)),
                "{kind:?}: another connection's pause"
            );
        }
    }

    #[test]
    fn rows_without_a_token_show_why_and_keep_the_listings_rollup() {
        let statuses = RowStatuses::default();
        for kind in [ProviderKind::AzureDevOps, ProviderKind::GitHub, ProviderKind::GitLab] {
            let anonymous = row(kind, false);
            assert!(matches!(plan(&anonymous), Plan::NotOffered(_)), "{kind:?}");
            assert!(
                matches!(statuses.checks(&anonymous, None), ChecksShown::Unknown(why) if why.contains("token")),
                "{kind:?}"
            );
        }
        let with_rollup =
            statuses.checks(&row(ProviderKind::GitHub, false), Some(RunStatus::Failed));
        assert_eq!(with_rollup, ChecksShown::Rollup(CheckState::Failed));
        assert_eq!(
            statuses.checks(&row(ProviderKind::GitHub, true), Some(RunStatus::Running)),
            ChecksShown::Rollup(CheckState::Pending),
            "the listing's state shows until the checks arrive"
        );
        assert_eq!(statuses.checks(&row(ProviderKind::GitLab, true), None), ChecksShown::Loading);
    }
}
