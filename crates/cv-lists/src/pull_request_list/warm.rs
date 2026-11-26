//! Reads ahead the pages of the pull requests around the row the pointer rests on in the list
//! (`list_rows::dwell` tells which): that row and the one painted above and below it, each as deep
//! as an open of it is likely ([`Interest::depth`]):
//! - [`Depth::Meta`]: the detail (description, reviewers, all the header shows) and the web images
//!   its description shows ([`ImageStore::fetch_ahead`], kept in memory for a while only).
//! - [`Depth::Full`]: that, then the mirror with both commits and their merge base, and for small
//!   pull requests the counted file list ([`diff_stats::warm`]).
//!
//! The page a click then opens seeds its detail (and files) from the page cache and finds the
//! commits in the mirror. A detail still on its way when the click lands is not sent twice: the
//! page's identical request joins it in the HTTP layer (`review_core::http`, unless a write of
//! the user's landed meanwhile).
//!
//! Each row read costs its detail (two requests on GitHub, one on Azure DevOps, three on GitLab
//! plus its project), its images, and at full depth one git fetch and for small pull requests one
//! of their blobs, all spent on rows that may never be opened. So only rows of connections with a
//! token are read, while the window is active, at most [`READING`] at once: the rows of a rest
//! that find no free place wait, replaced by those of the next rest. A row is read once per head
//! commit and depth (a deeper read later adds the git part), and its detail not while it is fresh
//! in the page cache. Answers go in stamped with the time their request left, so they never
//! replace the newer detail the page put there itself (one after a merge, say; see
//! [`cv_runtime::cache::page_cache`]), and the user's writes the host may not show yet apply
//! to the detail first.

use std::collections::HashMap;
use std::sync::Arc;

use cv_base::platform::is_window_active;
use cv_pr_session::images::description_targets;
use cv_runtime::cache::page_cache::pull_request_part;
use cv_runtime::load::Load;
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use cv_services::diff_stats::{self, StatRow};
use cv_services::self_review::Identities;
use cv_services::writes::Answer;
use review_core::{PullRequestDetail, PullRequestState};
use review_git::ChangedFile;

use super::interest::{Depth, Interest};

/// Pull requests read at once: the rested row and its two neighbours.
pub const READING: usize = 3;

/// A painted row and how likely the user opens it.
#[derive(Debug, Clone)]
pub struct AheadRow {
    pub row: StatRow,
    pub interest: Interest,
}

/// What a read is waiting for.
enum Stage {
    /// Boxed: a detail is large, and most reads end with it.
    Detail(Box<Load<PullRequestDetail>>),
    Git(Load<Option<Arc<Vec<ChangedFile>>>>),
}

/// One pull request being read ahead, and when its detail was asked for ([`Services::now`]).
struct Reading {
    row: StatRow,
    depth: Depth,
    asked_at: f64,
    stage: Stage,
}

#[derive(Default)]
pub struct Warming {
    reading: Vec<Reading>,
    /// Rows of the last rest that found no free place, likeliest first.
    waiting: Vec<(StatRow, Depth)>,
    /// The revision and depth each row was read to, by storage key.
    read: HashMap<String, (Option<String>, Depth)>,
}

impl Warming {
    /// The pointer rests on `rows[rested]`, if anywhere: reads it and its neighbours as deep as
    /// their interest asks, or lets them wait for a free place.
    pub fn rest(&mut self, services: &mut Services, rows: &[AheadRow], rested: Option<usize>) {
        let is_active = services.background.ctx().input(|input| is_window_active(input.viewport()));
        let Some(rested) = rested.filter(|_| is_active) else {
            return;
        };
        // Who the user is decides how likely they open a row: known from the next rest on. Not
        // asked before a rest, so a list alone costs no request for it.
        if let Some(ahead) = rows.get(rested) {
            Identities::of(&services.background.ctx()).ask(&ahead.row.connection_id);
        }
        let around = [Some(rested), rested.checked_sub(1), rested.checked_add(1)];
        let planned: Vec<(StatRow, Depth)> = around
            .into_iter()
            .flatten()
            .filter_map(|place| rows.get(place).map(|ahead| (place, ahead)))
            .map(|(place, ahead)| (ahead.row.clone(), ahead.interest.depth(place == rested)))
            .filter(|(row, depth)| self.is_wanted(services, row, *depth))
            .collect();
        self.waiting = planned;
        self.start_waiting(services);
    }

    /// Moves finished reads into the page cache (and their counts into the size column), starts
    /// the git part of a read that goes that deep, then the rows that waited.
    pub fn poll(&mut self, services: &mut Services) {
        let mut index = 0;
        while index < self.reading.len() {
            let reading = &mut self.reading[index];
            let is_done = match &mut reading.stage {
                Stage::Detail(load) => load.poll(),
                Stage::Git(load) => load.poll(),
            };
            if !is_done {
                index += 1;
                continue;
            }
            let reading = self.reading.swap_remove(index);
            if let Some(next) = self.land(services, reading) {
                self.reading.push(next);
            }
        }
        self.start_waiting(services);
    }

    /// Whether `row` is to be read to `depth`: not read that deep for its head yet, not read now,
    /// and not prepared by a count of its lines.
    fn is_wanted(&self, services: &Services, row: &StatRow, depth: Depth) -> bool {
        let is_read = self
            .read
            .get(&row.storage_key)
            .is_some_and(|(revision, read)| *revision == row.revision && *read >= depth);
        let is_reading =
            self.reading.iter().any(|reading| reading.row.storage_key == row.storage_key);
        depth > Depth::Nothing
            && row.has_token
            && !is_read
            && !is_reading
            // A count of the row prepares its page already.
            && !services.diff_stats.is_running(row)
    }

    fn start_waiting(&mut self, services: &mut Services) {
        while self.reading.len() < READING && !self.waiting.is_empty() {
            let (row, depth) = self.waiting.remove(0);
            let revision = (row.storage_key.clone(), row.revision.clone());
            match start(services, row, depth) {
                Some(reading) => self.reading.push(reading),
                // Read from the page cache at once (or its connection is gone).
                None => {
                    self.read.insert(revision.0, (revision.1, depth));
                }
            }
        }
    }

    /// Puts what `reading` brought where the page finds it; returns the read's git part when it
    /// goes that deep and its detail just came. A failure is not retried by resting again: the
    /// page the click opens shows it, with Retry.
    fn land(&mut self, services: &mut Services, reading: Reading) -> Option<Reading> {
        let Reading { row, depth, asked_at, stage } = reading;
        let key = key_of(&row);
        let now = services.now();
        match stage {
            Stage::Detail(load) => {
                if let Load::Ready(mut detail) = *load {
                    let answer = Answer::Host { asked_at };
                    services.writes.settle_detail(&key, &mut detail, answer, now);
                    let part = pull_request_part(&row.storage_key, "detail");
                    services.cache.put_answer(&part, detail.clone(), asked_at, now);
                    read_images(services, &row, &detail);
                    if depth == Depth::Full && detail.summary.state == PullRequestState::Open {
                        return start_git(services, row, depth, asked_at, detail);
                    }
                }
            }
            Stage::Git(Load::Ready(Some(files))) => {
                services.diff_stats.counted(&row, &files);
                let part = pull_request_part(&row.storage_key, "files");
                services.cache.put_answer(&part, files, asked_at, now);
            }
            Stage::Git(_) => {}
        }
        self.read.insert(row.storage_key.clone(), (row.revision.clone(), depth));
        None
    }
}

/// Starts reading `row` to `depth`: its detail, or, while the page cache has it fresh, its images
/// and the git part straight away. `None` when nothing is left to wait for.
fn start(services: &mut Services, row: StatRow, depth: Depth) -> Option<Reading> {
    let now = services.now();
    let part = pull_request_part(&row.storage_key, "detail");
    let cached: Option<PullRequestDetail> =
        if services.cache.is_stale(&part, now) { None } else { services.cache.get(&part) };
    if let Some(detail) = cached {
        read_images(services, &row, &detail);
        let is_open = detail.summary.state == PullRequestState::Open;
        return (depth == Depth::Full && is_open)
            .then(|| start_git(services, row, depth, now, detail))
            .flatten();
    }
    let provider = services.provider(&row.connection_id)?;
    let (repository, number) = (row.repository_key.clone(), row.number);
    let mut load = Load::Idle;
    load.start(
        &services.background,
        async move { provider.pull_request(&repository, number).await },
    );
    Some(Reading { row, depth, asked_at: now, stage: Stage::Detail(Box::new(load)) })
}

/// The git part of a read: the commits of `detail` in the mirror, and a small pull request's
/// counted files.
fn start_git(
    services: &mut Services,
    row: StatRow,
    depth: Depth,
    asked_at: f64,
    detail: PullRequestDetail,
) -> Option<Reading> {
    let provider = services.provider(&row.connection_id)?;
    let mirrors = services.mirrors.clone();
    let task_row = row.clone();
    let mut load = Load::Idle;
    load.start(&services.background, async move {
        diff_stats::warm(detail, provider.as_ref(), &mirrors, &task_row).await
    });
    Some(Reading { row, depth, asked_at, stage: Stage::Git(load) })
}

/// Reads the web images of `detail`'s description ahead.
fn read_images(services: &mut Services, row: &StatRow, detail: &PullRequestDetail) {
    let Some(provider) = services.provider(&row.connection_id) else {
        return;
    };
    let targets =
        description_targets(services, &key_of(row), row.kind, provider, &detail.description);
    services.images.fetch_ahead(services.now(), targets);
}

fn key_of(row: &StatRow) -> PullRequestKey {
    PullRequestKey {
        connection_id: row.connection_id.clone(),
        repository_key: row.repository_key.clone(),
        number: row.number,
    }
}
