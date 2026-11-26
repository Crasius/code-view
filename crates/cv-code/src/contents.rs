//! The full old and new text of the shown file, with syntax spans: hidden lines for "expand" and
//! the colors painted over every row. Blobs load and highlight in the background and stream in;
//! finished sides are cached by blob id, so going back to a file is instant.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use review_git::{ChangedFile, GitMirror};

use cv_runtime::cache::memory::{BoundedCache, Holder};
use cv_services::Services;
use cv_syntax::blob_text::BlobText;
use cv_syntax::color_worker;
use cv_syntax::grammar;
use cv_syntax::highlight::{self, LineSpans, Span};

/// Files above either limit stay plain unless the user asks for colors.
pub const AUTO_HIGHLIGHT_MAX_BYTES: usize = 8 * 1024 * 1024;
pub const AUTO_HIGHLIGHT_MAX_LINES: usize = 150_000;
/// Finished file sides kept per pull request: our number, the texts and colors of the files shown
/// last, so going back to one is instant. Half of it (12 MB) is the largest side kept, above the
/// largest file colored without asking; one further away loads and colors again in the background.
const CACHE_BUDGET_BYTES: usize = 24 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Old,
    New,
}

#[derive(Clone)]
pub struct SideContents {
    pub text: Arc<BlobText>,
    pub spans: Arc<LineSpans>,
}

impl SideContents {
    /// Spans for line `index` when that line reads `text` (a diff line must match the blob line).
    pub fn spans_for(&self, index: usize, text: &str) -> Option<&[Span]> {
        let matches = self.text.line(index) == Some(text);
        if matches { self.spans.line(index) } else { None }
    }

    fn heap_size(&self) -> usize {
        self.text.heap_size() + self.spans.heap_size()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coloring {
    /// File contents are loading or highlighting has not started.
    Waiting,
    Running {
        done: usize,
        total: usize,
    },
    Done,
    /// Too large to color automatically; `bytes` is the larger side.
    Skipped {
        bytes: usize,
    },
}

pub struct FileContents {
    pub old: Option<SideContents>,
    pub new: Option<SideContents>,
    pub coloring: Coloring,
    pub error: Option<String>,
}

impl FileContents {
    pub fn side(&self, side: Side) -> Option<&SideContents> {
        match side {
            Side::Old => self.old.as_ref(),
            Side::New => self.new.as_ref(),
        }
    }

    fn side_mut(&mut self, side: Side) -> &mut Option<SideContents> {
        match side {
            Side::Old => &mut self.old,
            Side::New => &mut self.new,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    side: Side,
    oid: String,
    path: String,
}

impl Request {
    fn cache_key(&self) -> String {
        format!("{}:{}", self.oid, self.path)
    }
}

enum Message {
    Text { side: Side, text: Arc<BlobText> },
    Spans { side: Side, chunk: LineSpans },
    Finished { side: Side },
    Skipped { bytes: usize },
    Failed(String),
}

struct Current {
    requests: Vec<Request>,
    force: bool,
    contents: FileContents,
    receiver: Option<Receiver<Message>>,
    /// Sides whose highlighting finished, waiting for the other side before caching.
    finished: Vec<Side>,
    _cancel: Cancel,
}

/// Set once the file is no longer shown, so its queued or running coloring stops right away.
#[derive(Default)]
struct Cancel(Arc<AtomicBool>);

impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct ContentsLoader {
    current: Option<Current>,
    cache: ContentsCache,
}

impl ContentsLoader {
    pub fn current(&self) -> Option<&FileContents> {
        self.current.as_ref().map(|current| &current.contents)
    }

    /// Starts loading `file`'s blobs unless they are already loaded or loading.
    /// `force` colors files above the automatic size limit.
    pub fn ensure(
        &mut self,
        file: &ChangedFile,
        git: &Arc<GitMirror>,
        services: &Services,
        force: bool,
    ) {
        let requests = requests_for(file);
        let is_current = self
            .current
            .as_ref()
            .is_some_and(|current| current.requests == requests && current.force == force);
        if is_current {
            return;
        }
        let mut contents =
            FileContents { old: None, new: None, coloring: Coloring::Waiting, error: None };
        let mut missing = Vec::new();
        for request in &requests {
            match self.cache.get(&request.cache_key()) {
                Some(cached) => *contents.side_mut(request.side) = Some(cached),
                None => missing.push(request.clone()),
            }
        }
        let cancel = Cancel::default();
        let receiver = if missing.is_empty() {
            contents.coloring = Coloring::Done;
            None
        } else {
            Some(start(missing, git.clone(), services, force, cancel.0.clone()))
        };
        self.current = Some(Current {
            requests,
            force,
            contents,
            receiver,
            finished: Vec::new(),
            _cancel: cancel,
        });
    }

    /// Forgets the current file so the next [`ensure`](Self::ensure) loads it again.
    pub fn retry(&mut self) {
        self.current = None;
    }

    /// Applies what the background work delivered since the last frame.
    pub fn poll(&mut self) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        let Some(receiver) = current.receiver.take() else {
            return;
        };
        loop {
            match receiver.try_recv() {
                Ok(message) => apply(current, message, &mut self.cache),
                Err(TryRecvError::Empty) => {
                    current.receiver = Some(receiver);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    let is_unfinished = matches!(
                        current.contents.coloring,
                        Coloring::Waiting | Coloring::Running { .. }
                    );
                    if is_unfinished && current.contents.error.is_none() {
                        current.contents.error =
                            Some("Loading the file contents stopped unexpectedly.".into());
                    }
                    return;
                }
            }
        }
    }
}

fn requests_for(file: &ChangedFile) -> Vec<Request> {
    let mut requests = Vec::with_capacity(2);
    if file.has_new_blob() {
        requests.push(Request {
            side: Side::New,
            oid: file.new_oid.clone(),
            path: file.path.clone(),
        });
    }
    if file.has_old_blob() {
        requests.push(Request {
            side: Side::Old,
            oid: file.old_oid.clone(),
            path: file.old_path.clone().unwrap_or_else(|| file.path.clone()),
        });
    }
    requests
}

fn apply(current: &mut Current, message: Message, cache: &mut ContentsCache) {
    let contents = &mut current.contents;
    match message {
        Message::Text { side, text } => {
            *contents.side_mut(side) =
                Some(SideContents { text, spans: Arc::new(LineSpans::default()) });
        }
        Message::Spans { side, chunk } => {
            if let Some(side_contents) = contents.side_mut(side) {
                Arc::make_mut(&mut side_contents.spans).append(chunk);
            }
            let lines = |side: &Option<SideContents>| {
                side.as_ref()
                    .map_or((0, 0), |side| (side.spans.line_count(), side.text.line_count()))
            };
            let (old_done, old_total) = lines(&contents.old);
            let (new_done, new_total) = lines(&contents.new);
            contents.coloring =
                Coloring::Running { done: old_done + new_done, total: old_total + new_total };
        }
        Message::Finished { side } => {
            if let Some(side_contents) = contents.side_mut(side) {
                Arc::make_mut(&mut side_contents.spans).shrink_to_fit();
            }
            current.finished.push(side);
            let is_done = current.requests.iter().all(|request| {
                current.finished.contains(&request.side) || cache.contains(&request.cache_key())
            });
            if is_done {
                contents.coloring = Coloring::Done;
            }
            let request = current.requests.iter().find(|request| request.side == side);
            if let (Some(request), Some(side_contents)) = (request, contents.side(side)) {
                cache.insert(request.cache_key(), side_contents.clone());
            }
        }
        Message::Skipped { bytes } => contents.coloring = Coloring::Skipped { bytes },
        Message::Failed(message) => contents.error = Some(message),
    }
}

/// Loads the missing blobs (both at once), then queues their coloring.
fn start(
    requests: Vec<Request>,
    git: Arc<GitMirror>,
    services: &Services,
    force: bool,
    cancel: Arc<AtomicBool>,
) -> Receiver<Message> {
    let (sender, receiver) = mpsc::channel();
    let ctx = services.background.ctx();
    services.background.spawn(async move {
        let blob = |request: Option<&Request>| {
            let (git, oid) = (git.clone(), request.map(|request| request.oid.clone()));
            async move {
                match oid {
                    Some(oid) => Some(git.blob(&oid).await),
                    None => None,
                }
            }
        };
        let (first, second) = tokio::join!(blob(requests.first()), blob(requests.get(1)));
        let mut loaded = Vec::with_capacity(requests.len());
        for (request, blob) in requests.into_iter().zip([first, second].into_iter().flatten()) {
            match blob {
                Ok(bytes) => {
                    let text = Arc::new(BlobText::from_bytes(bytes));
                    if sender
                        .send(Message::Text { side: request.side, text: text.clone() })
                        .is_err()
                    {
                        return;
                    }
                    loaded.push((request, text));
                }
                Err(error) => {
                    // The receiver goes with the file view: nobody is left to tell.
                    let _file_closed = sender.send(Message::Failed(format!(
                        "Could not load the file contents: {error}"
                    )));
                    ctx.request_repaint();
                    return;
                }
            }
        }
        ctx.request_repaint();
        let largest = loaded.iter().map(|(_, text)| text.len()).max().unwrap_or(0);
        let most_lines = loaded.iter().map(|(_, text)| text.line_count()).max().unwrap_or(0);
        let is_too_large =
            largest > AUTO_HIGHLIGHT_MAX_BYTES || most_lines > AUTO_HIGHLIGHT_MAX_LINES;
        if is_too_large && !force {
            // The receiver goes with the file view: nobody is left to tell.
            let _file_closed = sender.send(Message::Skipped { bytes: largest });
            ctx.request_repaint();
            return;
        }
        let send = move |message| {
            let is_open = sender.send(message).is_ok();
            if is_open {
                ctx.request_repaint();
            }
            is_open
        };
        color_worker::submit(move |_| highlight_sides(&loaded, &cancel, send));
    });
    receiver
}

/// Queues both sides on the reader threads at once with the shared compiled grammar, so the
/// deleted lines do not wait for the whole new version: each side's reader thread `send`s its
/// colors as it reads them. Returns once both sides are read or stopped, so the next file in the
/// color queue starts after this one.
fn highlight_sides(
    loaded: &[(Request, Arc<BlobText>)],
    cancel: &Arc<AtomicBool>,
    send: impl Fn(Message) -> bool + Clone + Send + 'static,
) {
    let (is_reading, all_read) = mpsc::channel::<()>();
    for (request, text) in loaded {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let side = request.side;
        let Some(grammar) = grammar::for_path(&request.path, text.line(0).unwrap_or_default())
        else {
            send(Message::Finished { side });
            continue;
        };
        let working = grammar::compiled(grammar);
        let (send_spans, cancel) = (send.clone(), cancel.clone());
        let deliver = move |chunk| {
            !cancel.load(Ordering::Relaxed) && send_spans(Message::Spans { side, chunk })
        };
        let (send, is_reading) = (send.clone(), is_reading.clone());
        highlight::start_highlight(text, grammar.syntax, &working, deliver, move || {
            send(Message::Finished { side });
            drop(is_reading);
        });
    }
    drop(is_reading);
    // Nothing is ever sent: this returns once each side's read, which holds a sender, is over.
    let _all_read = all_read.recv();
}

/// Finished file sides by blob and path, bounded by bytes like the diff cache.
struct ContentsCache(BoundedCache<String, SideContents>);

impl Default for ContentsCache {
    fn default() -> Self {
        Self(BoundedCache::new(Holder::FileTexts, CACHE_BUDGET_BYTES))
    }
}

impl ContentsCache {
    fn get(&mut self, key: &str) -> Option<SideContents> {
        self.0.get(key).cloned()
    }

    fn contains(&self, key: &str) -> bool {
        self.0.peek(key).is_some()
    }

    fn insert(&mut self, key: String, contents: SideContents) {
        let bytes = contents.heap_size();
        self.0.insert(key, contents, bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn loaded(side: Side, source: String) -> (Request, Arc<BlobText>) {
        let request = Request { side, oid: format!("{side:?}"), path: "a.rs".into() };
        (request, Arc::new(BlobText::from_bytes(source.into_bytes())))
    }

    /// Colors a long new side and a short old one, in the order `requests_for` asks for them.
    /// Returns the sides in the order they finished and the threads that sent the colors.
    fn color_sides() -> (Vec<Side>, Vec<String>) {
        let new = "fn read(a: u32) -> u32 { let b = a * 2; b + 1 }\n".repeat(20_000);
        let sides = [loaded(Side::New, new), loaded(Side::Old, "fn a() {}\n".into())];
        let (sender, messages) = mpsc::channel();
        let threads = Arc::new(Mutex::new(Vec::new()));
        let senders = threads.clone();
        highlight_sides(&sides, &Arc::default(), move |message| {
            let name = std::thread::current().name().unwrap_or_default().to_owned();
            senders.lock().expect("the thread list").push(name);
            sender.send(message).is_ok()
        });
        let finished = messages.try_iter().filter_map(|message| match message {
            Message::Finished { side } => Some(side),
            _ => None,
        });
        let threads = threads.lock().expect("the thread list").clone();
        (finished.collect(), threads)
    }

    /// The sides read on the kept reader threads, not on a thread of their own per file, and the
    /// short old side's colors are all sent while the long new side is still reading.
    #[test]
    fn colors_both_sides_on_the_reader_threads_short_side_first() {
        let (finished, threads) = color_sides();
        assert_eq!(finished, [Side::Old, Side::New]);
        let others: Vec<_> =
            threads.iter().filter(|name| !name.starts_with("syntax-colors-reader")).collect();
        assert!(others.is_empty(), "colors sent from {others:?}");
    }
}
