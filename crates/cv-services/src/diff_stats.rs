//! Lines and files each listed pull request changes, for the size column of the list rows.
//!
//! GitHub's search rows bring them along. Elsewhere they cost requests, so they are asked for only
//! for rows on screen and only while the window is active, and kept per pull request with the
//! revision (head commit) they were counted for: an unchanged pull request is never asked again,
//! and a pushed one never shows the numbers of its older head.
//!
//! - GitLab: one GraphQL request (`diffStatsSummary`) for every listed merge request of a project.
//!   Without a token GitLab allows few GraphQL requests an hour, so anonymous rows wait for the
//!   row's "Count lines".
//! - Azure DevOps reports no lines: the changed files of the latest iteration (with a token), then
//!   the lines from git. From the local mirror when it holds both commits and the changed blobs (no
//!   network), else, for pull requests of at most [`AUTO_COUNT_FILES`] files, by preparing the
//!   mirror the way the pull request's page does: opening it then finds commits, blobs and the file
//!   list ready. Larger ones wait for "Count lines".
//! - GitHub rows of a repository's REST listing wait for "Count lines" (one request each).
//!
//! Favorites (pull requests of a starred repository) are always counted on their own, whatever
//! their size and whether the connection has a token: GitLab's one request per project, Azure
//! DevOps' lines from git also beyond [`AUTO_COUNT_FILES`], one request per GitHub REST row. At most
//! [`MAX_FAVORITES_RUNNING`] of them count at once, so the other cards' rows keep their turn.
//!
//! A row the pointer rests on is prepared the same way ahead of the click ([`warm`]), the line
//! counts left out for pull requests over [`AUTO_COUNT_FILES`] files.

use std::collections::HashMap;
use std::sync::Arc;

use review_core::{
    DiffStat, LineCounts, Provider, ProviderError, ProviderKind, PullRequestDetail,
    PullRequestSummary,
};
use review_git::{ChangedFile, GitMirror};
use review_store::retention::MirrorUse;

use cv_base::platform;
use cv_runtime::cache::page_cache::pull_request_part;
use cv_runtime::load::Load;
use cv_runtime::route::PullRequestKey;

use crate::Services;
use crate::mirrors::MirrorRegistry;
use crate::writes::Answer;

/// Azure DevOps pull requests with at most this many changed files have their lines counted on
/// their own: their blobs download quickly, and their page reuses them.
pub(crate) const AUTO_COUNT_FILES: u64 = 20;
/// Automatic counts in flight at once; further rows wait their turn.
const MAX_RUNNING: usize = 4;
/// Of those, at most this many for favorites: the rows of the other cards always have the rest.
const MAX_FAVORITES_RUNNING: usize = 2;

/// A list row, as far as counting its lines goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatRow {
    pub connection_id: String,
    pub kind: ProviderKind,
    pub has_token: bool,
    /// The repository is starred: its rows are counted whatever their size.
    pub is_favorite: bool,
    pub repository_key: String,
    pub number: u64,
    /// The pull request's storage key: counts, and what is prepared for its page, are kept by it.
    pub storage_key: String,
    /// What counts are valid for: the head commit, else the last update.
    pub revision: Option<String>,
    pub head_sha: Option<String>,
    pub target_sha: Option<String>,
    /// What the listing reported.
    pub listed: DiffStat,
}

impl StatRow {
    pub fn new(
        connection_id: &str,
        kind: ProviderKind,
        has_token: bool,
        is_favorite: bool,
        storage_key: String,
        summary: &PullRequestSummary,
    ) -> Self {
        let revision = summary
            .head_sha
            .clone()
            .or_else(|| summary.updated_at.map(|at| at.unix_timestamp_nanos().to_string()));
        Self {
            connection_id: connection_id.to_owned(),
            kind,
            has_token,
            is_favorite,
            repository_key: summary.repository_key.clone(),
            number: summary.number,
            storage_key,
            revision,
            head_sha: summary.head_sha.clone(),
            target_sha: summary.target_sha.clone(),
            listed: summary.diff_stat,
        }
    }
}

/// What a row's size column shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    /// The lines are known; the files too, where reported.
    Counted(DiffStat),
    /// Being counted, or about to be.
    Counting,
    /// Not counted: the files where known, and why (the tooltip of "Count lines").
    Uncounted { files: Option<u64>, why: String },
}

/// How a row's counts come when nobody asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// One request per project for every listed merge request of it.
    Batch,
    /// The changed files, then the lines from git (small pull requests).
    PerPullRequest,
    /// Only when the user asks.
    OnRequest,
}

fn plan(row: &StatRow) -> Plan {
    match row.kind {
        ProviderKind::GitLab if row.has_token || row.is_favorite => Plan::Batch,
        // Without a token the files are unknown and nothing is fetched, but a mirror that holds
        // everything still counts the lines for free.
        ProviderKind::AzureDevOps => Plan::PerPullRequest,
        // A favorite's row of a repository's REST listing: one request for it.
        ProviderKind::GitHub if row.is_favorite => Plan::PerPullRequest,
        ProviderKind::GitLab | ProviderKind::GitHub => Plan::OnRequest,
    }
}

#[derive(Default)]
pub struct DiffStats {
    /// By storage key.
    entries: HashMap<String, Entry>,
    running: Vec<Running>,
}

struct Entry {
    revision: Option<String>,
    state: State,
}

enum State {
    Running,
    /// What the platform and git told; parts may be missing (a platform that has not worked them out).
    Done(DiffStat),
    /// The files; the lines are left for "Count lines": too many files to count on their own.
    Skipped(DiffStat),
    Failed(String),
}

/// One request (or chain of them) and the rows it counts: (storage key, number, revision).
struct Running {
    rows: Vec<(String, u64, Option<String>)>,
    is_automatic: bool,
    is_favorite: bool,
    /// When it started ([`Services::now`]): what it prepares was asked for then.
    asked_at: f64,
    load: Load<Outcome>,
}

struct Outcome {
    stats: Vec<(u64, DiffStat)>,
    prepared: Option<Prepared>,
    /// The lines were left out on purpose ([`State::Skipped`]).
    is_skipped: bool,
}

impl Outcome {
    fn counted(stats: Vec<(u64, DiffStat)>) -> Self {
        Self { stats, prepared: None, is_skipped: false }
    }
}

/// What preparing the mirror for a pull request found, which its page then shows at once.
struct Prepared {
    key: PullRequestKey,
    detail: PullRequestDetail,
    files: Arc<Vec<ChangedFile>>,
}

impl DiffStats {
    pub fn shown(&self, row: &StatRow) -> Shown {
        if row.listed.lines.is_some() {
            return Shown::Counted(row.listed);
        }
        let entry =
            self.entries.get(&row.storage_key).filter(|entry| entry.revision == row.revision);
        match entry.map(|entry| &entry.state) {
            Some(State::Done(stat)) if stat.lines.is_some() => Shown::Counted(stat.or(row.listed)),
            Some(State::Done(stat) | State::Skipped(stat)) => {
                Shown::Uncounted { files: stat.files.or(row.listed.files), why: why(row, true) }
            }
            Some(State::Failed(message)) => Shown::Uncounted {
                files: row.listed.files,
                why: format!("Could not count the lines: {message}"),
            },
            None if plan(row) == Plan::OnRequest => {
                Shown::Uncounted { files: row.listed.files, why: why(row, false) }
            }
            Some(State::Running) | None => Shown::Counting,
        }
    }

    /// Nothing known for the row's current revision, and nothing on its way; for a favorite also
    /// when its lines were skipped before its repository was starred.
    fn is_due(&self, row: &StatRow) -> bool {
        row.listed.lines.is_none()
            && self.entries.get(&row.storage_key).is_none_or(|entry| {
                entry.revision != row.revision
                    || (row.is_favorite && matches!(entry.state, State::Skipped(_)))
            })
    }

    /// A count of `row`'s current revision is on its way.
    pub fn is_running(&self, row: &StatRow) -> bool {
        self.entries.get(&row.storage_key).is_some_and(|entry| {
            entry.revision == row.revision && matches!(entry.state, State::Running)
        })
    }

    /// `row`'s lines, from the file list its page was prepared with ahead of time.
    pub fn counted(&mut self, row: &StatRow, files: &[ChangedFile]) {
        let state = State::Done(totals(files));
        self.entries
            .insert(row.storage_key.clone(), Entry { revision: row.revision.clone(), state });
    }

    /// An automatic count of `row` may start now: fewer than [`MAX_RUNNING`] are in flight, and
    /// for a favorite fewer than [`MAX_FAVORITES_RUNNING`] favorites.
    fn has_room(&self, row: &StatRow) -> bool {
        let automatic = self.running.iter().filter(|running| running.is_automatic);
        let (all, favorites) = automatic.fold((0, 0), |(all, favorites), running| {
            (all + 1, favorites + usize::from(running.is_favorite))
        });
        all < MAX_RUNNING && (!row.is_favorite || favorites < MAX_FAVORITES_RUNNING)
    }

    /// `row`'s counts as if they had arrived.
    #[cfg(any(test, feature = "test-support"))]
    pub fn counted_for_test(&mut self, row: &StatRow, stat: DiffStat) {
        self.entries.insert(
            row.storage_key.clone(),
            Entry { revision: row.revision.clone(), state: State::Done(stat) },
        );
    }

    fn start(&mut self, rows: &[&StatRow], is_automatic: bool, asked_at: f64, load: Load<Outcome>) {
        for row in rows {
            self.entries.insert(
                row.storage_key.clone(),
                Entry { revision: row.revision.clone(), state: State::Running },
            );
        }
        let is_favorite = rows.first().is_some_and(|row| row.is_favorite);
        let rows = rows
            .iter()
            .map(|row| (row.storage_key.clone(), row.number, row.revision.clone()))
            .collect();
        self.running.push(Running { rows, is_automatic, is_favorite, asked_at, load });
    }

    /// Keeps what a finished count found for its `rows`. A row pushed while it ran is being counted
    /// for its new head already; the older head's numbers do not replace that count.
    fn store(
        &mut self,
        rows: Vec<(String, u64, Option<String>)>,
        stats: &[(u64, DiffStat)],
        is_skipped: bool,
        failure: Option<&str>,
    ) {
        for (storage_key, number, revision) in rows {
            let is_superseded = self.entries.get(&storage_key).is_some_and(|entry| {
                entry.revision != revision && matches!(entry.state, State::Running)
            });
            if is_superseded {
                continue;
            }
            let stat = || {
                stats
                    .iter()
                    .find(|(reported, _)| *reported == number)
                    .map(|(_, stat)| *stat)
                    .unwrap_or_default()
            };
            let state = match failure {
                Some(message) => State::Failed(message.to_owned()),
                None if is_skipped => State::Skipped(stat()),
                None => State::Done(stat()),
            };
            self.entries.insert(storage_key, Entry { revision, state });
        }
    }
}

/// Why a row has no line counts yet. `was_asked`: the platform answered without them.
fn why(row: &StatRow, was_asked: bool) -> String {
    let text = match row.kind {
        ProviderKind::AzureDevOps => {
            "Azure DevOps doesn't report line counts; counting reads the changed files once and is reused when you open the pull request."
        }
        ProviderKind::GitLab if was_asked => {
            "GitLab has not worked out the changes of this merge request yet."
        }
        ProviderKind::GitLab => {
            "Without a token GitLab allows only a few GraphQL requests an hour, so lines are counted when you ask: one request for every listed merge request of this project."
        }
        ProviderKind::GitHub => {
            "GitHub lists a repository's pull requests without line counts; counting asks for this pull request once."
        }
    };
    text.to_owned()
}

/// Counts the rows on screen that need it, unless the window is inactive (then they wait, still
/// shown as counting). `project_rows` gives every loaded row of a row's repository, which a
/// batching platform counts together.
pub fn want<'a>(
    services: &mut Services,
    visible: impl IntoIterator<Item = &'a StatRow>,
    project_rows: impl Fn(&StatRow) -> Vec<StatRow>,
) {
    let is_active =
        services.background.ctx().input(|input| platform::is_window_active(input.viewport()));
    if !is_active {
        return;
    }
    for row in visible {
        if !services.diff_stats.is_due(row) || !services.diff_stats.has_room(row) {
            continue;
        }
        match plan(row) {
            Plan::Batch => start_batch(services, row, &project_rows(row), true),
            Plan::PerPullRequest => start_single(services, row, false),
            Plan::OnRequest => {}
        }
    }
}

/// The user asked to count `row` ("Count lines"): never paused, also after a failure.
pub fn count(
    services: &mut Services,
    row: &StatRow,
    project_rows: impl Fn(&StatRow) -> Vec<StatRow>,
) {
    if services.diff_stats.is_running(row) {
        return;
    }
    match row.kind {
        ProviderKind::GitLab => start_batch(services, row, &project_rows(row), false),
        ProviderKind::AzureDevOps | ProviderKind::GitHub => start_single(services, row, true),
    }
}

/// One request for `row` and every other row of its project that has no counts yet.
fn start_batch(
    services: &mut Services,
    row: &StatRow,
    project_rows: &[StatRow],
    is_automatic: bool,
) {
    let Some(provider) = services.provider(&row.connection_id) else {
        return;
    };
    let mut rows: Vec<&StatRow> = vec![row];
    rows.extend(
        project_rows
            .iter()
            .filter(|other| other.number != row.number && services.diff_stats.is_due(other)),
    );
    let numbers: Vec<u64> = rows.iter().map(|row| row.number).collect();
    let repository = row.repository_key.clone();
    let mut load = Load::Idle;
    load.start(&services.background, async move {
        Ok(Outcome::counted(provider.diff_stats(&repository, &numbers).await?))
    });
    let asked_at = services.now();
    services.diff_stats.start(&rows, is_automatic, asked_at, load);
}

/// One pull request. Azure DevOps counts from git (see the module docs); `force` (the user asked)
/// and a favorite prepare the mirror whatever the size, without asking for the files first.
fn start_single(services: &mut Services, row: &StatRow, force: bool) {
    let Some(provider) = services.provider(&row.connection_id) else {
        return;
    };
    let mirrors = services.mirrors.clone();
    let task_row = row.clone();
    let whatever_the_size = force || row.is_favorite;
    let mut load = Load::Idle;
    load.start(&services.background, async move {
        if task_row.kind == ProviderKind::AzureDevOps {
            count_from_git(provider.as_ref(), &mirrors, &task_row, whatever_the_size).await
        } else {
            Ok(Outcome::counted(
                provider.diff_stats(&task_row.repository_key, &[task_row.number]).await?,
            ))
        }
    });
    let asked_at = services.now();
    services.diff_stats.start(&[row], !force, asked_at, load);
}

async fn count_from_git(
    provider: &dyn Provider,
    mirrors: &MirrorRegistry,
    row: &StatRow,
    whatever_the_size: bool,
) -> Result<Outcome, ProviderError> {
    let number = row.number;
    let mut stat = row.listed;
    if !whatever_the_size && stat.files.is_none() {
        let reported = provider.diff_stats(&row.repository_key, &[number]).await?;
        stat = reported
            .into_iter()
            .find(|(reported, _)| *reported == number)
            .map_or(stat, |(_, reported)| reported.or(stat));
    }
    if let (Some(target), Some(head)) = (&row.target_sha, &row.head_sha) {
        let remote = provider.git_remote(&row.repository_key).await?;
        if let Some(git) = mirrors.existing(&remote).await?
            && let Some(files) = git.local_changed_files(target, head).await?
        {
            return Ok(Outcome::counted(vec![(number, totals(&files))]));
        }
    }
    if !whatever_the_size && stat.files.is_none_or(|files| files > AUTO_COUNT_FILES) {
        return Ok(Outcome { stats: vec![(number, stat)], prepared: None, is_skipped: true });
    }
    let prepared = prepare(provider, mirrors, row).await?;
    Ok(Outcome {
        stats: vec![(number, totals(&prepared.files))],
        prepared: Some(prepared),
        is_skipped: false,
    })
}

/// What the pull request's page does before its Files tab shows: the commits ([`prepare_commits`]),
/// and the detailed file list, whose line counts download the changed blobs. Nothing shows the
/// counts on the way, so none is published.
async fn prepare(
    provider: &dyn Provider,
    mirrors: &MirrorRegistry,
    row: &StatRow,
) -> Result<Prepared, ProviderError> {
    let detail = provider.pull_request(&row.repository_key, row.number).await?;
    let Commits { detail, git, merge_base } =
        prepare_commits(detail, provider, mirrors, row).await?;
    let files = git.changed_files_detailed(&merge_base, &detail.head_sha, &|_| {}).await?;
    let key = PullRequestKey {
        connection_id: row.connection_id.clone(),
        repository_key: row.repository_key.clone(),
        number: row.number,
    };
    Ok(Prepared { key, detail, files: Arc::new(files) })
}

/// The detail, and the mirror with both commits and their merge base: what the page waits for
/// before it lists the files.
struct Commits {
    detail: PullRequestDetail,
    git: Arc<GitMirror>,
    merge_base: String,
}

async fn prepare_commits(
    detail: PullRequestDetail,
    provider: &dyn Provider,
    mirrors: &MirrorRegistry,
    row: &StatRow,
) -> Result<Commits, ProviderError> {
    let remote = provider.git_remote(&row.repository_key).await?;
    let local_refs: Vec<String> =
        detail.git_fetch_refs.iter().map(|name| review_git::local_ref_name(name)).collect();
    let usage = MirrorUse {
        connection_id: &row.connection_id,
        pull_request: &row.storage_key,
        local_refs: &local_refs,
    };
    let git = mirrors.open(&remote, &usage).await?;
    git.ensure_commits(&detail.git_fetch_refs, &[&detail.target_sha, &detail.head_sha], &|_| {})
        .await?;
    let merge_base = git.merge_base(&detail.target_sha, &detail.head_sha).await?;
    Ok(Commits { detail, git, merge_base })
}

/// The git part of a pull request's page, made ready ahead of the click for `detail`, read
/// ahead already: the commits in the mirror, and for small pull requests the counted file list
/// (the tree-only list tells, from the mirror), whose few blobs download quickly; larger ones
/// leave their blobs to the page and answer `None`.
pub async fn warm(
    detail: PullRequestDetail,
    provider: &dyn Provider,
    mirrors: &MirrorRegistry,
    row: &StatRow,
) -> Result<Option<Arc<Vec<ChangedFile>>>, ProviderError> {
    let Commits { detail, git, merge_base } =
        prepare_commits(detail, provider, mirrors, row).await?;
    let changed = git.changed_files_fast(&merge_base, &detail.head_sha).await?;
    if changed.len() as u64 > AUTO_COUNT_FILES {
        return Ok(None);
    }
    let files = git.changed_files_detailed(&merge_base, &detail.head_sha, &|_| {}).await?;
    Ok(Some(Arc::new(files)))
}

fn totals(files: &[ChangedFile]) -> DiffStat {
    let lines = LineCounts {
        added: files.iter().filter_map(|file| file.additions).map(u64::from).sum(),
        deleted: files.iter().filter_map(|file| file.deletions).map(u64::from).sum(),
    };
    DiffStat { lines: Some(lines), files: Some(files.len() as u64) }
}

/// Moves finished counts in; what was prepared for a pull request's page goes into the page cache,
/// stamped with when it was asked for and with the user's writes the host may not show yet
/// applied, so a detail read before a write never undoes it there.
pub fn poll(services: &mut Services) {
    let now = services.now();
    let Services { diff_stats, cache, writes, .. } = services;
    let mut index = 0;
    while index < diff_stats.running.len() {
        let Some(result) = diff_stats.running[index].load.take_finished() else {
            index += 1;
            continue;
        };
        let Running { rows, asked_at, .. } = diff_stats.running.swap_remove(index);
        let (stats, is_skipped, failure) = match result {
            Ok(Outcome { stats, prepared, is_skipped }) => {
                if let Some(Prepared { key, mut detail, files }) = prepared {
                    writes.settle_detail(&key, &mut detail, Answer::Host { asked_at }, now);
                    let part = |name: &str| pull_request_part(&key.storage_key(), name);
                    cache.put_answer(&part("detail"), detail, asked_at, now);
                    cache.put_answer(&part("files"), files, asked_at, now);
                }
                (stats, is_skipped, None)
            }
            Err(message) => (Vec::new(), false, Some(message)),
        };
        diff_stats.store(rows, &stats, is_skipped, failure.as_deref());
    }
}

/// How full the size column's bar is for `total` changed lines: logarithmic, so a one-line fix
/// still shows and everything from 5,000 lines on fills it.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "a fraction of the bar's width: the line count only needs f64 precision, the result \
              is in 0..=1"
)]
pub fn bar_fraction(total: u64) -> f32 {
    ((total as f64 + 1.0).log10() / 5001f64.log10()).min(1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How a count of a row arrives.
    #[derive(Clone, Copy)]
    enum Arrival<'a> {
        Done(DiffStat),
        Skipped(DiffStat),
        Failed(&'a str),
    }

    /// `row`'s count starts and arrives as `how` says, the way a finished count lands.
    fn arrived(stats: &mut DiffStats, row: &StatRow, how: Arrival<'_>) {
        stats.start(&[row], true, 0.0, Load::Idle);
        let (stat, is_skipped, failure) = match how {
            Arrival::Done(stat) => (stat, false, None),
            Arrival::Skipped(stat) => (stat, true, None),
            Arrival::Failed(message) => (DiffStat::default(), false, Some(message)),
        };
        let rows = vec![(row.storage_key.clone(), row.number, row.revision.clone())];
        stats.store(rows, &[(row.number, stat)], is_skipped, failure);
    }

    fn row(kind: ProviderKind, has_token: bool, listed: DiffStat) -> StatRow {
        StatRow {
            connection_id: "c".into(),
            kind,
            has_token,
            is_favorite: false,
            repository_key: "group/app".into(),
            number: 7,
            storage_key: "c|group/app|7".into(),
            revision: Some("head1".into()),
            head_sha: Some("head1".into()),
            target_sha: None,
            listed,
        }
    }

    #[test]
    fn the_bar_grows_logarithmically_and_fills_at_5000_lines() {
        assert_eq!(bar_fraction(0), 0.0);
        assert!((bar_fraction(5000) - 1.0).abs() < 1e-6);
        assert_eq!(bar_fraction(80_000), 1.0, "never more than the track");
        let one = bar_fraction(1);
        assert!(one > 0.05 && one < 0.1, "a one-line change still shows: {one}");
        assert!(bar_fraction(1552) > bar_fraction(50), "more lines, a longer bar");
        assert!((bar_fraction(1552) - 0.863).abs() < 0.01, "{}", bar_fraction(1552));
    }

    #[test]
    fn listed_counts_show_at_once_and_others_follow_the_platforms_plan() {
        let stats = DiffStats::default();
        let listed = DiffStat::new(10, 2, 3);
        assert_eq!(stats.shown(&row(ProviderKind::GitHub, true, listed)), Shown::Counted(listed));
        assert!(!stats.is_due(&row(ProviderKind::GitHub, true, listed)), "nothing to ask");
        assert_eq!(
            stats.shown(&row(ProviderKind::GitLab, true, DiffStat::default())),
            Shown::Counting,
            "a token batches on its own"
        );
        assert!(matches!(
            stats.shown(&row(ProviderKind::GitLab, false, DiffStat::default())),
            Shown::Uncounted { files: None, .. }
        ));
        assert!(matches!(
            stats.shown(&row(ProviderKind::GitHub, true, DiffStat::default())),
            Shown::Uncounted { .. }
        ));
        assert_eq!(
            stats.shown(&row(ProviderKind::AzureDevOps, false, DiffStat::default())),
            Shown::Counting,
            "a mirror may count for free"
        );
    }

    #[test]
    fn counts_are_kept_per_revision() {
        let mut stats = DiffStats::default();
        let counted = row(ProviderKind::GitLab, true, DiffStat::default());
        arrived(&mut stats, &counted, Arrival::Done(DiffStat::new(5, 1, 2)));
        assert_eq!(stats.shown(&counted), Shown::Counted(DiffStat::new(5, 1, 2)));
        assert!(!stats.is_due(&counted), "an unchanged pull request is not asked again");
        let pushed = StatRow { revision: Some("head2".into()), ..counted.clone() };
        assert_eq!(stats.shown(&pushed), Shown::Counting, "a new head never shows the old numbers");
        assert!(stats.is_due(&pushed));

        let files_only = StatRow { kind: ProviderKind::AzureDevOps, ..counted };
        let forty_files = DiffStat { lines: None, files: Some(40) };
        arrived(&mut stats, &files_only, Arrival::Skipped(forty_files));
        assert!(
            matches!(stats.shown(&files_only), Shown::Uncounted { files: Some(40), .. }),
            "too many files to count on its own"
        );
        assert!(!stats.is_due(&files_only), "a large pull request waits for Count lines");
        arrived(&mut stats, &files_only, Arrival::Failed("offline"));
        assert!(
            matches!(stats.shown(&files_only), Shown::Uncounted { why, .. } if why.contains("offline"))
        );
        assert!(!stats.is_due(&files_only), "a failure is not retried on its own");
    }

    /// Pushed while the old head was counted: the old head's answer leaves the new count running,
    /// so the row neither starts a second count nor offers "Count lines" meanwhile.
    #[test]
    fn a_count_of_the_old_head_leaves_the_new_heads_count_running() {
        let mut stats = DiffStats::default();
        let old = row(ProviderKind::GitLab, true, DiffStat::default());
        let pushed = StatRow { revision: Some("head2".into()), ..old.clone() };
        let rows =
            |row: &StatRow| vec![(row.storage_key.clone(), row.number, row.revision.clone())];
        stats.start(&[&old], true, 0.0, Load::Idle);
        stats.start(&[&pushed], true, 0.0, Load::Idle);
        stats.store(rows(&old), &[(7, DiffStat::new(5, 1, 2))], false, None);
        assert!(stats.is_running(&pushed));
        assert!(!stats.is_due(&pushed), "no second count for the new head");
        stats.store(rows(&pushed), &[(7, DiffStat::new(9, 3, 4))], false, None);
        assert_eq!(stats.shown(&pushed), Shown::Counted(DiffStat::new(9, 3, 4)));
    }

    fn favorite(kind: ProviderKind, has_token: bool) -> StatRow {
        StatRow { is_favorite: true, ..row(kind, has_token, DiffStat::default()) }
    }

    #[test]
    fn favorites_are_counted_on_their_own_whatever_their_size() {
        let mut stats = DiffStats::default();
        assert_eq!(
            stats.shown(&favorite(ProviderKind::GitLab, false)),
            Shown::Counting,
            "one request for the project, even without a token"
        );
        assert_eq!(plan(&favorite(ProviderKind::GitLab, false)), Plan::Batch);
        assert_eq!(
            stats.shown(&favorite(ProviderKind::GitHub, false)),
            Shown::Counting,
            "a REST row is asked for on its own"
        );
        assert_eq!(plan(&favorite(ProviderKind::GitHub, true)), Plan::PerPullRequest);
        assert_eq!(plan(&favorite(ProviderKind::AzureDevOps, true)), Plan::PerPullRequest);

        // Skipped as too large before the repository was starred: a favorite asks again, once.
        let large = favorite(ProviderKind::AzureDevOps, true);
        let forty_files = DiffStat { lines: None, files: Some(40) };
        arrived(&mut stats, &large, Arrival::Skipped(forty_files));
        assert!(stats.is_due(&large), "a favorite's lines are counted whatever the size");
        assert!(
            !stats.is_due(&StatRow { is_favorite: false, ..large.clone() }),
            "other rows keep Count lines"
        );
        arrived(&mut stats, &large, Arrival::Done(forty_files));
        assert!(!stats.is_due(&large), "a platform that reported no lines is not asked again");
    }

    #[test]
    fn favorites_never_take_every_automatic_count() {
        let mut stats = DiffStats::default();
        let other = row(ProviderKind::AzureDevOps, true, DiffStat::default());
        let starred = favorite(ProviderKind::AzureDevOps, true);
        let running = |is_automatic: bool, is_favorite: bool| Running {
            rows: Vec::new(),
            is_automatic,
            is_favorite,
            asked_at: 0.0,
            load: Load::Idle,
        };
        stats.running.extend([running(true, true), running(true, true)]);
        assert!(!stats.has_room(&starred), "favorites hold at most two counts");
        assert!(stats.has_room(&other), "the other cards keep theirs");
        stats.running.push(running(false, false));
        assert!(stats.has_room(&other), "a count the user asked for takes no automatic place");
        stats.running.extend([running(true, false), running(true, false)]);
        assert!(!stats.has_room(&other), "four automatic counts at most");
        stats.running.clear();
        stats.running.extend([running(true, false), running(true, false), running(true, false)]);
        assert!(stats.has_room(&starred), "a favorite still finds the last place");
    }
}
