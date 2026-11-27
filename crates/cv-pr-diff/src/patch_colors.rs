//! Syntax colors by diff line, worked out from the diff's own text, so a diff shows colored on
//! the first frame instead of waiting for both file versions to load and highlight from line 0.
//!
//! Each hunk is read from its first line with a fresh grammar state, per side (unchanged and added
//! lines as the new version, unchanged and deleted lines as the old one). A hunk that starts inside
//! a comment or string spanning its start reads that part wrong until the file's own colors arrive
//! and take over line by line; everywhere else both agree, so nothing visible changes then.
//!
//! The work runs on a small pool next to the whole-file coloring thread, so it never waits behind
//! a long file, and on the shared compiled grammars (see [`grammar::compiled`]). A diff load waits
//! for its first lines (or at most [`WAIT`]) before it reports the diff, so diff and colors reach
//! the screen in the same frame.
//!
//! Colors that took long to work out (a large diff, a grammar compiled first) also go to the disk
//! tier ([`ColdTier`]), keyed by the compared commits, path and options (immutable) and the app
//! version (the grammars ship with it): after a restart that diff shows colored without compiling.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cv_base::theme::syntax::TokenKind;
use cv_runtime::cache::cold::ColdTier;
use cv_runtime::cache::memory::Holder;
use cv_syntax::grammar::{self, Grammar, WorkingSet};
use cv_syntax::highlight::{
    self, GrammarState, LineHighlighter, LineReader, LineSpans, Part, ReadParts, Span,
};
use cv_syntax::pool::{Pool, Priority};
use review_git::{ChangedFile, FileDiff, LineKind};

use crate::diff_cache::DiffKey;
use crate::warm_up::{self, WarmUp};

/// Larger diffs color only their first lines this way; the file's own colors cover the rest.
const MAX_LINES: usize = 50_000;
/// The first chunk covers a screen of rows, so the view can show colored before the rest is done.
const FIRST_CHUNK_LINES: usize = 120;
const CHUNK_LINES: usize = 1_000;
/// How long a diff load waits for its first colored lines before it shows the diff without them.
pub(crate) const WAIT: Duration = Duration::from_millis(150);
/// How long it waits instead while a warm-up compiles the diff's grammar (a file opened right
/// after the pull request): the colors are on their way, so the diff shows with them. TypeScript,
/// the slowest grammar, warms in about 0.3 to 0.5 s.
pub(crate) const WARMING_WAIT: Duration = Duration::from_millis(1000);
/// Colors kept per pull request, the least recently used dropped first: our number, the open file
/// and the neighbours worked out ahead (a 15,000-line file takes about 1 MB), so stepping back and
/// forth shows colored at once. A file further away colors again: small ones in milliseconds,
/// large ones come back from the disk tier.
const CACHE_BUDGET_BYTES: usize = 8 * 1024 * 1024;
/// Colors that took this long go to the disk tier: our number, ten frames; reading them back takes
/// a millisecond or two.
const COLD_WORTHY: Duration = Duration::from_millis(150);
/// Diffs shorter than this color fast enough unless a grammar compiles first, which the disk tier
/// does not need to cover: the warm-up does.
const COLD_MIN_LINES: usize = 2_000;
const COLD_KIND: &str = "diff-colors";
const COLD_VERSION: u32 = 1;
/// Pool threads: the open file and a neighbour or a card can color at once.
static POOL: Pool = Pool::new("patch-colors", 2);

/// The grammar for a diff of `path`, falling back to its first line (shebangs, `<?xml`). Loads
/// the grammar set on first use, so only call this from background work.
fn grammar_for(path: &str, diff: &FileDiff) -> Option<Grammar> {
    let first_line = diff
        .lines()
        .iter()
        .find(|line| line.kind != LineKind::Hunk)
        .map_or("", |line| diff.text(line));
    grammar::for_path(path, first_line)
}

/// Highlights `diff` hunk by hunk into spans per diff line (hunk headers get none), handing each
/// finished chunk to `deliver`; stops as soon as `deliver` returns false.
///
/// Both sides read the unchanged lines, but while their grammar states agree (before a hunk's
/// first change, and again once a change is closed) only the new side does, so unchanged lines
/// cost one parse, not two. A long diff reads in parts at once (see [`highlight::read_lines`]),
/// with the same colors.
pub(crate) fn highlight_patch(
    grammar: Grammar,
    working: &Arc<WorkingSet>,
    diff: &FileDiff,
    mut deliver: impl FnMut(LineSpans) -> bool,
) {
    let mut chunk = LineSpans::default();
    let mut chunk_lines = FIRST_CHUNK_LINES;
    let count = diff.lines().len().min(MAX_LINES);
    let reader = PatchReader::new(grammar, working, diff);
    let is_read = highlight::read_lines(reader, count, |spans| {
        chunk.push_line(spans);
        if chunk.line_count() < chunk_lines {
            return true;
        }
        chunk_lines = CHUNK_LINES;
        deliver(std::mem::take(&mut chunk))
    });
    if is_read && chunk.line_count() > 0 {
        deliver(chunk);
    }
}

/// The lines of a diff, each read from its own side.
struct PatchReader<'a> {
    grammar: Grammar,
    working: &'a Arc<WorkingSet>,
    diff: &'a FileDiff,
    new: LineHighlighter<'a>,
    old: LineHighlighter<'a>,
    /// The old side is in the new side's state and has not read anything of its own.
    is_in_step: bool,
    unused: Vec<Span>,
}

impl<'a> PatchReader<'a> {
    fn new(grammar: Grammar, working: &'a Arc<WorkingSet>, diff: &'a FileDiff) -> Self {
        Self {
            grammar,
            working,
            diff,
            new: LineHighlighter::new(grammar.syntax, working),
            old: LineHighlighter::new(grammar.syntax, working),
            is_in_step: true,
            unused: Vec::new(),
        }
    }
}

/// What a [`PatchReader`]'s parts read with on the reader threads.
struct PatchParts {
    grammar: Grammar,
    working: Arc<WorkingSet>,
    diff: FileDiff,
}

type PatchState = (GrammarState, Option<GrammarState>);

impl ReadParts for PatchParts {
    type State = PatchState;

    /// A part starts as at the start of a hunk, its new side in `state`'s. The old side starts in
    /// step: one out of step from elsewhere would never come back in step.
    fn read_part(
        &self,
        state: Option<&PatchState>,
        lines: Range<usize>,
        stop: &AtomicBool,
    ) -> Part<PatchState> {
        let mut reader = PatchReader::new(self.grammar, &self.working, &self.diff);
        if let Some((new, _)) = state {
            reader.new.restore(new);
        }
        highlight::read_part(reader, lines, stop)
    }
}

impl LineReader for PatchReader<'_> {
    /// The new side's state, and the old side's while it is not in step.
    type State = PatchState;
    type Parts = PatchParts;

    /// The reader threads get a copy of the diff: the caller's diff is only borrowed.
    fn parts(&self) -> PatchParts {
        PatchParts { grammar: self.grammar, working: self.working.clone(), diff: self.diff.clone() }
    }

    fn restore(&mut self, (new, old): &PatchState) {
        self.new.restore(new);
        if let Some(old) = old {
            self.old.restore(old);
        }
        self.is_in_step = old.is_none();
    }

    fn read(&mut self, index: usize, spans: &mut Vec<Span>) {
        let line = &self.diff.lines()[index];
        let text = self.diff.text(line);
        match line.kind {
            LineKind::Hunk => {
                self.new.restart();
                self.is_in_step = true;
                spans.clear();
            }
            LineKind::Context => {
                self.new.line(text, spans);
                if !self.is_in_step {
                    self.old.line(text, &mut self.unused);
                    self.is_in_step = self.old.is_in_step_with(&self.new);
                }
            }
            LineKind::Added | LineKind::Deleted => {
                if self.is_in_step {
                    self.old.follow(&self.new);
                    self.is_in_step = false;
                }
                let side = if line.kind == LineKind::Added { &mut self.new } else { &mut self.old };
                side.line(text, spans);
            }
        }
    }

    fn state(&self) -> Self::State {
        (self.new.state(), (!self.is_in_step).then(|| self.old.state()))
    }

    fn is_in(&self, (new, old): &Self::State) -> bool {
        let is_old_in = match old {
            Some(old) => !self.is_in_step && self.old.is_in(old),
            None => self.is_in_step,
        };
        is_old_in && self.new.is_in(new)
    }

    /// Only unchanged lines: a part starting at a change would have its old side miss the
    /// deleted lines before it.
    fn start_indent(&self, index: usize) -> Option<usize> {
        let line = &self.diff.lines()[index];
        if line.kind != LineKind::Context {
            return None;
        }
        highlight::start_indent(self.diff.text(line))
    }
}

enum Message {
    Chunk { key: DiffKey, job: u64, chunk: LineSpans },
    Finished { key: DiffKey, job: u64 },
}

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

/// Colorings started outside the UI thread, with their job and cancel flag.
type Started = Arc<Mutex<Vec<(DiffKey, u64, Arc<AtomicBool>)>>>;

/// Starts colorings from anywhere (a diff load's task, the UI); [`PatchColors`] keeps the results.
#[derive(Clone)]
pub struct Colorer {
    sender: Sender<Message>,
    /// Jobs started outside the UI thread, announced so the UI does not start them again.
    started: Started,
    ctx: egui::Context,
    wait: Duration,
    warming_wait: Duration,
    cold: Option<ColdTier>,
    cold_worthy: Duration,
}

impl Colorer {
    /// Colors `diff` in the background and returns once its first lines are colored, the diff has
    /// no grammar, or after [`WAIT`] ([`WARMING_WAIT`] while a warm-up compiles its grammar),
    /// whichever comes first. [`Priority::Urgent`] puts it before queued work.
    pub async fn color(&self, key: DiffKey, diff: Arc<FileDiff>, priority: Priority) {
        let (first, first_colored) = tokio::sync::oneshot::channel();
        let job = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.started.lock().unwrap_or_else(PoisonError::into_inner).push((
            key.clone(),
            job,
            cancel.clone(),
        ));
        let grammar = grammar_for(&key.path, &diff);
        self.run(key, job, diff, cancel, priority, Some(first));
        let Some(grammar) = grammar else {
            // No colors to wait for, however long the pool takes to get to the job.
            return;
        };
        let is_warming = grammar::is_warming(grammar);
        // First lines in, the wait over or the job ended uncolored: the diff shows either way.
        let _shown_either_way = tokio::time::timeout(
            if is_warming { self.warming_wait } else { self.wait },
            first_colored,
        )
        .await;
    }

    fn run(
        &self,
        key: DiffKey,
        job: u64,
        diff: Arc<FileDiff>,
        cancel: Arc<AtomicBool>,
        priority: Priority,
        first: Option<tokio::sync::oneshot::Sender<()>>,
    ) {
        let (sender, ctx, cold) = (self.sender.clone(), self.ctx.clone(), self.cold.clone());
        let cold_worthy = self.cold_worthy;
        POOL.submit(priority, move || {
            let mut first = first;
            // A diff that fits the first chunk is finished before its load goes on.
            let is_one_chunk = diff.lines().len() <= FIRST_CHUNK_LINES;
            let cold = cold.filter(|_| diff.lines().len() >= COLD_MIN_LINES);
            let saved = cold.as_ref().and_then(|cold| {
                cold.read(COLD_KIND, COLD_VERSION, &cold_key(&key)).and_then(|bytes| decode(&bytes))
            });
            if let Some(chunk) = saved {
                let _pull_request_closed =
                    sender.send(Message::Chunk { key: key.clone(), job, chunk });
            } else if let Some(grammar) =
                grammar_for(&key.path, &diff).filter(|_| !cancel.load(Ordering::Relaxed))
            {
                let started = Instant::now();
                let mut all = cold.as_ref().map(|_| LineSpans::default());
                highlight_patch(grammar, &grammar::compiled(grammar), &diff, |chunk| {
                    if let Some(all) = &mut all {
                        all.append(chunk.clone());
                    }
                    let is_sent =
                        sender.send(Message::Chunk { key: key.clone(), job, chunk }).is_ok();
                    // The chunk is in the channel before the load that waits for it reports the diff.
                    if !is_one_chunk && let Some(first) = first.take() {
                        // The load may have stopped waiting (see `WAIT`).
                        let _load_went_on = first.send(());
                    }
                    ctx.request_repaint();
                    is_sent && !cancel.load(Ordering::Relaxed)
                });
                let is_complete = all
                    .as_ref()
                    .is_some_and(|all| all.line_count() == diff.lines().len().min(MAX_LINES));
                if let (Some(cold), Some(all), true) = (&cold, all, is_complete)
                    && started.elapsed() >= cold_worthy
                {
                    cold.write(COLD_KIND, COLD_VERSION, cold_key(&key), move || encode(&all));
                }
            }
            let _pull_request_closed = sender.send(Message::Finished { key, job });
            if let Some(first) = first {
                let _load_went_on = first.send(());
            }
            ctx.request_repaint();
        });
    }
}

/// The disk tier's key of the colors of `key`: the app version first, since the grammars ship
/// with it.
fn cold_key(key: &DiffKey) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let DiffKey { path, old_path, old_oid, new_oid, options } = key;
    format!(
        "{version}|{old_oid}|{new_oid}|{options:?}|{path}|{}",
        old_path.as_deref().unwrap_or("")
    )
}

/// Per line its span count, then per span its start (4 bytes) and kind (1 byte), little-endian;
/// `None` when a count or a start does not fit in 4 bytes.
fn encode(spans: &LineSpans) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    for index in 0..spans.line_count() {
        let line = spans.line(index).unwrap_or_default();
        bytes.extend_from_slice(&u32::try_from(line.len()).ok()?.to_le_bytes());
        for span in line {
            bytes.extend_from_slice(&u32::try_from(span.start()).ok()?.to_le_bytes());
            bytes.push(span.kind() as u8);
        }
    }
    Some(bytes)
}

/// The spans [`encode`] wrote; `None` for bytes it did not write.
fn decode(mut bytes: &[u8]) -> Option<LineSpans> {
    let mut spans = LineSpans::default();
    let mut line = Vec::new();
    while let Some((count, rest)) = bytes.split_first_chunk::<4>() {
        line.clear();
        bytes = rest;
        for _ in 0..u32::from_le_bytes(*count) {
            let (start, rest) = bytes.split_first_chunk::<4>()?;
            let (kind, rest) = rest.split_first()?;
            let start = usize::try_from(u32::from_le_bytes(*start)).ok()?;
            line.push(Span::new(start, TokenKind::from_index(*kind)));
            bytes = rest;
        }
        spans.push_line(&line);
    }
    bytes.is_empty().then_some(spans)
}

/// Colors all of `diff` (the file at `path`) on the pool behind the work queued already and hands
/// the colors to `done`, empty when the file has no grammar; nothing when `cancel` was set.
pub fn color_whole(
    path: String,
    diff: Arc<FileDiff>,
    cancel: Arc<AtomicBool>,
    done: impl FnOnce(LineSpans) + Send + 'static,
) {
    POOL.submit(Priority::Queued, move || {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let mut spans = LineSpans::default();
        if let Some(grammar) = grammar_for(&path, &diff) {
            highlight_patch(grammar, &grammar::compiled(grammar), &diff, |chunk| {
                spans.append(chunk);
                !cancel.load(Ordering::Relaxed)
            });
        }
        if !cancel.load(Ordering::Relaxed) {
            spans.shrink_to_fit();
            done(spans);
        }
    });
}

struct Entry {
    job: u64,
    spans: Arc<LineSpans>,
    is_done: bool,
    cancel: Arc<AtomicBool>,
}

/// The diff-line colors of the diffs shown or worked out ahead, by diff.
pub struct PatchColors {
    sender: Sender<Message>,
    receiver: Receiver<Message>,
    started: Started,
    entries: HashMap<DiffKey, Entry>,
    /// Keys from least to most recently used.
    order: VecDeque<DiffKey>,
    /// The bytes of `entries` as last counted under [`Holder::DiffColors`].
    counted: usize,
    cold: Option<ColdTier>,
    /// [`COLD_WORTHY`]; a test of the disk tier stores every coloring.
    cold_worthy: Duration,
    /// The warm-up of the pull request's languages; dropping it stops what has not started.
    warm_up: Option<WarmUp>,
    /// [`WAIT`]; a test that checks the wait itself, not its length, waits longer.
    wait: Duration,
    /// [`WARMING_WAIT`]; a test that checks the wait itself, not its length, waits longer.
    warming_wait: Duration,
}

impl Default for PatchColors {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            started: Arc::default(),
            entries: HashMap::new(),
            order: VecDeque::new(),
            counted: 0,
            cold: None,
            cold_worthy: COLD_WORTHY,
            warm_up: None,
            wait: WAIT,
            warming_wait: WARMING_WAIT,
        }
    }
}

impl PatchColors {
    pub fn colorer(&self, ctx: &egui::Context) -> Colorer {
        Colorer {
            sender: self.sender.clone(),
            started: self.started.clone(),
            ctx: ctx.clone(),
            wait: self.wait,
            warming_wait: self.warming_wait,
            cold: self.cold.clone(),
            cold_worthy: self.cold_worthy,
        }
    }

    /// Large colors go to `cold` from now on, and come back from it after a restart.
    pub fn use_cold(&mut self, cold: &ColdTier) {
        if self.cold.is_none() {
            self.cold = Some(cold.clone());
        }
    }

    /// Diff loads wait `wait` instead of [`WAIT`] for their first colors, so a test of that wait
    /// does not depend on how soon a pool busy with other tests' colors gets to them.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_wait(mut self, wait: Duration) -> Self {
        self.wait = wait;
        self
    }

    /// Diff loads wait `wait` instead of [`WARMING_WAIT`] while a warm-up compiles their grammar,
    /// so a test of that wait does not depend on how fast a loaded machine compiles.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_warming_wait(mut self, wait: Duration) -> Self {
        self.warming_wait = wait;
        self
    }

    /// Takes in what the colorings delivered since the last call. Call it after polling the
    /// diff loads, so a diff and the colors its load waited for show in the same frame.
    pub fn poll(&mut self) {
        let started =
            std::mem::take(&mut *self.started.lock().unwrap_or_else(PoisonError::into_inner));
        for (key, job, cancel) in started {
            self.begin(&key, job, cancel);
        }
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                Message::Chunk { key, job, chunk } => {
                    if let Some(entry) = self.entries.get_mut(&key).filter(|entry| entry.job == job)
                    {
                        Arc::make_mut(&mut entry.spans).append(chunk);
                    }
                }
                Message::Finished { key, job } => {
                    if let Some(entry) = self.entries.get_mut(&key).filter(|entry| entry.job == job)
                    {
                        Arc::make_mut(&mut entry.spans).shrink_to_fit();
                        entry.is_done = true;
                    }
                }
            }
        }
        self.evict();
    }

    /// Warms up the grammars of every language among `files` (the pull request's changed files),
    /// `first`'s (the file the Files tab opens) before the others; replaces an earlier warm-up.
    /// The process keeps the compiled sets; nothing here waits for them.
    pub fn warm_up(&mut self, files: &[ChangedFile], first: Option<String>) {
        self.warm_up = Some(warm_up::start(warm_up::changed(files), first));
    }

    /// The colors of the diff `key` as far as they are worked out, the first lines first.
    pub fn spans(&mut self, key: &DiffKey) -> Option<&LineSpans> {
        self.touch(key);
        self.entries.get(key).map(|entry| &*entry.spans)
    }

    /// The colors of the diff `key` once all of them are worked out.
    pub fn finished(&mut self, key: &DiffKey) -> Option<Arc<LineSpans>> {
        self.touch(key);
        self.entries.get(key).filter(|entry| entry.is_done).map(|entry| entry.spans.clone())
    }

    /// Starts coloring `diff`, before queued work, unless it is colored or coloring already (a
    /// diff from the cache whose colors were dropped meanwhile).
    pub fn ensure(&mut self, ctx: &egui::Context, key: &DiffKey, diff: &Arc<FileDiff>) {
        self.poll();
        if self.entries.contains_key(key) {
            return;
        }
        let job = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.begin(key, job, cancel.clone());
        self.colorer(ctx).run(key.clone(), job, diff.clone(), cancel, Priority::Urgent, None);
    }

    /// Test driver: polls on every repaint request of the workers (see
    /// [`cv_syntax::color_worker::observe_repaints`]) until all colors of `key` are worked out,
    /// for at most [`cv_testkit::WAIT_LIMIT`].
    #[cfg(test)]
    pub(crate) fn wait_finished(
        &mut self,
        repaints: &Receiver<()>,
        key: &DiffKey,
    ) -> Option<Arc<LineSpans>> {
        let deadline = Instant::now() + cv_testkit::WAIT_LIMIT;
        loop {
            self.poll();
            if let Some(spans) = self.finished(key) {
                return Some(spans);
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            repaints.recv_timeout(left).ok()?;
        }
    }

    fn begin(&mut self, key: &DiffKey, job: u64, cancel: Arc<AtomicBool>) {
        let entry = Entry { job, spans: Arc::default(), is_done: false, cancel };
        if let Some(replaced) = self.entries.insert(key.clone(), entry) {
            replaced.cancel.store(true, Ordering::Relaxed);
        }
        self.touch(key);
    }

    fn touch(&mut self, key: &DiffKey) {
        if self.order.back() == Some(key) {
            return;
        }
        if let Some(position) = self.order.iter().position(|kept| kept == key) {
            self.order.remove(position);
        }
        if self.entries.contains_key(key) {
            self.order.push_back(key.clone());
        }
    }

    /// Drops the colors of `key` (a diff loaded ahead the user moved away from), stopping their
    /// job if it still runs.
    pub fn release(&mut self, key: &DiffKey) {
        if let Some(entry) = self.entries.remove(key) {
            entry.cancel.store(true, Ordering::Relaxed);
            self.order.retain(|ordered| ordered != key);
        }
    }

    /// Drops every color, stopping the jobs that still run (the page went out of sight).
    pub fn release_all(&mut self) {
        for (_, entry) in self.entries.drain() {
            entry.cancel.store(true, Ordering::Relaxed);
        }
        self.order.clear();
        self.evict();
    }

    /// Drops the least recently used colors beyond the budget, never the most recent one, and
    /// counts what is left under [`Holder::DiffColors`].
    fn evict(&mut self) {
        let mut bytes: usize = self.entries.values().map(|entry| entry.spans.heap_size()).sum();
        while bytes > CACHE_BUDGET_BYTES && self.order.len() > 1 {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                entry.cancel.store(true, Ordering::Relaxed);
                bytes -= entry.spans.heap_size();
            }
        }
        Holder::DiffColors.add(bytes);
        Holder::DiffColors.sub(self.counted);
        self.counted = bytes;
    }
}

impl Drop for PatchColors {
    fn drop(&mut self) {
        Holder::DiffColors.sub(self.counted);
    }
}

#[cfg(test)]
mod tests {
    use cv_base::theme::syntax::TokenKind;
    use cv_syntax::color_worker::observe_repaints;
    use review_git::{DiffOptions, FileDiff};

    use super::*;

    fn key(path: &str) -> DiffKey {
        DiffKey {
            path: path.into(),
            old_path: None,
            old_oid: "1".repeat(40),
            new_oid: "2".repeat(40),
            options: DiffOptions::context(3),
        }
    }

    fn patch_spans(path: &str, diff: &FileDiff) -> LineSpans {
        let grammar = grammar_for(path, diff).expect("a grammar");
        let mut spans = LineSpans::default();
        highlight_patch(grammar, &grammar::compiled(grammar), diff, |chunk| {
            spans.append(chunk);
            true
        });
        spans
    }

    fn kinds(spans: &LineSpans, index: usize) -> Vec<TokenKind> {
        spans.line(index).unwrap().iter().map(|span| span.kind()).collect()
    }

    #[test]
    fn colors_each_line_from_its_own_side() {
        let diff = FileDiff::parse_unified(
            b"@@ -1,2 +1,2 @@\n fn main() {\n-    let a = \"old\";\n+    let a = 42;\n",
        );
        let spans = patch_spans("src/main.rs", &diff);
        assert_eq!(spans.line_count(), diff.lines().len(), "one entry per diff line");
        assert!(spans.line(0).unwrap().is_empty(), "the hunk header");
        assert!(kinds(&spans, 1).contains(&TokenKind::Keyword), "fn: {:?}", kinds(&spans, 1));
        assert!(
            kinds(&spans, 2).contains(&TokenKind::String),
            "the deleted line's string: {:?}",
            kinds(&spans, 2)
        );
        assert!(
            kinds(&spans, 3).contains(&TokenKind::Number),
            "the added line's number: {:?}",
            kinds(&spans, 3)
        );
    }

    #[test]
    fn starts_every_hunk_afresh() {
        // The first hunk ends inside an unterminated block comment; the second starts at code.
        let diff = FileDiff::parse_unified(
            b"@@ -1,1 +1,1 @@\n-/* open\n+/* open\n@@ -40,1 +40,1 @@\n-let a = 1;\n+let a = 2;\n",
        );
        let spans = patch_spans("src/lib.rs", &diff);
        assert!(
            kinds(&spans, 4).contains(&TokenKind::Keyword),
            "the second hunk reads as code: {:?}",
            kinds(&spans, 4)
        );
    }

    /// A long hunk read in parts colors every line as reading it in order does, both sides,
    /// wherever a part starts: in a change, in a comment opened by a deleted line, in a class.
    #[test]
    fn reads_a_long_hunk_in_parts_as_in_order() {
        let block = concat!(
            " export class Name<T> {\n",
            "-    /* old comment\n",
            "+    // new comment\n",
            "     over lines */ private field = 1;\n",
            "-    method(argument: string): void { return; }\n",
            "+    method(argument: number): void { return `${argument}`; }\n",
            " }\n",
            " const text = `template\n",
            "+added inside the template\n",
            " still the template`;\n",
        );
        let side_lines = |marker: char| {
            30 * block.lines().filter(|line| line.starts_with([' ', marker])).count()
        };
        let (old, new) = (side_lines('-'), side_lines('+'));
        let diff = FileDiff::parse_unified(
            format!("@@ -1,{old} +1,{new} @@\n{}", block.repeat(30)).as_bytes(),
        );
        assert_eq!(diff.lines().len(), 1 + 30 * block.lines().count());
        let grammar = grammar_for("a.ts", &diff).expect("a grammar");
        let working = grammar.working_set();
        let read = |parts: usize, head: usize| {
            let mut spans = LineSpans::default();
            let reader = PatchReader::new(grammar, &working, &diff);
            let is_read =
                highlight::read_in_parts(reader, diff.lines().len(), parts, head, |line| {
                    spans.push_line(line);
                    true
                });
            assert!(is_read);
            spans
        };
        let in_order = read(1, 0);
        assert!(kinds(&in_order, 2).contains(&TokenKind::Comment), "{:?}", kinds(&in_order, 2));
        for (parts, head) in [(2, 0), (3, 40), (7, 0), (33, 5)] {
            assert_eq!(read(parts, head), in_order, "{parts} parts after {head}");
        }
    }

    #[test]
    fn delivers_a_screen_first_and_stops_when_asked() {
        let body = "+let a = 1;\n".repeat(FIRST_CHUNK_LINES + CHUNK_LINES + 5);
        let diff = FileDiff::parse_unified(
            format!("@@ -0,0 +1,{} @@\n{body}", FIRST_CHUNK_LINES + CHUNK_LINES + 5).as_bytes(),
        );
        let grammar = grammar_for("a.rs", &diff).unwrap();
        let working = grammar::compiled(grammar);
        let mut chunks = Vec::new();
        highlight_patch(grammar, &working, &diff, |chunk| {
            chunks.push(chunk.line_count());
            true
        });
        assert_eq!(chunks, [FIRST_CHUNK_LINES, CHUNK_LINES, 6]);
        let mut calls = 0;
        highlight_patch(grammar, &working, &diff, |_| {
            calls += 1;
            false
        });
        assert_eq!(calls, 1);
    }

    /// The load's wait ends with the first colored lines already in the channel, so the frame that
    /// shows the diff has them.
    #[test]
    fn a_diff_load_hands_over_its_first_colors_before_it_finishes() {
        let ctx = egui::Context::default();
        let mut colors = PatchColors::default().with_wait(cv_testkit::WAIT_LIMIT);
        let colorer = colors.colorer(&ctx);
        let diff =
            Arc::new(FileDiff::parse_unified(b"@@ -1,1 +1,1 @@\n-fn old() {}\n+fn new() {}\n"));
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        runtime.block_on(colorer.color(key("src/a.rs"), diff.clone(), Priority::Urgent));
        colors.poll();
        let spans = colors.spans(&key("src/a.rs")).expect("colors for the diff");
        assert_eq!(spans.line_count(), diff.lines().len());
        assert!(kinds(spans, 2).contains(&TokenKind::Keyword));
    }

    #[test]
    fn ensure_colors_once_and_wakes_the_ui() {
        let ctx = egui::Context::default();
        let repaints = observe_repaints(&ctx);
        let mut colors = PatchColors::default();
        let diff =
            Arc::new(FileDiff::parse_unified(b"@@ -1,1 +1,1 @@\n-let a = 1;\n+let a = 2;\n"));
        let key = key("src/b.rs");
        colors.ensure(&ctx, &key, &diff);
        let first = colors.wait_finished(&repaints, &key).expect("colors for the diff");
        assert_eq!(first.line_count(), 3);
        assert!(ctx.has_requested_repaint(), "the worker asked for a frame");
        colors.ensure(&ctx, &key, &diff);
        assert!(
            colors.finished(&key).is_some_and(|again| Arc::ptr_eq(&again, &first)),
            "a second ensure keeps the colors"
        );
    }

    #[test]
    fn colors_read_back_as_written() {
        let body = "+let a = \"text\"; // note\n".repeat(40);
        let diff = FileDiff::parse_unified(format!("@@ -0,0 +1,40 @@\n{body}").as_bytes());
        let spans = patch_spans("a.rs", &diff);
        let bytes = encode(&spans).expect("spans fit");
        assert_eq!(decode(&bytes), Some(spans));
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None, "a cut file is no colors");
    }

    /// Colors worth keeping reach the disk tier, and a later coloring of that diff (after a
    /// restart) shows what is stored there instead of working it out again.
    #[test]
    fn large_colors_go_to_the_disk_tier_and_come_back() {
        let dir = cv_testkit::ScratchDir::new("patch-colors-cold");
        let cold = ColdTier::new(&dir.app_paths());
        let body = "+let a = 1;\n".repeat(COLD_MIN_LINES);
        let diff = Arc::new(FileDiff::parse_unified(
            format!("@@ -0,0 +1,{COLD_MIN_LINES} @@\n{body}").as_bytes(),
        ));
        let (key, ctx) = (key("src/cold.rs"), egui::Context::default());
        let repaints = observe_repaints(&ctx);
        let stored_is = |wanted: &LineSpans| {
            cold.flush();
            let stored = cold
                .read(COLD_KIND, COLD_VERSION, &cold_key(&key))
                .and_then(|bytes| decode(&bytes));
            assert_eq!(stored.as_ref(), Some(wanted), "the colors reach the disk tier");
        };
        let mut colors = PatchColors::default();
        colors.cold_worthy = Duration::ZERO;
        colors.use_cold(&cold);
        colors.ensure(&ctx, &key, &diff);
        let finished = colors.wait_finished(&repaints, &key).expect("colors");
        stored_is(&finished);

        let mut marked = LineSpans::default();
        for _ in 0..diff.lines().len() {
            marked.push_line(&[Span::new(0, TokenKind::Heading)]);
        }
        let expected = marked.clone();
        cold.write(COLD_KIND, COLD_VERSION, cold_key(&key), move || encode(&marked));
        stored_is(&expected);
        let mut restarted = PatchColors::default();
        restarted.use_cold(&cold);
        restarted.ensure(&ctx, &key, &diff);
        let shown = restarted.wait_finished(&repaints, &key).expect("colors");
        assert_eq!(*shown, expected, "the stored colors, not worked out again");
    }

    #[test]
    fn plain_files_finish_without_colors() {
        let ctx = egui::Context::default();
        let repaints = observe_repaints(&ctx);
        let mut colors = PatchColors::default();
        let diff = Arc::new(FileDiff::parse_unified(b"@@ -1,1 +1,1 @@\n-a\n+b\n"));
        let colorer = colors.colorer(&ctx);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let _in_runtime = runtime.enter();
        let load = std::pin::pin!(colorer.color(key("notes.zzqq"), diff, Priority::Urgent));
        let polled = load.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
        assert!(polled.is_ready(), "no grammar: the load does not wait, even for a busy pool");
        let finished = colors.wait_finished(&repaints, &key("notes.zzqq"));
        assert_eq!(finished.map(|spans| spans.line_count()), Some(0));
    }
}
