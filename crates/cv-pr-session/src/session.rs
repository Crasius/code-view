mod repository;

use std::collections::HashMap;
use std::sync::Arc;

use cv_runtime::cache::page_cache::pull_request_part as cache_key;
use cv_runtime::links::link_target::Here;
use cv_runtime::load::{Live, Load};
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use cv_services::connection::providers::unknown_connection;
use cv_services::mirrors::MirrorRegistry;
use cv_services::writes::Answer;
use review_core::PullRequestState;
use review_core::{
    Capabilities, PipelineJob, PipelineRun, Provider, ProviderError, ProviderKind,
    PullRequestDetail, PullRequestSummary, ReviewThread, RunStatus, Timestamp,
};
use review_git::{ChangedFile, CommitInfo, GitMirror};
use review_store::retention::{Lease, MirrorUse};

use self::repository::Repository;
use crate::prepare_progress::{SharedProgress, Step};

/// Which commits the Files tab compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareRange {
    /// Merge base of target and head against head: what the pull request changes.
    AllChanges,
    /// One commit against its first parent.
    Commit { sha: String, parent: String },
    /// What was pushed since the user last looked: the head they saw against the new head.
    SinceSeen { seen: String, head: String },
    /// What the pull request changed before the latest push came in.
    Previous { base: String, head: String },
}

/// Hands the list row a pull request is opened from to its page: called before the navigation, so
/// the page that opens at the end of the frame reads it from the page cache.
pub fn hand_over_row(services: &mut Services, key: &PullRequestKey, row: PullRequestSummary) {
    let now = services.now();
    services.cache.put(&cache_key(&key.storage_key(), "summary"), row, now);
}

#[derive(Clone)]
pub struct Mirror {
    pub git: Arc<GitMirror>,
    pub merge_base: String,
    pub head: String,
}

/// Data shared by all tabs of one pull request.
///
/// Stale-while-revalidate: every part keeps its last answer on screen while a refresh runs, and a
/// page opened again (even after it was dropped) starts from the page cache, and a detail fresh
/// there (a warm-up from the list) is not asked for again. Git data is rebuilt
/// only when a refreshed detail names other commits than the ones the mirror was prepared for.
///
/// The user's writes ([`cv_services::writes`]) show in the detail and the threads in the frame
/// they succeed ([`Self::catch_up_writes`]), and every detail and thread list that arrives gets
/// those the host may not show yet applied before it is shown or remembered.
pub struct PullRequestSession {
    pub key: PullRequestKey,
    pub kind: ProviderKind,
    pub has_token: bool,
    pub provider: Arc<dyn Provider>,
    pub capabilities: Capabilities,
    pub detail: Live<PullRequestDetail>,
    /// The list row the page was opened from, while no detail is known: the header shows its
    /// title, number and branches, never as final (detail-only parts keep their skeletons).
    pub listed: Option<PullRequestSummary>,
    pub threads: Live<Vec<ReviewThread>>,
    /// Pipeline runs and status checks (Overview checks and Pipelines tab): the head commit's, and
    /// older runs where the platform lists them.
    pub runs: Live<Vec<PipelineRun>>,
    /// Stages (with their jobs) per run id, loaded on demand: the head runs' for the stage bars of
    /// the checks table and the overview, an older run's once it is opened.
    stages: HashMap<String, StageLoad>,
    /// Earlier attempts the platform lists by id only, by (run id, attempt target), loaded when
    /// the user opens one. They never change, so each loads once.
    attempts: HashMap<(String, String), Live<PipelineJob>>,
    /// The remote and the mirror, opened with the page; [`Self::mirror`] fetches into it.
    repository: Repository,
    pub mirror: Load<Mirror>,
    /// (target, head) commits the mirror was prepared for.
    mirrored: Option<(String, String)>,
    pub range: CompareRange,
    pub files_fast: Live<Arc<Vec<ChangedFile>>>,
    pub files_detailed: Live<Arc<Vec<ChangedFile>>>,
    pub commits: Live<Vec<CommitInfo>>,
    pub progress: SharedProgress,
    /// Bumped whenever the pull request was refreshed, so tabs can drop stale derived state.
    pub generation: u64,
    /// The comparison the Files tab showed before a live update moved the commits.
    previous: Option<CompareRange>,
    /// When the detail (resp. threads) on its way, or the last one, was asked for.
    detail_asked_at: f64,
    threads_asked_at: f64,
    /// The last write of the user's the detail and threads show; seeded ones catch up at once.
    writes_seen: u64,
    /// Keeps retention off this pull request's data (and its drafts) while the page lives.
    _open: Lease<String>,
}

impl PullRequestSession {
    pub fn open(key: PullRequestKey, services: &mut Services) -> Result<Self, ProviderError> {
        let connection = services
            .connection(&key.connection_id)
            .cloned()
            .ok_or_else(|| unknown_connection(&key.connection_id))?;
        let provider = services
            .provider(&key.connection_id)
            .ok_or_else(|| unknown_connection(&key.connection_id))?;
        let capabilities = provider.capabilities().clone();
        let now = services.now();
        let cache = &mut services.cache;
        let storage_key = key.storage_key();
        let part = |name: &str| cache_key(&storage_key, name);
        let detail: Option<PullRequestDetail> = cache.get(&part("detail"));
        let is_detail_fresh = !cache.is_stale(&part("detail"), now);
        let listed = if detail.is_none() { cache.get(&part("summary")) } else { None };
        let mut session = Self {
            kind: connection.kind,
            has_token: connection.token().is_some(),
            provider,
            capabilities,
            detail: Live::seeded(detail),
            listed,
            threads: Live::seeded(cache.get(&part("threads"))),
            runs: Live::seeded(cache.get(&part("runs"))),
            stages: HashMap::new(),
            attempts: HashMap::new(),
            repository: Repository::default(),
            mirror: Load::Idle,
            mirrored: None,
            range: CompareRange::AllChanges,
            files_fast: Live::seeded(cache.get(&part("files"))),
            files_detailed: Live::seeded(cache.get(&part("files"))),
            commits: Live::seeded(cache.get(&part("commits"))),
            progress: SharedProgress::default(),
            generation: 0,
            previous: None,
            detail_asked_at: f64::NEG_INFINITY,
            threads_asked_at: f64::NEG_INFINITY,
            writes_seen: 0,
            _open: services.retention.hold_pull_request(&storage_key),
            key,
        };
        session.open_repository(services);
        if is_detail_fresh {
            // Answered moments ago (the list warmed it on a resting pointer, or the page was just
            // closed): not asked again, as a revisit would not ask.
            session.reload_threads(services);
            session.reload_runs(services);
            session.generation += 1;
        } else {
            session.refresh(services);
        }
        Ok(session)
    }

    /// Resolves the remote and opens its mirror in the background, alongside the detail request:
    /// neither needs the pull request's commits, so only the fetch waits for the detail.
    fn open_repository(&mut self, services: &Services) {
        self.repository = Repository::default();
        let repository = self.repository.clone();
        let provider = self.provider.clone();
        let mirrors = services.mirrors.clone();
        let repository_key = self.key.repository_key.clone();
        services.background.spawner().spawn(async move {
            // A failure stays in `repository`; the preparation reports it once the detail is in.
            if let Ok(remote) = repository.remote(provider.as_ref(), &repository_key).await {
                let _reported_by_the_preparation = repository.mirror(&mirrors, &remote).await;
            }
        });
    }

    /// The page is shown again: its parts stay on screen and refresh in the background when stale.
    pub fn revisit(&mut self, services: &Services) {
        let detail_key = cache_key(&self.key.storage_key(), "detail");
        if !self.detail.is_loading() && services.cache.is_stale(&detail_key, services.now()) {
            self.refresh(services);
        }
    }

    /// The repository's clone URL, once the platform named it.
    pub fn clone_url(&self) -> Option<String> {
        self.repository.clone_url()
    }

    /// The repository this pull request's shorthand references (`#12`, `!12`) are relative to.
    pub fn here(&self) -> Here {
        Here {
            connection_id: self.key.connection_id.clone(),
            kind: self.kind,
            repository_key: self.key.repository_key.clone(),
        }
    }

    /// Reloads detail, threads and runs; git data follows when the head moved.
    pub fn refresh(&mut self, services: &Services) {
        let is_git_failed = self.mirror.error().is_some() || self.files_fast.error().is_some();
        if is_git_failed {
            // A failed preparation starts over (Retry), not only once the head moves.
            self.mirror = Load::Idle;
            self.mirrored = None;
            self.files_fast.clear();
            self.files_detailed.clear();
            self.open_repository(services);
        }
        if is_git_failed || self.detail.error().is_some() {
            self.progress.reset();
        }
        if let Some(mirror) = commits_to_list_again(self.mirror.ready(), self.commits.error()) {
            self.start_commits(services, &mirror);
        }
        let provider = self.provider.clone();
        let repository = self.key.repository_key.clone();
        let number = self.key.number;
        self.detail_asked_at = services.now();
        self.detail.start(&services.background, async move {
            provider.pull_request(&repository, number).await
        });
        self.reload_threads(services);
        self.reload_runs(services);
        self.generation += 1;
    }

    pub fn reload_threads(&mut self, services: &Services) {
        let provider = self.provider.clone();
        let repository = self.key.repository_key.clone();
        let number = self.key.number;
        self.threads_asked_at = services.now();
        self.threads.start(&services.background, async move {
            provider.threads(&repository, number).await
        });
    }

    pub fn reload_runs(&mut self, services: &Services) {
        let provider = self.provider.clone();
        let repository = self.key.repository_key.clone();
        let number = self.key.number;
        self.runs.start(&services.background, async move {
            provider.pipeline_runs(&repository, number).await
        });
    }

    /// Reloads the runs while one of them is still going and no reload runs already.
    pub fn refresh_running_runs(&mut self, services: &Services) {
        let is_running =
            self.runs.ready().is_some_and(|runs| runs.iter().any(|run| !run.status.is_finished()));
        if is_running && !self.runs.is_loading() {
            self.reload_runs(services);
        }
    }

    /// The stages of a run, once asked for with [`Self::want_stages`].
    pub fn stages(&self, run_id: &str) -> Option<&Live<Vec<PipelineJob>>> {
        self.stages.get(run_id).map(|load| &load.live)
    }

    /// Loads the stages of `run` unless they were loaded for its current state already; a run
    /// that changed state since (finished, or was rerun) loads them again behind the old answer.
    pub fn want_stages(&mut self, run: &PipelineRun, services: &Services) {
        if !run.has_jobs {
            return;
        }
        let state = (run.status, run.finished_at);
        let is_current = self
            .stages
            .get(&run.id)
            .is_some_and(|load| load.live.is_loading() || load.loaded_for == state);
        if !is_current {
            self.start_stages(&run.id, state, services);
        }
    }

    /// Loads the stages of the head runs ([`PipelineRun::is_head`]), as [`Self::want_stages`] does.
    /// Older runs cost a few requests each (rate limits of an hour without a token), so theirs load
    /// only once the user opens one.
    pub fn want_head_stages(&mut self, services: &Services) {
        let Some(runs) = self.runs.ready().cloned() else {
            return;
        };
        for run in runs.iter().filter(|run| run.is_head) {
            self.want_stages(run, services);
        }
    }

    /// True when every head run's stages are there or failed to load: the checks table waits for
    /// them, so their job rows never shift in later.
    pub fn are_head_stages_settled(&self) -> bool {
        self.runs.ready().into_iter().flatten().filter(|run| run.has_jobs && run.is_head).all(
            |run| {
                self.stages(&run.id)
                    .is_some_and(|load| load.ready().is_some() || load.error().is_some())
            },
        )
    }

    /// Loads the stages of `run_id` again, whatever is on screen (after a rerun, or Retry).
    pub fn reload_stages(&mut self, run_id: &str, services: &Services) {
        let state = self
            .runs
            .ready()
            .and_then(|runs| runs.iter().find(|run| run.id == run_id))
            .map_or((RunStatus::Queued, None), |run| (run.status, run.finished_at));
        self.start_stages(run_id, state, services);
    }

    /// The live refresh: stages already shown of runs still going load again.
    pub fn reload_running_stages(&mut self, services: &Services) {
        let running: Vec<(String, (RunStatus, Option<Timestamp>))> = self
            .runs
            .ready()
            .into_iter()
            .flatten()
            .filter(|run| !run.status.is_finished())
            .filter(|run| self.stages.get(&run.id).is_some_and(|load| !load.live.is_loading()))
            .map(|run| (run.id.clone(), (run.status, run.finished_at)))
            .collect();
        for (run_id, state) in running {
            self.start_stages(&run_id, state, services);
        }
    }

    fn start_stages(
        &mut self,
        run_id: &str,
        state: (RunStatus, Option<Timestamp>),
        services: &Services,
    ) {
        let provider = self.provider.clone();
        let repository = self.key.repository_key.clone();
        let number = self.key.number;
        let run = run_id.to_owned();
        let load = self
            .stages
            .entry(run_id.to_owned())
            .or_insert_with(|| StageLoad { live: Live::default(), loaded_for: state });
        load.loaded_for = state;
        load.live.start(&services.background, async move {
            provider.pipeline_jobs(&repository, number, &run).await
        });
    }

    /// An earlier attempt of `run_id` listed by id only, once asked for with [`Self::want_attempt`].
    pub fn attempt(&self, run_id: &str, target: &str) -> Option<&Live<PipelineJob>> {
        self.attempts.get(&(run_id.to_owned(), target.to_owned()))
    }

    /// The earlier attempts of `run_id` loaded so far, by target.
    pub fn loaded_attempts(&self, run_id: &str) -> Vec<(&str, &PipelineJob)> {
        self.attempts
            .iter()
            .filter(|((run, _), _)| run == run_id)
            .filter_map(|((_, target), live)| live.ready().map(|job| (target.as_str(), job)))
            .collect()
    }

    /// Loads an earlier attempt the user opened, once; a failed load starts again (Retry).
    pub fn want_attempt(&mut self, run_id: &str, target: &str, services: &Services) {
        let key = (run_id.to_owned(), target.to_owned());
        let live = self.attempts.entry(key).or_default();
        if live.ready().is_some() || live.is_loading() {
            return;
        }
        let provider = self.provider.clone();
        let repository = self.key.repository_key.clone();
        let number = self.key.number;
        let (run, target) = (run_id.to_owned(), target.to_owned());
        live.start(&services.background, async move {
            provider.pipeline_attempt(&repository, number, &run, &target).await
        });
    }

    /// Advances the load pipeline: detail (while the mirror opens) → the pull request's commits in
    /// the mirror → file list (fast, then detailed) and commits.
    /// Every answer that arrives is also remembered in the page cache, the detail and the threads
    /// with the user's writes the host may not show yet applied.
    pub fn poll(&mut self, services: &mut Services) {
        let now = services.now();
        let storage_key = self.key.storage_key();
        let (writes, key) = (&mut services.writes, &self.key);
        let answer = Answer::Host { asked_at: self.detail_asked_at };
        if self.detail.poll()
            && let Some(detail) =
                settled(&mut self.detail, |detail| writes.settle_detail(key, detail, answer, now))
        {
            if detail.summary.state != PullRequestState::Open {
                // Cleaned up once this page closes.
                services.retention.pull_request_finished(&self.key.connection_id, &storage_key);
            }
            let part = cache_key(&storage_key, "detail");
            services.cache.put_answer(&part, detail, self.detail_asked_at, now);
        }
        let (writes, key) = (&mut services.writes, &self.key);
        let answer = Answer::Host { asked_at: self.threads_asked_at };
        if self.threads.poll()
            && let Some(threads) = settled(&mut self.threads, |threads| {
                writes.settle_threads(key, threads, answer, now)
            })
        {
            let part = cache_key(&storage_key, "threads");
            services.cache.put_answer(&part, threads, self.threads_asked_at, now);
        }
        self.catch_up_writes(services);
        let cache = &mut services.cache;
        if self.runs.poll()
            && let Some(runs) = self.runs.ready()
        {
            forget_unlisted(&mut self.stages, &mut self.attempts, runs);
            cache.put(&cache_key(&storage_key, "runs"), runs.clone(), now);
        }
        for load in self.stages.values_mut() {
            load.live.poll();
        }
        for live in self.attempts.values_mut() {
            live.poll();
        }
        if self.mirror.poll()
            && let Some(mirror) = self.mirror.ready().cloned()
        {
            self.progress.enter(Step::ListingFiles);
            self.start_file_lists(services, &mirror);
            self.start_commits(services, &mirror);
        }
        let cache = &mut services.cache;
        let is_all_changes = self.range == CompareRange::AllChanges;
        self.files_fast.poll();
        // Counts on the way to the result are not remembered: only the finished list is.
        if self.files_detailed.poll()
            && !self.files_detailed.is_loading()
            && is_all_changes
            && let Some(files) = self.files_detailed.ready()
        {
            cache.put(&cache_key(&storage_key, "files"), files.clone(), now);
        }
        if self.commits.poll()
            && let Some(commits) = self.commits.ready()
        {
            cache.put(&cache_key(&storage_key, "commits"), commits.clone(), now);
        }

        let wanted =
            self.detail.ready().map(|detail| (detail.target_sha.clone(), detail.head_sha.clone()));
        if wanted.is_some()
            && wanted != self.mirrored
            && !self.mirror.is_loading()
            && let Some(detail) = self.detail.ready().cloned()
        {
            self.mirrored = wanted;
            self.progress.restart();
            self.start_mirror(services, detail);
        }
    }

    /// Shows the user's writes logged since the detail and threads last caught up, in place; the
    /// page cache had them applied when they were logged.
    pub fn catch_up_writes(&mut self, services: &mut Services) {
        if self.writes_seen == services.writes.last() {
            return;
        }
        self.writes_seen = services.writes.last();
        let (now, writes, key) = (services.now(), &mut services.writes, &self.key);
        settled(&mut self.detail, |detail| writes.settle_detail(key, detail, Answer::Shown, now));
        settled(&mut self.threads, |threads| {
            writes.settle_threads(key, threads, Answer::Shown, now)
        });
    }

    /// Prepares the mirror for `detail`'s commits; the remote and the mirror itself are the ones
    /// [`Self::open_repository`] started with the page, waited for if still on the way. Every step
    /// and finding goes into the progress log, which the Files tab shows (and repaints) while it
    /// waits.
    fn start_mirror(&mut self, services: &Services, detail: PullRequestDetail) {
        let provider = self.provider.clone();
        let mirrors = services.mirrors.clone();
        let repository = self.repository.clone();
        let repository_key = self.key.repository_key.clone();
        let connection_id = self.key.connection_id.clone();
        let storage_key = self.key.storage_key();
        let retention = services.retention.clone();
        let progress = self.progress.clone();
        progress.log(&format!(
            "Head {} into target {}",
            short_sha(&detail.head_sha),
            short_sha(&detail.target_sha)
        ));
        self.mirror.start(&services.background, async move {
            progress.enter(Step::ResolvingRemote);
            let remote = repository.remote(provider.as_ref(), &repository_key).await?;
            progress.log(&format!("Remote {}", remote.clone_url));
            progress.enter(Step::OpeningMirror);
            let local_refs: Vec<String> =
                detail.git_fetch_refs.iter().map(|name| review_git::local_ref_name(name)).collect();
            let usage = MirrorUse {
                connection_id: &connection_id,
                pull_request: &storage_key,
                local_refs: &local_refs,
            };
            let git = repository.mirror(&mirrors, &remote).await?;
            MirrorRegistry::record_use(&git, &usage);
            progress.log(&format!("Mirror {}", git.git_dir().display()));
            progress.enter(Step::Fetching);
            let sink_progress = progress.clone();
            let sink = move |event: review_git::GitEvent| sink_progress.report_git(event);
            let gone = git
                .ensure_commits(
                    &detail.git_fetch_refs,
                    &[&detail.target_sha, &detail.head_sha],
                    &sink,
                )
                .await?;
            if is_source_gone(&gone, &detail.summary.source_branch) {
                retention.pull_request_finished(&connection_id, &storage_key);
            }
            progress.enter(Step::FindingMergeBase);
            let merge_base = git.merge_base(&detail.target_sha, &detail.head_sha).await?;
            progress.log(&format!("Merge base {}", short_sha(&merge_base)));
            Ok(Mirror { git, merge_base, head: detail.head_sha.clone() })
        });
    }

    /// Puts a live update that was read ahead on screen at once: detail and threads are replaced
    /// (with the user's writes the host may not show yet applied), git data switches to the mirror
    /// prepared for the new commits, and the file lists and commits follow, each keeping its older
    /// value on screen until the newer one is there.
    pub fn apply_update(
        &mut self,
        services: &mut Services,
        mut detail: PullRequestDetail,
        mut threads: Vec<ReviewThread>,
        mirror: Option<Mirror>,
    ) {
        let storage_key = self.key.storage_key();
        let now = services.now();
        // Read ahead moments ago; when exactly is the live update's to know.
        let answer = Answer::Host { asked_at: now };
        services.writes.settle_detail(&self.key, &mut detail, answer, now);
        services.writes.settle_threads(&self.key, &mut threads, answer, now);
        services.cache.put(&cache_key(&storage_key, "detail"), detail.clone(), now);
        services.cache.put(&cache_key(&storage_key, "threads"), threads.clone(), now);
        if let Some(mirror) = mirror {
            if let Some(shown) = self.mirror.ready() {
                self.previous = Some(CompareRange::Previous {
                    base: shown.merge_base.clone(),
                    head: shown.head.clone(),
                });
            }
            self.mirrored = Some((detail.target_sha.clone(), detail.head_sha.clone()));
            self.mirror = Load::Ready(mirror.clone());
            self.start_file_lists(services, &mirror);
            self.start_commits(services, &mirror);
        }
        self.detail.set(detail);
        self.threads.set(threads);
        self.reload_runs(services);
        self.generation += 1;
    }

    /// The comparison of the version before the latest live update, while there is one.
    pub fn previous_version(&self) -> Option<CompareRange> {
        self.previous.clone()
    }

    /// (target, head) commits the mirror was prepared for, so a live update can tell whether the
    /// git data has to move.
    pub fn mirrored(&self) -> Option<&(String, String)> {
        self.mirrored.as_ref()
    }

    pub fn set_range(&mut self, services: &Services, range: CompareRange) {
        if self.range == range {
            return;
        }
        self.range = range;
        if let Some(mirror) = self.mirror.ready().cloned() {
            // Another comparison: its files replace the shown ones, which are not its files.
            self.files_fast.clear();
            self.files_detailed.clear();
            self.start_file_lists(services, &mirror);
        }
    }

    /// (base, head) commits of the current comparison.
    pub fn compared_commits(&self) -> Option<(String, String)> {
        let mirror = self.mirror.ready()?;
        let commits = match &self.range {
            CompareRange::AllChanges => (mirror.merge_base.clone(), mirror.head.clone()),
            CompareRange::Commit { sha, parent } => (parent.clone(), sha.clone()),
            CompareRange::SinceSeen { seen, head } => (seen.clone(), head.clone()),
            CompareRange::Previous { base, head } => (base.clone(), head.clone()),
        };
        Some(commits)
    }

    fn start_file_lists(&mut self, services: &Services, mirror: &Mirror) {
        let Some((base, head)) = self.compared_commits() else {
            return;
        };
        let fast_git = mirror.git.clone();
        let (fast_base, fast_head) = (base.clone(), head.clone());
        self.files_fast.start(&services.background, async move {
            let files = fast_git.changed_files_fast(&fast_base, &fast_head).await?;
            Ok(Arc::new(files))
        });
        let detailed_git = mirror.git.clone();
        self.files_detailed.start_streaming(&services.background, move |counted| async move {
            let publish = move |files: &[ChangedFile]| counted.publish(Arc::new(files.to_vec()));
            let files = detailed_git.changed_files_detailed(&base, &head, &publish).await?;
            Ok(Arc::new(files))
        });
    }

    fn start_commits(&mut self, services: &Services, mirror: &Mirror) {
        let git = mirror.git.clone();
        let (base, head) = (mirror.merge_base.clone(), mirror.head.clone());
        self.commits.start(&services.background, async move { git.commits(&base, &head).await });
    }

    /// The best file list available right now: detailed (renames, counts) once ready, else the fast
    /// one. While lines are counted, the detailed list has the counts so far and no renames yet.
    pub fn files(&self) -> Option<&Arc<Vec<ChangedFile>>> {
        self.files_detailed.ready().or_else(|| self.files_fast.ready())
    }

    pub fn is_counting_lines(&self) -> bool {
        self.files_detailed.is_loading()
    }

    /// A refresh runs behind data already on screen.
    pub fn is_refreshing(&self) -> bool {
        self.detail.is_refreshing() || self.threads.is_refreshing()
    }

    /// The first failed refresh whose older data stayed on screen.
    pub fn refresh_error(&self) -> Option<&str> {
        self.detail.refresh_error().or_else(|| self.threads.refresh_error())
    }
}

/// The value `live` shows after `settle` changed it in place (true when it did), which then
/// replaces the shown one.
fn settled<T: Clone>(live: &mut Live<T>, settle: impl FnOnce(&mut T) -> bool) -> Option<T> {
    let mut value = live.ready()?.clone();
    if settle(&mut value) {
        live.set(value.clone());
    }
    Some(value)
}

/// The mirror to list the commits from again when only their listing failed: the mirror is fine,
/// so nothing else would start it before the head moves (the Commits tab's Retry).
fn commits_to_list_again<M: Clone>(mirror: Option<&M>, listing_error: Option<&str>) -> Option<M> {
    listing_error.and(mirror).cloned()
}

/// Drops the stages and attempts of runs that `runs` no longer lists. Such a run is shown nowhere
/// (an open one gives way to the checks table), and a pipeline polled for hours lists a new run
/// every few minutes, each run's stages about 28 KB.
fn forget_unlisted<S, A>(
    stages: &mut HashMap<String, S>,
    attempts: &mut HashMap<(String, String), A>,
    runs: &[PipelineRun],
) {
    let is_listed = |run_id: &str| runs.iter().any(|run| run.id == run_id);
    stages.retain(|run_id, _| is_listed(run_id));
    attempts.retain(|(run_id, _), _| is_listed(run_id));
}

/// One run's stages and the run state they were asked for.
struct StageLoad {
    live: Live<Vec<PipelineJob>>,
    loaded_for: (RunStatus, Option<Timestamp>),
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// The fetch found the pull request's source branch deleted on the remote (only platforms whose
/// fetch refs include the branch itself can tell; the others report it through the state).
fn is_source_gone(gone: &[String], source_branch: &str) -> bool {
    let branch = source_branch.trim_start_matches("refs/heads/");
    gone.iter().any(|name| name.strip_prefix("refs/heads/") == Some(branch))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::commits_to_list_again;

    #[test]
    fn retry_lists_the_commits_again_when_only_their_listing_failed() {
        assert_eq!(commits_to_list_again(Some(&"mirror"), Some("bad revision")), Some("mirror"));
        assert_eq!(
            commits_to_list_again::<&str>(None, Some("bad revision")),
            None,
            "the mirror lists them"
        );
        assert_eq!(commits_to_list_again(Some(&"mirror"), None), None, "nothing failed");
    }

    /// A pipeline polled for hours lists a new run every few minutes: the stages and attempts of
    /// the runs it stopped listing go, the listed runs' stay.
    #[test]
    fn stages_of_runs_no_longer_listed_are_forgotten() {
        let run = |id: &str| review_core::PipelineRun {
            id: id.to_owned(),
            name: format!("Pipeline #{id}"),
            status: review_core::RunStatus::Running,
            description: None,
            web_url: None,
            started_at: None,
            finished_at: None,
            has_jobs: true,
            is_head: true,
            reruns: Vec::new(),
        };
        let mut stages: HashMap<String, u8> =
            ["1001", "1002", "1003"].map(|id| (id.to_owned(), 0)).into();
        let attempts_of = |id: &str| ((id.to_owned(), "job-7".to_owned()), 0);
        let mut attempts: HashMap<(String, String), u8> =
            [attempts_of("1001"), attempts_of("1003")].into();
        super::forget_unlisted(&mut stages, &mut attempts, &[run("1003"), run("1002")]);
        let mut kept: Vec<&str> = stages.keys().map(String::as_str).collect();
        kept.sort_unstable();
        assert_eq!(kept, ["1002", "1003"]);
        assert_eq!(attempts.keys().collect::<Vec<_>>(), [&("1003".to_owned(), "job-7".to_owned())]);
    }

    #[test]
    fn a_deleted_source_branch_counts_as_gone_but_a_missing_merge_ref_does_not() {
        let gone = vec!["refs/heads/feature/x".to_owned()];
        assert!(super::is_source_gone(&gone, "feature/x"));
        assert!(super::is_source_gone(&gone, "refs/heads/feature/x"));
        assert!(!super::is_source_gone(&["refs/pull/22/merge".to_owned()], "feature/x"));
        assert!(!super::is_source_gone(&gone, "feature/y"));
    }
}
