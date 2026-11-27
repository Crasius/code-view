//! Phase 2 of the file list: every changed file's line counts and whitespace-only mark, and the
//! renames, published file by file while they are worked out.
//!
//! Counting reads both blobs of every changed file, and the mirror is blob-less. One
//! `git diff-tree --numstat` over the comparison downloads every missing blob in one batch and
//! prints nothing before all are there (1,504 files: 17 s). So the files count in chunks:
//! - Files whose blobs are local already (the user opened them, the folder summary showed them,
//!   an earlier count read them) count first, without waiting for a download. Which ones they are
//!   `git rev-list --missing=print` tells without fetching. While chunks download, every file not
//!   counted yet is checked again every [`RECHECK`], also one whose chunk is on its way, so a file
//!   the user opens meanwhile counts soon after its diff brought its blobs. Its chunk counts it
//!   again later, to the same numbers. A chunk whose files are all local has its download behind
//!   it and is counting already, so its files are left to it.
//! - The rest count in chunks in the order a file tree lists them ([`tree_order`]), [`PARALLEL`]
//!   at a time. The first chunk is small ([`FIRST_CHUNK`]) so counts show early; each next one is
//!   twice as large, up to [`MAX_CHUNK`] files or [`MAX_PATH_BYTES`] of paths on its command
//!   line. A chunk is one `git diff-tree --numstat -- <paths>`, which downloads exactly its files'
//!   blobs in one request before it counts them, then the same offline with `-w`.
//! - Files the user is near count first ([`Wanted`]): the Files tab and the threads feed the
//!   paths they show or may show next ([`crate::GitMirror::want`]), and a diff of a file the count
//!   has not reached waits for it instead of fetching the file's blobs itself, which the count
//!   would download a second time. The next chunk takes the wanted files that wait for a chunk, at
//!   most [`FIRST_CHUNK`] of them, filled up with the files next in tree order only while it is
//!   the first chunk (so a pull request that opens with wanted files fetches as often as one
//!   without). Later, the wanted files gather for [`GATHER`] first, so a diff and the neighbours
//!   asked for in the same frame come in one fetch. The chunk may start beside the [`PARALLEL`]
//!   others. A wanted file in a chunk on its way already waits for that chunk: no blob downloads
//!   twice, unless the chunk takes longer than [`COUNT_WAIT`]. A diff waits that long at most
//!   once the count listed its files (a diff asked for while it lists waits for the list) and
//!   then fetches its file's blobs itself, so a slow chunk (a large one, a slow host) never holds
//!   a diff up for longer than about one fetch of its own would; the chunk may then download those
//!   blobs a second time. Work beside the view, such as a thread's excerpt on the overview, waits
//!   for the chunk however long it takes ([`crate::GitMirror::file_diff_after_count`]): a wanted
//!   file's chunk is small, about one fetch of its own.
//! - After each chunk the caller gets the whole list: counted files with their counts and marks,
//!   the others without counts yet. Nothing is published when every blob was local from the
//!   start; the result follows at once then.
//! - Chunks compare without rename detection, which needs every added and deleted file's blobs.
//!   Once all are local, one `git diff-tree -M --diff-filter=R` pairs the renames, and each pair
//!   replaces its deleted and added file in one step. The result equals one
//!   `git diff-tree -M --numstat` over the comparison, with the marks of the same with `-w`.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use review_core::{ProviderError, Result};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::changed_files::{self, ChangeKind, ChangedFile};
use crate::command::{self, GitCommand, Objects};

/// Files in the first chunk that downloads: few, so the first counts come quickly.
const FIRST_CHUNK: usize = 16;
/// Files in a chunk at most; each chunk after the first doubles until it reaches this.
const MAX_CHUNK: usize = 256;
/// Bytes of paths on one chunk's command line at most; Windows caps a command line at 32,767
/// characters.
const MAX_PATH_BYTES: usize = 16 * 1024;
/// Chunks downloading at the same time.
const PARALLEL: usize = 4;
/// How long wanted files gather before their chunk starts, once a chunk is on its way: a frame
/// asks for the open file's diff and then for its neighbours, and one fetch brings them all.
const GATHER: Duration = Duration::from_millis(25);
/// How long a diff waits at most for the count to bring its file's blobs before it fetches them
/// itself: about one fetch of one file's blobs (0.87 s against github.com, commit b67a85c). Our
/// own number.
pub const COUNT_WAIT: Duration = Duration::from_secs(1);
/// How often the files not counted yet are checked for blobs that arrived another way while
/// chunks download; each check reads their blob ids.
const RECHECK: Duration = Duration::from_secs(1);

/// The sizes of the chunks `files` changed files download in when none of their blobs is local and
/// their paths are short enough that no chunk reaches [`MAX_PATH_BYTES`]: one fetch each.
pub fn download_chunk_sizes(files: usize) -> Vec<usize> {
    let mut sizes = Vec::new();
    let (mut next, mut rest) = (FIRST_CHUNK, files);
    while rest > 0 {
        let size = next.min(rest);
        sizes.push(size);
        rest -= size;
        next = grown(next);
    }
    sizes
}

/// The size of the chunk after one of `size` files.
fn grown(size: usize) -> usize {
    (size * 2).min(MAX_CHUNK)
}

/// Receives the file list after each counted chunk ([`count`]).
pub(crate) type CountSink<'a> = dyn Fn(&[ChangedFile]) + Send + Sync + 'a;

/// Counts the comparison of `base` and `head` as the module doc says, publishing to `publish`;
/// the files `wanted` names count first.
pub(crate) async fn count(
    git: &GitCommand<'_>,
    (base, head): (&str, &str),
    wanted: &Wanted,
    publish: &CountSink<'_>,
) -> Result<Vec<ChangedFile>> {
    // Noted before the listing, so a diff asked for meanwhile waits for the count, not past it.
    let run = wanted.start(base, head);
    let listed = git.run(["diff-tree", "-r", "-z", "--no-renames", "--raw", base, head]).await?;
    let mut files = changed_files::parse_raw(&String::from_utf8_lossy(&listed))?;
    run.listed(&files);
    let mut wanted_changes = wanted.subscribe();
    let index_of: HashMap<String, usize> =
        files.iter().enumerate().map(|(index, file)| (file.path.clone(), index)).collect();
    let mut plan = Plan::new(tree_order(&files));
    let mut missing = missing_blobs(git, blob_ids(&files, &plan.uncounted())).await?;
    let mut checked_at = Instant::now();
    let is_streamed = !missing.is_empty();
    let mut running = JoinSet::new();
    let mut downloads = 0;
    let mut gathered: Option<Instant> = None;
    loop {
        let has_blobs = |file: &ChangedFile| !has_missing_blob(file, &missing);
        for chunk in plan.take_local(&files, has_blobs) {
            running.spawn(count_chunk(git, (base, head), &files, chunk, Source::Local));
        }
        let first: Vec<usize> =
            wanted.paths().iter().filter_map(|path| index_of.get(path).copied()).collect();
        if !plan.waits_for(&first) {
            gathered = None;
        } else if !plan.is_first_chunk() {
            gathered.get_or_insert_with(|| Instant::now() + GATHER);
        }
        let is_gathering = gathered.is_some_and(|until| Instant::now() < until);
        // A wanted chunk may start beside the others; the rest keep to their number.
        while downloads <= PARALLEL {
            let wanted_chunk = (!is_gathering).then(|| plan.take_wanted(&files, &first)).flatten();
            if wanted_chunk.is_some() {
                gathered = None;
            }
            let chunk = wanted_chunk
                .or_else(|| (downloads < PARALLEL).then(|| plan.take_download(&files)).flatten());
            let Some(chunk) = chunk else {
                break;
            };
            running.spawn(count_chunk(git, (base, head), &files, chunk, Source::Download));
            downloads += 1;
        }
        tokio::select! {
            Some(done) = running.join_next() => {
                let counted = done.map_err(|error| {
                    ProviderError::Local(format!("counting lines stopped: {error}"))
                })??;
                if counted.source == Source::Download {
                    downloads -= 1;
                    plan.downloaded(&counted.indexes);
                }
                changed_files::apply_counts(
                    &mut files,
                    &counted.indexes,
                    &String::from_utf8_lossy(&counted.counts),
                    &String::from_utf8_lossy(&counted.ignoring_whitespace),
                )?;
                run.counted(counted.indexes.iter().filter_map(|&index| files.get(index)));
                if is_streamed {
                    publish(&files);
                }
            }
            Ok(()) = wanted_changes.changed(), if !plan.is_counted() => {}
            () = tokio::time::sleep_until(gathered.unwrap_or_else(Instant::now)), if is_gathering => {}
            () = tokio::time::sleep_until(checked_at + RECHECK), if !plan.is_counted() => {}
            else => break,
        }
        if !plan.is_counted() && checked_at.elapsed() >= RECHECK {
            missing = missing_blobs(git, blob_ids(&files, &plan.uncounted())).await?;
            checked_at = Instant::now();
        }
    }
    pair_renames(git, base, head, files).await
}

/// Where a chunk's blobs come from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The mirror has them.
    Local,
    /// The chunk's count downloads them.
    Download,
}

/// One chunk's counts, from `git diff-tree --numstat` and the same with `-w`.
struct Counted {
    indexes: Vec<usize>,
    source: Source,
    counts: Vec<u8>,
    ignoring_whitespace: Vec<u8>,
}

/// Counts the files at `indexes` of `files` (`base` against `head`), apart from the borrows: the
/// task can run beside the others.
fn count_chunk(
    git: &GitCommand<'_>,
    (base, head): (&str, &str),
    files: &[ChangedFile],
    indexes: Vec<usize>,
    source: Source,
) -> impl Future<Output = Result<Counted>> + Send + 'static {
    let paths = indexes.iter().filter_map(|&index| files.get(index)).map(|file| file.path.as_str());
    let args = |ignore_whitespace: bool| {
        let mut args = vec!["--literal-pathspecs", "diff-tree", "-r", "-z", "--no-renames"];
        args.extend(["--numstat"].into_iter().chain(ignore_whitespace.then_some("-w")));
        args.extend([base, head, "--"]);
        args.extend(paths.clone());
        args
    };
    // The count downloads what is missing in one request; the second pass finds it all local.
    let counts = git.command(args(false), Objects::FetchMissing);
    let ignoring_whitespace = git.command(args(true), Objects::LocalOnly);
    async move {
        let counts = command::stdout_of(counts).await?;
        let ignoring_whitespace = command::stdout_of(ignoring_whitespace).await?;
        Ok(Counted { indexes, source, counts, ignoring_whitespace })
    }
}

/// The ids of the blobs the files at `indexes` have on either side.
fn blob_ids<'f>(files: &'f [ChangedFile], indexes: &[usize]) -> Vec<&'f str> {
    let mut ids = Vec::new();
    for file in indexes.iter().filter_map(|&index| files.get(index)) {
        let blobs = [(file.has_old_blob(), &file.old_oid), (file.has_new_blob(), &file.new_oid)];
        ids.extend(blobs.into_iter().filter(|(exists, _)| *exists).map(|(_, oid)| oid.as_str()));
    }
    ids
}

/// The blobs among `ids` that the mirror does not have, asked without fetching:
/// `git rev-list --objects --no-walk --ignore-missing --missing=print` lists the blobs it has and
/// skips the others. Not `git cat-file --batch-check`: offline, git 2.47 answers `<id> missing`,
/// but git 2.39.5 (Debian 12) stops at the first missing blob with "could not fetch … from
/// promisor remote", and no file would ever count.
pub(crate) async fn missing_blobs<'a>(
    git: &GitCommand<'_>,
    ids: impl IntoIterator<Item = &'a str>,
) -> Result<HashSet<String>> {
    let mut asked: HashSet<&str> = ids.into_iter().collect();
    if asked.is_empty() {
        return Ok(HashSet::new());
    }
    let input: String = asked.iter().flat_map(|oid| [oid, "\n"]).collect();
    let listed = ["rev-list", "--objects", "--no-walk", "--ignore-missing", "--missing=print"];
    let output =
        git.run_offline_with_input(listed.into_iter().chain(["--stdin"]), input.as_bytes()).await?;
    let output = String::from_utf8_lossy(&output);
    // `<id> ` per blob the mirror has; a `?<id>` line names one it lacks.
    for local in output.lines().filter(|line| !line.starts_with('?')) {
        asked.remove(local.split(' ').next().unwrap_or_default());
    }
    Ok(asked.into_iter().map(str::to_owned).collect())
}

fn has_missing_blob(file: &ChangedFile, missing: &HashSet<String>) -> bool {
    missing.contains(&file.old_oid) || missing.contains(&file.new_oid)
}

/// `files` with their renames paired by one pass over the comparison (module doc). Renames pair
/// a deleted with an added file, so without both there is nothing to pair.
async fn pair_renames(
    git: &GitCommand<'_>,
    base: &str,
    head: &str,
    files: Vec<ChangedFile>,
) -> Result<Vec<ChangedFile>> {
    let has_kind = |kind: ChangeKind| files.iter().any(|file| file.kind == kind);
    if !(has_kind(ChangeKind::Added) && has_kind(ChangeKind::Deleted)) {
        return Ok(files);
    }
    let renames = ["diff-tree", "-r", "-z", "-M", "--diff-filter=R"];
    let output = git.run(renames.into_iter().chain(["--raw", "--numstat", base, head])).await?;
    let mut renamed = changed_files::parse_raw_with_numstat(&String::from_utf8_lossy(&output))?;
    let ignoring_whitespace =
        git.run_offline(renames.into_iter().chain(["--numstat", "-w", base, head])).await?;
    changed_files::mark_whitespace_only(
        &mut renamed,
        &String::from_utf8_lossy(&ignoring_whitespace),
    )?;
    Ok(changed_files::pair_renames(files, renamed))
}

/// Who feeds files to count first ([`crate::GitMirror::want`]), most wanted first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WantedBy {
    /// The Files tab: the open file, the files it loads ahead and the row the pointer rests on.
    Files,
    /// The threads' files, which their excerpts show.
    Threads,
}

/// What the user is near, for the counts running on one mirror (module doc): each feed's files,
/// the files diffs wait for, and every running count's files not counted yet.
pub(crate) struct Wanted {
    state: Mutex<WantedState>,
    /// Bumped when the wanted files change: the running counts take them up.
    changed: watch::Sender<u64>,
    /// Bumped when a count counted files or ended: the waiting diffs look again.
    counted: watch::Sender<u64>,
}

#[derive(Default)]
struct WantedState {
    /// Each feed's files, replaced as a whole, in [`WantedBy`] order.
    feeds: Vec<(WantedBy, Vec<String>)>,
    /// Files diffs wait for, with how many wait for each.
    waits: Vec<(String, usize)>,
    counts: Vec<RunningCount>,
    next_id: u64,
}

/// A count on its way and its files not counted yet; `None` while it lists them.
struct RunningCount {
    id: u64,
    base: String,
    head: String,
    uncounted: Option<HashSet<String>>,
}

impl Default for Wanted {
    fn default() -> Self {
        Self { state: Mutex::default(), changed: watch::channel(0).0, counted: watch::channel(0).0 }
    }
}

impl Wanted {
    fn lock(&self) -> MutexGuard<'_, WantedState> {
        // The state stays whole between statements, so a panic elsewhere leaves it usable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the files `by` wants counted first.
    pub(crate) fn feed(&self, by: WantedBy, paths: &[String]) {
        let mut state = self.lock();
        match state.feeds.iter_mut().find(|(feed, _)| *feed == by) {
            Some((_, fed)) if fed.as_slice() == paths => return,
            Some((_, fed)) => *fed = paths.to_vec(),
            None if paths.is_empty() => return,
            None => {
                state.feeds.push((by, paths.to_vec()));
                state.feeds.sort_by_key(|(feed, _)| *feed);
            }
        }
        drop(state);
        self.changed.send_modify(|version| *version += 1);
    }

    /// Every wanted file, most wanted first: the ones diffs wait for, then each feed's.
    fn paths(&self) -> Vec<String> {
        let state = self.lock();
        let waits = state.waits.iter().map(|(path, _)| path);
        waits.chain(state.feeds.iter().flat_map(|(_, paths)| paths)).cloned().collect()
    }

    fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Notes the count of `base` against `head` until the returned run is dropped; its files
    /// follow once listed ([`CountRun::listed`]).
    fn start(&self, base: &str, head: &str) -> CountRun<'_> {
        let mut state = self.lock();
        let id = state.next_id;
        state.next_id += 1;
        state.counts.push(RunningCount {
            id,
            base: base.to_owned(),
            head: head.to_owned(),
            uncounted: None,
        });
        CountRun { wanted: self, id }
    }

    /// Of `paths`, those a count of `base` against `head` on its way has not counted yet; `None`
    /// while such a count still lists its files.
    fn uncounted(&self, base: &str, head: &str, paths: &[&str]) -> Option<Vec<String>> {
        let state = self.lock();
        let mut uncounted: Vec<&HashSet<String>> = Vec::new();
        for count in state.counts.iter().filter(|count| count.base == base && count.head == head) {
            uncounted.push(count.uncounted.as_ref()?);
        }
        let is_uncounted = |path: &str| uncounted.iter().any(|files| files.contains(path));
        Some(
            paths.iter().filter(|path| is_uncounted(path)).map(|path| (*path).to_owned()).collect(),
        )
    }

    /// Waits until the counts of `base` against `head` on their way have counted `paths`, which
    /// count first meanwhile: their blobs are local then. Returns at once when none counts them,
    /// and `limit` after the counts listed their files at most. Listing is one local
    /// `git diff-tree` and does not count: a diff asked for while a large pull request's count
    /// lists its files would else run out of time before the count could take its file up.
    pub(crate) async fn wait_counted(
        &self,
        base: &str,
        head: &str,
        paths: &[&str],
        limit: Option<Duration>,
    ) {
        // Subscribed before the first look, so no count that lands after it goes unseen.
        let mut counted = self.counted.subscribe();
        let pending = loop {
            if let Some(pending) = self.uncounted(base, head, paths) {
                break pending;
            }
            // The sender lives as long as `self`.
            if counted.changed().await.is_err() {
                return;
            }
        };
        if pending.is_empty() {
            return;
        }
        let listed = self.wait_listed(&mut counted, base, head, pending);
        match limit {
            Some(limit) => {
                // Elapsed: the chunk is slow, and the caller fetches the blobs itself.
                let _elapsed = tokio::time::timeout(limit, listed).await;
            }
            None => listed.await,
        }
    }

    /// [`Self::wait_counted`] once the counts listed their files: `pending` count first meanwhile.
    async fn wait_listed(
        &self,
        counted: &mut watch::Receiver<u64>,
        base: &str,
        head: &str,
        pending: Vec<String>,
    ) {
        let _waits = Waits::new(self, pending.clone());
        let pending: Vec<&str> = pending.iter().map(String::as_str).collect();
        while self.uncounted(base, head, &pending).is_none_or(|left| !left.is_empty()) {
            if counted.changed().await.is_err() {
                return;
            }
        }
    }
}

/// A count on its way, noted in [`Wanted`] until dropped (also when the count is cancelled), so
/// diffs never wait for a count that is gone.
struct CountRun<'a> {
    wanted: &'a Wanted,
    id: u64,
}

impl CountRun<'_> {
    /// The count's files are listed, none of them counted yet.
    fn listed(&self, files: &[ChangedFile]) {
        let mut state = self.wanted.lock();
        if let Some(count) = state.counts.iter_mut().find(|count| count.id == self.id) {
            count.uncounted = Some(files.iter().map(|file| file.path.clone()).collect());
        }
        drop(state);
        self.wanted.counted.send_modify(|version| *version += 1);
    }

    /// `files` are counted: their blobs are local.
    fn counted<'f>(&self, files: impl Iterator<Item = &'f ChangedFile>) {
        let mut state = self.wanted.lock();
        let count = state.counts.iter_mut().find(|count| count.id == self.id);
        if let Some(uncounted) = count.and_then(|count| count.uncounted.as_mut()) {
            for file in files {
                uncounted.remove(&file.path);
            }
        }
        drop(state);
        self.wanted.counted.send_modify(|version| *version += 1);
    }
}

impl Drop for CountRun<'_> {
    fn drop(&mut self) {
        self.wanted.lock().counts.retain(|count| count.id != self.id);
        self.wanted.counted.send_modify(|version| *version += 1);
    }
}

/// Files a diff waits for, wanted by [`Wanted`] until dropped (also when the diff is cancelled).
struct Waits<'a> {
    wanted: &'a Wanted,
    paths: Vec<String>,
}

impl<'a> Waits<'a> {
    fn new(wanted: &'a Wanted, paths: Vec<String>) -> Self {
        let mut state = wanted.lock();
        for path in &paths {
            match state.waits.iter_mut().find(|(waited, _)| waited == path) {
                Some((_, diffs)) => *diffs += 1,
                None => state.waits.push((path.clone(), 1)),
            }
        }
        drop(state);
        wanted.changed.send_modify(|version| *version += 1);
        Self { wanted, paths }
    }
}

impl Drop for Waits<'_> {
    fn drop(&mut self) {
        let mut state = self.wanted.lock();
        for path in &self.paths {
            if let Some((_, diffs)) = state.waits.iter_mut().find(|(waited, _)| waited == path) {
                *diffs = diffs.saturating_sub(1);
            }
        }
        state.waits.retain(|(_, diffs)| *diffs > 0);
    }
}

/// The files still to count: those in no chunk yet in counting order, the uncounted ones of each
/// download on its way, and the size of the next chunk to download.
struct Plan {
    waiting: Vec<usize>,
    /// Per running download: its first file, which names it, and its files not counted yet.
    downloading: Vec<(usize, Vec<usize>)>,
    next_chunk: usize,
}

impl Plan {
    fn new(order: Vec<usize>) -> Self {
        Self { waiting: order, downloading: Vec::new(), next_chunk: FIRST_CHUNK }
    }

    /// Every file is counted or in a local chunk.
    fn is_counted(&self) -> bool {
        self.waiting.is_empty() && self.downloading.is_empty()
    }

    /// The files no count has reached yet: the waiting ones, then those downloading.
    fn uncounted(&self) -> Vec<usize> {
        let downloading = self.downloading.iter().flat_map(|(_, files)| files);
        self.waiting.iter().chain(downloading).copied().collect()
    }

    /// Takes every uncounted file `has_blobs` accepts, in chunks of at most [`MAX_PATH_BYTES`] of
    /// paths; a download whose files it accepts all is counting them already and keeps them.
    fn take_local(
        &mut self,
        files: &[ChangedFile],
        has_blobs: impl Fn(&ChangedFile) -> bool,
    ) -> Vec<Vec<usize>> {
        let is_local = |index: &usize| files.get(*index).is_some_and(&has_blobs);
        let (mut local, waiting): (Vec<usize>, Vec<usize>) =
            self.waiting.iter().copied().partition(is_local);
        self.waiting = waiting;
        for (_, uncounted) in &mut self.downloading {
            if !uncounted.iter().all(is_local) {
                let (arrived, still): (Vec<usize>, Vec<usize>) =
                    uncounted.iter().copied().partition(is_local);
                local.extend(arrived);
                *uncounted = still;
            }
        }
        let mut chunks = Vec::new();
        let mut rest = local.as_slice();
        while !rest.is_empty() {
            let size = fitting(files, rest, usize::MAX);
            let (chunk, after) = rest.split_at(size);
            chunks.push(chunk.to_vec());
            rest = after;
        }
        chunks
    }

    /// Takes the next chunk to download from the front: [`Self::next_chunk`] files at most, then
    /// the next one is twice as large, up to [`MAX_CHUNK`].
    fn take_download(&mut self, files: &[ChangedFile]) -> Option<Vec<usize>> {
        let size = fitting(files, &self.waiting, self.next_chunk);
        let chunk: Vec<usize> = self.waiting.drain(..size).collect();
        let first = *chunk.first()?;
        self.next_chunk = grown(self.next_chunk);
        self.downloading.push((first, chunk.clone()));
        Some(chunk)
    }

    /// No chunk was taken yet.
    fn is_first_chunk(&self) -> bool {
        self.next_chunk == FIRST_CHUNK
    }

    /// Some file of `wanted` (indexes) waits for a chunk.
    fn waits_for(&self, wanted: &[usize]) -> bool {
        wanted.iter().any(|index| self.waiting.contains(index))
    }

    /// Takes the files of `wanted` (indexes, most wanted first) that wait for a chunk, at most
    /// [`FIRST_CHUNK`] of them; the first chunk is filled up from the front to its usual size and
    /// counts as that chunk. `None` when no wanted file waits for a chunk.
    fn take_wanted(&mut self, files: &[ChangedFile], wanted: &[usize]) -> Option<Vec<usize>> {
        let waiting: HashSet<usize> = self.waiting.iter().copied().collect();
        let mut chunk: Vec<usize> = Vec::new();
        for &index in wanted.iter().filter(|index| waiting.contains(index)) {
            if chunk.len() == FIRST_CHUNK {
                break;
            }
            if !chunk.contains(&index) {
                chunk.push(index);
            }
        }
        let first = *chunk.first()?;
        if self.is_first_chunk() {
            let front: Vec<usize> = self.waiting.iter().copied().take(FIRST_CHUNK).collect();
            for index in front {
                if chunk.len() < FIRST_CHUNK && !chunk.contains(&index) {
                    chunk.push(index);
                }
            }
            self.next_chunk = grown(self.next_chunk);
        }
        chunk.truncate(fitting(files, &chunk, FIRST_CHUNK));
        self.waiting.retain(|index| !chunk.contains(index));
        self.downloading.push((first, chunk.clone()));
        Some(chunk)
    }

    /// The download of `chunk` counted its files.
    fn downloaded(&mut self, chunk: &[usize]) {
        self.downloading.retain(|(first, _)| chunk.first() != Some(first));
    }
}

/// How many of `indexes`, from the front, make one chunk: at most `limit` files and
/// [`MAX_PATH_BYTES`] of paths, and at least one file.
fn fitting(files: &[ChangedFile], indexes: &[usize], limit: usize) -> usize {
    let mut bytes = 0;
    let fitting = indexes
        .iter()
        .take(limit)
        .take_while(|&&index| {
            // The path, a separating space and quotes around it, where it needs them.
            bytes += files.get(index).map_or(0, |file| file.path.len() + 3);
            bytes <= MAX_PATH_BYTES
        })
        .count();
    fitting.max(1).min(indexes.len())
}

/// The indexes of `files` in the order a file tree lists them: in every folder its subfolders
/// first, then its files, each by name.
fn tree_order(files: &[ChangedFile]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by(|&first, &second| match (files.get(first), files.get(second)) {
        (Some(first), Some(second)) => tree_cmp(&first.path, &second.path),
        _ => Ordering::Equal,
    });
    order
}

/// `first` against `second` as [`tree_order`] lists them.
fn tree_cmp(first: &str, second: &str) -> Ordering {
    let (mut first, mut second) = (first.split('/').peekable(), second.split('/').peekable());
    loop {
        let (Some(first_part), Some(second_part)) = (first.next(), second.next()) else {
            return Ordering::Equal;
        };
        // A part with more after it is a folder.
        let (first_is_file, second_is_file) = (first.peek().is_none(), second.peek().is_none());
        let order = first_is_file.cmp(&second_is_file).then_with(|| first_part.cmp(second_part));
        if order != Ordering::Equal {
            return order;
        }
    }
}

#[cfg(test)]
#[path = "line_counts_tests.rs"]
mod tests;
