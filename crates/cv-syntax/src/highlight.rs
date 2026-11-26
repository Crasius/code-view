//! Whole-file syntax highlighting into compact per-line spans. Grammars need every line in order,
//! so the full blob is parsed (in the background), and finished lines are handed over in chunks
//! so long files color from the top while the rest is still parsing.
//!
//! Parsing is the whole cost (the regex searches of the grammar: 15,000 lines of TypeScript take
//! seconds on one thread), so a long text reads in parts at once, see [`read_lines`]. The threads
//! that read are kept ones, never started per text: each thread that searched with a grammar adds
//! its own search caches to it (see [`READERS`]).

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use cv_base::platform::heap;
use cv_base::theme::syntax::TokenKind;
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference};

use crate::blob_text::BlobText;
use crate::grammar::{self, Classifier, Grammar, WorkingSet};
use crate::pool::{Pool, Priority};

/// Longer lines (minified code, data blobs) stay plain: regex grammars can take seconds on them.
pub const MAX_LINE_BYTES: usize = 5_000;
/// Lines per delivered chunk: small enough that the top of a file colors at once and a file the
/// user left stops coloring quickly.
const CHUNK_LINES: usize = 250;
/// Lines per part below which a text reads on one thread: our number. A part's thread and its
/// fresh grammar state cost little next to reading a thousand lines (70 ms of Rust, 300 ms of
/// TypeScript).
const MIN_PART_LINES: usize = 1_000;
/// Parts a text reads in at most: our number, so both sides of a file and two diffs coloring at
/// once still leave cores to the rest of the app.
const MAX_PARTS: usize = 8;
/// A part keeps its reader's state every this many lines, where the reader of the lines before
/// checks whether it reached the same state.
const CHECK_EVERY: usize = 16;
/// Lines read before the other parts start: our number, the screen that shows first, so its
/// colors come as fast as without parts (all threads at once slow each one down).
const HEAD_LINES: usize = 250;
const OFFSET_BITS: u32 = 27;
const OFFSET_MASK: u32 = (1 << OFFSET_BITS) - 1;

/// Where a token starts in its line and what it is, packed into 4 bytes. A token runs until the
/// next span; text before a line's first span is plain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span(u32);

impl Span {
    pub fn new(start: usize, kind: TokenKind) -> Self {
        Self(((kind as u32) << OFFSET_BITS) | (start as u32 & OFFSET_MASK))
    }

    pub fn start(self) -> usize {
        (self.0 & OFFSET_MASK) as usize
    }

    pub fn kind(self) -> TokenKind {
        TokenKind::from_index((self.0 >> OFFSET_BITS) as u8)
    }
}

/// Spans of consecutive lines from line 0, growing as chunks arrive.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LineSpans {
    /// End of each line's spans in `spans`.
    ends: Vec<u32>,
    spans: Vec<Span>,
}

impl LineSpans {
    pub fn line_count(&self) -> usize {
        self.ends.len()
    }

    /// Spans of line `index` (0-based), `None` while that line is not highlighted yet.
    pub fn line(&self, index: usize) -> Option<&[Span]> {
        let end = *self.ends.get(index)? as usize;
        let start = index.checked_sub(1).map_or(0, |previous| self.ends[previous] as usize);
        Some(&self.spans[start..end])
    }

    pub fn push_line(&mut self, spans: &[Span]) {
        self.spans.extend_from_slice(spans);
        self.ends.push(self.spans.len() as u32);
    }

    pub fn append(&mut self, chunk: LineSpans) {
        let offset = self.spans.len() as u32;
        self.spans.extend(chunk.spans);
        self.ends.extend(chunk.ends.into_iter().map(|end| end + offset));
    }

    pub fn shrink_to_fit(&mut self) {
        self.ends.shrink_to_fit();
        self.spans.shrink_to_fit();
    }

    pub fn heap_size(&self) -> usize {
        (self.ends.capacity() + self.spans.capacity()) * std::mem::size_of::<u32>()
    }
}

/// The threads that read with the grammars: the texts [`highlight`] is given (first, as
/// [`Priority::Urgent`]) and the parts of long texts and diffs (see [`read_lines`]). They are
/// kept, not started per text: the regex engine keeps a search cache for each thread that
/// searched with a regex, for as long as the regex lives (about 3 MB per thread for TypeScript's,
/// with the caches review-grammars caps), so fresh threads per part grew a compiled set with every
/// text read. A text reads in at most [`MAX_PARTS`] parts, so that many threads.
static READERS: Pool = Pool::new("syntax-colors-reader", MAX_PARTS);

/// Highlights every line of `text`, handing each finished chunk to `deliver`; stops as soon as
/// `deliver` returns false (nobody is waiting for the result anymore). The text is read on a
/// reader thread (see [`start_highlight`]); this thread waits for the chunks.
pub fn highlight(
    text: &BlobText,
    syntax: &'static SyntaxReference,
    working: &Arc<WorkingSet>,
    mut deliver: impl FnMut(LineSpans) -> bool,
) {
    let (sender, chunks) = mpsc::channel();
    start_highlight(text, syntax, working, move |chunk| sender.send(chunk).is_ok(), || {});
    for chunk in chunks {
        if !deliver(chunk) {
            return;
        }
    }
}

/// [`highlight`] without waiting: queues `text` for a reader thread ([`READERS`]), which hands each
/// finished chunk to `deliver` itself and calls `finish` once `deliver` took the last one. The text
/// is read from a copy, which costs little next to reading it.
pub fn start_highlight(
    text: &BlobText,
    syntax: &'static SyntaxReference,
    working: &Arc<WorkingSet>,
    mut deliver: impl FnMut(LineSpans) -> bool + Send + 'static,
    finish: impl FnOnce() + Send + 'static,
) {
    let lines = (0..text.line_count()).filter_map(|index| text.line(index));
    let copy: String = lines.flat_map(|line| [line, "\n"]).collect();
    let text = Arc::new(BlobText::from_bytes(copy.into_bytes()));
    let source = TextSource { text, syntax, working: working.clone() };
    READERS.submit(Priority::Urgent, move || {
        let mut chunk = LineSpans::default();
        let count = source.text.line_count();
        let is_read = read_lines(TextReader::new(&source), count, |spans| {
            chunk.push_line(spans);
            chunk.line_count() < CHUNK_LINES || deliver(std::mem::take(&mut chunk))
        });
        if is_read && deliver(chunk) {
            finish();
        }
    });
}

/// A text and the grammar it reads with, for the threads that read it.
#[derive(Clone)]
struct TextSource {
    text: Arc<BlobText>,
    syntax: &'static SyntaxReference,
    working: Arc<WorkingSet>,
}

/// The lines of a text, read one after the other by a [`LineReader`].
struct TextReader<'a> {
    source: &'a TextSource,
    lines: LineHighlighter<'a>,
}

impl<'a> TextReader<'a> {
    fn new(source: &'a TextSource) -> Self {
        Self { source, lines: LineHighlighter::new(source.syntax, &source.working) }
    }
}

impl LineReader for TextReader<'_> {
    type State = GrammarState;
    type Parts = TextSource;

    fn parts(&self) -> TextSource {
        self.source.clone()
    }

    fn read(&mut self, index: usize, spans: &mut Vec<Span>) {
        self.lines.line(self.source.text.line(index).unwrap_or_default(), spans);
    }

    fn state(&self) -> GrammarState {
        self.lines.state()
    }

    fn restore(&mut self, state: &GrammarState) {
        self.lines.restore(state);
    }

    fn is_in(&self, state: &GrammarState) -> bool {
        self.lines.is_in(state)
    }

    fn start_indent(&self, index: usize) -> Option<usize> {
        start_indent(self.source.text.line(index)?)
    }
}

impl ReadParts for TextSource {
    type State = GrammarState;

    fn read_part(
        &self,
        state: Option<&GrammarState>,
        lines: Range<usize>,
        stop: &AtomicBool,
    ) -> Part<GrammarState> {
        let mut reader = TextReader::new(self);
        if let Some(state) = state {
            reader.restore(state);
        }
        read_part(reader, lines, stop)
    }
}

/// Reads numbered lines in order, each in the state the lines before it left.
pub trait LineReader: Send + Sized {
    /// What decides how the next line reads.
    type State: Clone + Send + 'static;
    /// What reads this reader's lines on the reader threads.
    type Parts: ReadParts<State = Self::State>;

    /// The lines and grammar of this reader for the reader threads, which outlive its borrows.
    fn parts(&self) -> Self::Parts;

    /// The spans of line `index` into `spans`; called for each line after the one before it.
    fn read(&mut self, index: usize, spans: &mut Vec<Span>);

    fn state(&self) -> Self::State;

    /// Takes over `state`, as if this reader had read the lines that led there.
    fn restore(&mut self, state: &Self::State);

    /// Whether this reader is in `state`, so it reads every following line as the reader that was
    /// in it does.
    fn is_in(&self, state: &Self::State) -> bool;

    /// How far line `index` is indented, when a part may start there (see [`start_indent`]).
    fn start_indent(&self, index: usize) -> Option<usize>;
}

/// Reads a part of a [`LineReader`]'s lines on a reader thread.
pub trait ReadParts: Send + Sync + 'static {
    type State: Clone + Send + 'static;

    /// Reads `lines` (see [`read_part`]), starting in `state` or as at the start of the text
    /// without one.
    fn read_part(
        &self,
        state: Option<&Self::State>,
        lines: Range<usize>,
        stop: &AtomicBool,
    ) -> Part<Self::State>;
}

/// How far `line` is indented (a tab counts as one), `None` for a line a part should not start
/// at: a blank one, or one that closes a block, which a part would read as closing its own.
pub fn start_indent(line: &str) -> Option<usize> {
    let code = line.trim_start();
    let is_closing = code.starts_with(['}', ')', ']']);
    (!code.is_empty() && !is_closing).then(|| line.len() - code.len())
}

/// Reads lines `0..count` with `reader`, handing each line's spans to `each` in order; stops as
/// soon as `each` returns false. Returns whether every line was read.
///
/// A long text reads in parts at once (see [`MIN_PART_LINES`]): the first on this thread, the
/// others on the reader threads ([`READERS`]), queued once the first [`HEAD_LINES`] are read and
/// handed on. A part no reader thread started by the time the lines before it are read is read
/// here, in order. A part's lines count from the line where the reader of the lines before it is
/// in the state the part's reader was in there: from then on both read every line alike, so the
/// result is the one reading in order gives, not a guess. Up to that line the reader before reads
/// on; it reads the whole part when the states never meet.
///
/// Where a part starts decides how soon they meet. It starts at the least indented line near its
/// share of the text, in the state the reader had at the last line of the same indent among the
/// first lines (as if the text started there when there was none): at the top level of a file of
/// declarations, and inside the block of a file that is one long block (a namespace, a test
/// suite), where a fresh state would never meet the lines before.
pub fn read_lines<R: LineReader>(
    reader: R,
    count: usize,
    each: impl FnMut(&[Span]) -> bool,
) -> bool {
    let threads = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let parts = (count / MIN_PART_LINES).clamp(1, MAX_PARTS.min(threads));
    read_in_parts(reader, count, parts, HEAD_LINES, each)
}

/// What a reader thread read of a part: the spans of its lines from `start`, and its reader's
/// state every [`CHECK_EVERY`] lines (by line). `end` is the reader's state once it read the
/// whole part.
pub struct Part<S> {
    start: usize,
    spans: LineSpans,
    checks: Vec<(usize, S)>,
    end: Option<S>,
}

/// A part queued for the reader threads.
struct Queued<S> {
    /// A reader thread started it, or the reader of the lines before took it back.
    is_taken: Arc<AtomicBool>,
    read: Receiver<Part<S>>,
}

impl<S> Queued<S> {
    /// What a reader thread read, waiting for it to finish; `None` when no reader thread started
    /// the part (it is not started after this) or its thread failed.
    fn take(self) -> Option<Part<S>> {
        if !self.is_taken.swap(true, Ordering::AcqRel) {
            return None;
        }
        self.read.recv().ok()
    }
}

/// Queues lines `lines` for the reader threads, read by `parts` from `state`.
fn queue_part<P: ReadParts>(
    parts: &Arc<P>,
    state: Option<P::State>,
    lines: Range<usize>,
    stop: &Arc<AtomicBool>,
) -> Queued<P::State> {
    let (sender, read) = mpsc::sync_channel(1);
    let is_taken = Arc::new(AtomicBool::new(false));
    let (parts, stop, taken) = (parts.clone(), stop.clone(), is_taken.clone());
    READERS.submit(Priority::Queued, move || {
        if stop.load(Ordering::Relaxed) || taken.swap(true, Ordering::AcqRel) {
            return;
        }
        let _nobody_waits = sender.send(parts.read_part(state.as_ref(), lines, &stop));
    });
    Queued { is_taken, read }
}

/// [`read_lines`] in `parts` parts after the first `head` lines.
pub fn read_in_parts<R: LineReader>(
    reader: R,
    count: usize,
    parts: usize,
    head: usize,
    each: impl FnMut(&[Span]) -> bool,
) -> bool {
    let mut in_order = InOrder { reader, each, next: 0, spans: Vec::new(), is_stopped: false };
    let head = head.min(count);
    // By indent, the state at the last line of that indent.
    let mut states = BTreeMap::new();
    for index in 0..head {
        if let Some(indent) = in_order.reader.start_indent(index) {
            states.insert(indent, in_order.reader.state());
        }
        if !in_order.read_to(index + 1) {
            return false;
        }
    }
    let starts = part_starts(&in_order.reader, count, parts, head);
    // One part: the reader threads get nothing to read.
    if starts.len() <= 2 {
        return in_order.read_to(count);
    }
    let (readers, stop) = (Arc::new(in_order.reader.parts()), Arc::new(AtomicBool::new(false)));
    let later = starts[1..].windows(2).map(|range| {
        let (start, end) = (range[0], range[1]);
        let state = in_order.reader.start_indent(start).and_then(|indent| states.get(&indent));
        Some(queue_part(&readers, state.cloned(), start..end, &stop))
    });
    let later: Vec<_> = later.collect();
    for (queued, &end) in std::iter::once(None).chain(later).zip(&starts[1..]) {
        let part = queued.and_then(Queued::take);
        if !part.is_some_and(|part| in_order.join(part)) && !in_order.read_to(end) {
            stop.store(true, Ordering::Relaxed);
            return false;
        }
    }
    true
}

/// Where each of `parts` parts of lines `0..count` starts, then `count`: the least indented line
/// (the nearest of them) within a quarter part of an even share, never inside the first `head`.
fn part_starts<R: LineReader>(reader: &R, count: usize, parts: usize, head: usize) -> Vec<usize> {
    let reach = count / parts / 4;
    let mut starts = vec![0];
    for part in 1..parts {
        let even = part * count / parts;
        let previous = starts.last().copied().unwrap_or_default();
        let earliest = even.saturating_sub(reach).max(head).max(previous);
        let near = earliest..(even + reach).min(count);
        let least = near
            .filter_map(|index| Some((reader.start_indent(index)?, index.abs_diff(even), index)))
            .min()
            .map_or(even.max(earliest), |(_, _, index)| index);
        starts.push(least);
    }
    starts.push(count);
    starts
}

/// The reader of the lines in order, handing each line's spans on.
struct InOrder<R, F> {
    reader: R,
    each: F,
    /// The line to read next.
    next: usize,
    spans: Vec<Span>,
    /// `each` asked to stop.
    is_stopped: bool,
}

impl<R: LineReader, F: FnMut(&[Span]) -> bool> InOrder<R, F> {
    /// Reads on up to line `end`; false once `each` asked to stop.
    fn read_to(&mut self, end: usize) -> bool {
        while self.next < end && !self.is_stopped {
            self.reader.read(self.next, &mut self.spans);
            self.is_stopped = !(self.each)(&self.spans);
            self.next += 1;
        }
        !self.is_stopped
    }

    /// Reads on into `part` until this reader is in the state the part's reader was in at the same
    /// line, then hands on the part's lines from there and goes on as its reader. False when the
    /// states never met (the reader stopped at the part's last check) or `each` asked to stop.
    fn join(&mut self, part: Part<R::State>) -> bool {
        let Some(end_state) = part.end else {
            return false;
        };
        let met =
            part.checks.iter().find(|(at, state)| self.read_to(*at) && self.reader.is_in(state));
        let Some(&(at, _)) = met else {
            return false;
        };
        let end = part.start + part.spans.line_count();
        for line in at..end {
            if !(self.each)(part.spans.line(line - part.start).unwrap_or_default()) {
                self.is_stopped = true;
                return false;
            }
        }
        self.reader.restore(&end_state);
        self.next = end;
        true
    }
}

/// Reads `lines` with `reader`, keeping its state every [`CHECK_EVERY`] lines from the first;
/// gives up once `stop` is set.
pub fn read_part<R: LineReader>(
    mut reader: R,
    lines: Range<usize>,
    stop: &AtomicBool,
) -> Part<R::State> {
    let mut part =
        Part { start: lines.start, spans: LineSpans::default(), checks: Vec::new(), end: None };
    let mut spans = Vec::new();
    for index in lines {
        if stop.load(Ordering::Relaxed) {
            return part;
        }
        if (index - part.start).is_multiple_of(CHECK_EVERY) {
            part.checks.push((index, reader.state()));
        }
        reader.read(index, &mut spans);
        part.spans.push_line(&spans);
    }
    part.end = Some(reader.state());
    part
}

/// Where a [`LineHighlighter`] is in its grammar: what decides how it reads the next line.
#[derive(Clone)]
pub struct GrammarState {
    parser: ParseState,
    stack: ScopeStack,
}

/// Highlights lines one after the other, each in the grammar state the previous ones left.
pub struct LineHighlighter<'a> {
    syntax: &'a SyntaxReference,
    working: &'a WorkingSet,
    classifier: Classifier,
    parser: ParseState,
    stack: ScopeStack,
    buffer: String,
}

impl<'a> LineHighlighter<'a> {
    pub fn new(syntax: &'a SyntaxReference, working: &'a WorkingSet) -> Self {
        Self {
            syntax,
            working,
            classifier: Classifier::default(),
            parser: ParseState::new(syntax),
            stack: ScopeStack::new(),
            buffer: String::new(),
        }
    }

    /// Takes over `state`, as if this highlighter had read the lines that led there.
    pub fn restore(&mut self, state: &GrammarState) {
        self.parser.clone_from(&state.parser);
        self.stack.clone_from(&state.stack);
    }

    pub fn state(&self) -> GrammarState {
        GrammarState { parser: self.parser.clone(), stack: self.stack.clone() }
    }

    /// Whether this highlighter is in `state`, so it reads every following line alike.
    pub fn is_in(&self, state: &GrammarState) -> bool {
        self.parser == state.parser && self.stack == state.stack
    }

    /// Forgets the state: the next line is read as if it started the file.
    pub fn restart(&mut self) {
        self.parser = ParseState::new(self.syntax);
        self.stack = ScopeStack::new();
    }

    /// Takes `other`'s grammar state, as if it had read the same lines.
    pub fn follow(&mut self, other: &Self) {
        self.parser.clone_from(&other.parser);
        self.stack.clone_from(&other.stack);
    }

    /// Whether `other` is in the same grammar state, so both would read the next line alike.
    pub fn is_in_step_with(&self, other: &Self) -> bool {
        self.parser == other.parser && self.stack == other.stack
    }

    /// The spans of `line` (without its line break) into `spans`.
    pub fn line(&mut self, line: &str, spans: &mut Vec<Span>) {
        spans.clear();
        if line.len() > MAX_LINE_BYTES {
            return;
        }
        self.buffer.clear();
        self.buffer.push_str(line);
        self.buffer.push('\n');
        let Ok(operations) = self.parser.parse_line(&self.buffer, &self.working.set) else {
            return;
        };
        // Each operation changes the scope stack at its offset; the text before it has the previous stack's kind.
        let mut position = 0;
        for (offset, operation) in operations {
            if offset > position && position < line.len() {
                push_merged(spans, position, self.classifier.kind(self.stack.as_slice()));
            }
            if self.stack.apply(&operation).is_err() {
                break;
            }
            position = position.max(offset);
        }
        if position < line.len() {
            push_merged(spans, position, self.classifier.kind(self.stack.as_slice()));
        }
    }
}

/// Code in the shapes most languages share: comments, strings, numbers, declarations, calls,
/// blocks, generics, attributes, tags. Highlighting it compiles the regexes most real code needs,
/// so the first file in a language colors without waiting for them.
const WARM_UP: &str = r#"// line comment
/* block comment */
# hash comment
/// doc comment
import { name, type Other } from "./module";
use std::collections::HashMap;
using System.Collections.Generic;
package main
#include <stdio.h>
@Attribute(value = 1)
#[derive(Debug)]
[Serializable]
export default class Name<T extends Base> extends Base implements Other {
    private readonly field: Map<string, number> = new Map();
    public static async method(argument: string, other?: number): Promise<void> {
        const text = `template ${argument} text`;
        let value = 'c' + "string \"escaped\"\n" + 0x1F + 3.5e2 + 42;
        if (value != null && other >= 0 || !flag) { return; } else { value += 1; }
        for (const item of items) { console.log(item?.name ?? [1, 2, 3]); }
        while (true) { break; }
        try { throw new Error("x"); } catch (error) { } finally { }
    }
}
pub fn function(argument: &str) -> Result<Vec<u8>, Error> { let mut x = vec![1]; x.push(2); Ok(x) }
public sealed class Name : IBase where T : class { public int Count { get; set; } = 0; }
def function(argument, *args, **kwargs):
    return f"value {argument}" if argument else None
func main() { fmt.Println("x") }
<div class="name" id='x'>text &amp; more</div>
<?xml version="1.0"?>
key: value
- item
{"key": [true, false, null, 1.5]}
[section]
name = "value"
SELECT name FROM table WHERE id = 1;
$variable = "text"
"#;

/// Highlights a little code of the usual shapes with `grammar`'s shared compiled set, once per
/// compiled set, so the regexes most code needs are compiled before the first file asks. Returns
/// the set, compiled as far as the warm-up got (another thread may still be at it).
pub fn warm_up(grammar: Grammar) -> Arc<WorkingSet> {
    let working = grammar::compiled(grammar);
    if grammar::begin_warm_up(grammar) {
        let before = heap::thread_net_bytes();
        warm_up_set(grammar.syntax, &working);
        let compiled = heap::thread_net_bytes().wrapping_sub(before);
        grammar::finish_warm_up(grammar, &working, usize::try_from(compiled).unwrap_or(0));
    }
    working
}

/// Runs the warm-up code through `syntax` in `working`.
pub fn warm_up_set(syntax: &SyntaxReference, working: &WorkingSet) {
    let mut lines = LineHighlighter::new(syntax, working);
    let mut spans = Vec::new();
    for line in WARM_UP.lines() {
        lines.line(line, &mut spans);
    }
}

fn push_merged(spans: &mut Vec<Span>, start: usize, kind: TokenKind) {
    let previous_kind = spans.last().map_or(TokenKind::Plain, |span: &Span| span.kind());
    if previous_kind != kind {
        spans.push(Span::new(start, kind));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::grammar;

    fn kinds_of(path: &str, source: &str) -> Vec<Vec<(String, TokenKind)>> {
        let text = BlobText::from_bytes(source.as_bytes().to_vec());
        let grammar = grammar::for_path(path, text.line(0).unwrap_or_default()).expect("grammar");
        let mut spans = LineSpans::default();
        highlight(&text, grammar.syntax, &grammar.working_set(), |chunk| {
            spans.append(chunk);
            true
        });
        (0..text.line_count())
            .map(|index| {
                let line = text.line(index).unwrap();
                let line_spans = spans.line(index).unwrap();
                tokens(line, line_spans)
            })
            .collect()
    }

    fn tokens(line: &str, spans: &[Span]) -> Vec<(String, TokenKind)> {
        let mut tokens = Vec::new();
        let first = spans.first().map_or(line.len(), |span| span.start());
        if first > 0 {
            tokens.push((line[..first].to_owned(), TokenKind::Plain));
        }
        for (index, span) in spans.iter().enumerate() {
            let end = spans.get(index + 1).map_or(line.len(), |next| next.start());
            tokens.push((line[span.start()..end].to_owned(), span.kind()));
        }
        tokens
    }

    fn kind_of(tokens: &[(String, TokenKind)], text: &str) -> TokenKind {
        tokens
            .iter()
            .find(|(token, _)| token.trim() == text)
            .map_or_else(|| panic!("no token {text:?} in {tokens:?}"), |(_, kind)| *kind)
    }

    #[test]
    fn packs_offset_and_kind() {
        let span = Span::new(1234, TokenKind::Function);
        assert_eq!((span.start(), span.kind()), (1234, TokenKind::Function));
    }

    #[test]
    fn colors_rust_like_vs_code() {
        let lines =
            kinds_of("a.rs", "// note\nfn main() {\n    let name = \"x\";\n    return 42;\n}\n");
        assert_eq!(kind_of(&lines[0], "// note"), TokenKind::Comment);
        assert_eq!(kind_of(&lines[1], "fn"), TokenKind::Keyword);
        assert_eq!(kind_of(&lines[1], "main"), TokenKind::Function);
        assert_eq!(kind_of(&lines[2], "\"x\""), TokenKind::String);
        assert_eq!(kind_of(&lines[3], "return"), TokenKind::Control);
        assert_eq!(kind_of(&lines[3], "42"), TokenKind::Number);
    }

    #[test]
    fn colors_typescript_like_vs_code() {
        let source = concat!(
            "import { Type } from \"./types\";\n",
            "export function check(node: Node): boolean {\n",
            "    if (node.kind === SyntaxKind.Identifier) return true;\n",
            "    const pattern = /a+b/g;\n",
            "}\n",
        );
        let lines = kinds_of("src/compiler/checker.ts", source);
        assert_eq!(kind_of(&lines[0], "import"), TokenKind::Control);
        assert_eq!(kind_of(&lines[0], "\"./types\""), TokenKind::String);
        assert_eq!(kind_of(&lines[1], "function"), TokenKind::Keyword);
        assert_eq!(kind_of(&lines[1], "check"), TokenKind::Function);
        assert_eq!(kind_of(&lines[1], "boolean"), TokenKind::Type);
        assert_eq!(kind_of(&lines[2], "if"), TokenKind::Control);
        assert_eq!(kind_of(&lines[2], "true"), TokenKind::Keyword);
        assert_eq!(kind_of(&lines[3], "const"), TokenKind::Keyword);
    }

    #[test]
    fn keeps_state_across_lines() {
        let lines = kinds_of("a.ts", "/* start\nstill comment */\nconst x = 1;\n");
        assert_eq!(lines[1], [("still comment */".to_owned(), TokenKind::Comment)]);
        assert_eq!(kind_of(&lines[2], "const"), TokenKind::Keyword);
    }

    /// The tokens of the first line of `source` that contains `needle`.
    fn line_with<'a>(
        lines: &'a [Vec<(String, TokenKind)>],
        source: &str,
        needle: &str,
    ) -> &'a [(String, TokenKind)] {
        let index = source
            .lines()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle:?}"));
        &lines[index]
    }

    /// The text pieces of each line of `source` with the scope stack the grammar for `path` gives
    /// each piece.
    fn scopes_of(path: &str, source: &str) -> Vec<Vec<(String, Vec<String>)>> {
        let grammar = grammar::for_path(path, "").expect("grammar");
        let working = grammar.working_set();
        let mut parser = ParseState::new(grammar.syntax);
        let mut stack = ScopeStack::new();
        let mut lines = Vec::new();
        for line in source.lines() {
            let text = format!(
                "{line}
"
            );
            let operations = parser.parse_line(&text, &working.set).expect("parses");
            let mut pieces = Vec::new();
            let mut position = 0;
            let ends = operations.iter().map(|(offset, _)| *offset).chain([text.len()]);
            let mut operations = operations.iter();
            for end in ends {
                if end > position {
                    let scopes = stack.as_slice().iter().map(|scope| scope.build_string());
                    pieces.push((text[position..end].to_owned(), scopes.collect()));
                    position = end;
                }
                if let Some((_, operation)) = operations.next() {
                    stack.apply(operation).expect("a valid scope operation");
                }
            }
            lines.push(pieces);
        }
        lines
    }

    /// Every `invalid` scope the grammar for `path` puts on `source`, with its line number.
    fn invalid_scopes(path: &str, source: &str) -> Vec<String> {
        let lines = scopes_of(path, source);
        let scopes = lines.iter().enumerate().flat_map(|(index, pieces)| {
            pieces
                .iter()
                .flat_map(move |(_, scopes)| scopes.iter().map(move |scope| (index, scope)))
        });
        scopes
            .filter(|(_, scope)| scope.starts_with("invalid"))
            .map(|(index, scope)| format!("line {}: {scope}", index + 1))
            .collect()
    }

    /// Markdown's C# fences and `.cs` files share one grammar: the same code gets the same scopes
    /// below the C# root scope.
    #[test]
    fn csharp_fences_in_markdown_read_like_csharp_files() {
        let code = concat!(
            "if (invoice.RecipientCompanyName is { Length: > 0 } companyName)
",
            "    column.Item().Text(companyName);
",
            "var json = \"\"\"
",
            "    { \"net\": 1 }
",
            "    \"\"\";
",
        );
        let below_root =
            |lines: Vec<Vec<(String, Vec<String>)>>| -> Vec<Vec<(String, Vec<String>)>> {
                let lines = lines.into_iter().map(|pieces| {
                    let pieces = pieces.into_iter().map(|(text, scopes)| {
                        let root = scopes.iter().position(|scope| scope == "source.cs");
                        let root = root.unwrap_or_else(|| panic!("{text:?} is not C#: {scopes:?}"));
                        (text, scopes[root + 1..].to_vec())
                    });
                    pieces.collect()
                });
                lines.collect()
            };
        let file = below_root(scopes_of("Invoice.cs", code));
        let markdown = scopes_of(
            "README.md",
            &format!(
                "# Title

```csharp
{code}```
"
            ),
        );
        let fence = below_root(markdown[3..3 + code.lines().count()].to_vec());
        assert_eq!(fence, file);
    }

    /// The owner's report: bat's 2019 C# grammar read the property patterns as invalid (red) and
    /// lost its place, so the lines after them came out red or uncolored.
    #[test]
    fn colors_csharp_property_patterns_and_the_lines_after() {
        let source = concat!(
            "namespace App;\n",
            "\n",
            "public sealed class Doc\n",
            "{\n",
            "    void Compose(IContainer container)\n",
            "    {\n",
            "        container.Column(column =>\n",
            "        {\n",
            "            if (invoice.RecipientCompanyName is { Length: > 0 } companyName)\n",
            "                column.Item().PaddingTop(4).Text(companyName).Bold();\n",
            "\n",
            "            column.Item().PaddingTop(2).Text(invoice.RecipientLegalName);\n",
            "            column.Item().Text(invoice.RecipientAddressLine1);\n",
            "\n",
            "            if (invoice.RecipientAddressLine2 is { Length: > 0 } addressLine2)\n",
            "                column.Item().Text(addressLine2);\n",
            "\n",
            "            column.Item().Text($\"{invoice.RecipientPostalCode} {invoice.RecipientCityName}\");\n",
            "            column.Item().Text(IsoCountryCodes.NameOf(invoice.RecipientCountryCode));\n",
            "        });\n",
            "    }\n",
            "}\n",
        );
        assert_eq!(invalid_scopes("Invoice.cs", source), Vec::<String>::new());
        let lines = kinds_of("Invoice.cs", source);
        let line = |needle| line_with(&lines, source, needle);
        let first_if = line("RecipientCompanyName is");
        assert_eq!(kind_of(first_if, "if"), TokenKind::Control);
        assert_eq!(kind_of(first_if, "Length"), TokenKind::Variable);
        assert_eq!(kind_of(first_if, "0"), TokenKind::Number);
        assert_eq!(kind_of(first_if, "companyName"), TokenKind::Variable);
        let bold = line(".Bold()");
        assert_eq!(kind_of(bold, "Text"), TokenKind::Function);
        assert_eq!(kind_of(bold, "companyName"), TokenKind::Variable);
        assert_eq!(kind_of(line("RecipientLegalName"), "RecipientLegalName"), TokenKind::Variable);
        assert_eq!(kind_of(line("RecipientAddressLine1"), "Text"), TokenKind::Function);
        assert_eq!(kind_of(line(".Text(addressLine2)"), "addressLine2"), TokenKind::Variable);
        assert_eq!(kind_of(line("RecipientPostalCode"), "$\""), TokenKind::String);
        let country = line("IsoCountryCodes");
        assert_eq!(kind_of(country, "IsoCountryCodes"), TokenKind::Variable);
        assert_eq!(kind_of(country, "NameOf"), TokenKind::Function);
        assert_eq!(line("});"), [("        });".to_owned(), TokenKind::Plain)]);
    }

    /// Records, primary constructors, `required`, `file`, collection expressions, raw strings,
    /// switch expressions with relational, property, list and tuple patterns.
    #[test]
    fn colors_modern_csharp() {
        let source = concat!(
            "using Alias = System.Collections.Generic.Dictionary<string, int>;\n",
            "\n",
            "namespace Billing.Invoices;\n",
            "\n",
            "/// <summary>An invoice <see cref=\"Money\"/> line.</summary>\n",
            "public sealed record InvoiceLine(string Description, decimal Amount, int Quantity = 1);\n",
            "\n",
            "public readonly record struct Money(decimal Value, string Currency);\n",
            "\n",
            "file sealed class Totals(IReadOnlyList<InvoiceLine> lines) : ITotals\n",
            "{\n",
            "    public required string Currency { get; init; }\n",
            "    private readonly List<InvoiceLine> _lines = [.. lines];\n",
            "    public decimal Net => _lines.Sum(line => line.Amount * line.Quantity);\n",
            "    public event EventHandler? Changed;\n",
            "    [GeneratedRegex(\"^[0-9]+$\")]\n",
            "    private static partial Regex Digits();\n",
            "    private const string Query = @\"SELECT name FROM invoices\";\n",
            "\n",
            "    public static string Describe(object? value) => value switch\n",
            "    {\n",
            "        null => \"none\",\n",
            "        int n and > 0 and < 10 => $\"small {n}\",\n",
            "        string { Length: > 0 } text => text,\n",
            "        [var first, .., var last] => $\"{first}..{last}\",\n",
            "        InvoiceLine { Amount: >= 100m, Quantity: not 0 } => \"big\",\n",
            "        (int x, int y) => (x + y).ToString(),\n",
            "        _ => \"other\",\n",
            "    };\n",
            "\n",
            "    public async Task<int> SaveAsync(CancellationToken cancellationToken = default)\n",
            "    {\n",
            "        var json = \"\"\"\n",
            "            { \"net\": 1 }\n",
            "            \"\"\";\n",
            "        var (net, count) = (Net, Count);\n",
            "        if (net is > 0 and < 1_000_000 && count is not 0)\n",
            "        {\n",
            "            await using var stream = File.OpenWrite(\"totals.json\");\n",
            "        }\n",
            "        int[] numbers = [1, 2, 3];\n",
            "        var text = $\"{net:N2} and {count,5}\";\n",
            "        if (numbers is [var head, .. var rest] && rest is [_, > 1, ..])\n",
            "            return head;\n",
            "        return numbers is [1, ..] ? count : 0;\n",
            "    }\n",
            "}\n",
            "\n",
            "public enum Kind : byte { None = 0, Some }\n",
        );
        assert_eq!(invalid_scopes("Totals.cs", source), Vec::<String>::new());
        let lines = kinds_of("Totals.cs", source);
        let line = |needle| line_with(&lines, source, needle);
        let record = line("record InvoiceLine");
        assert_eq!(kind_of(record, "record"), TokenKind::Keyword);
        assert_eq!(kind_of(record, "InvoiceLine"), TokenKind::Type);
        assert_eq!(kind_of(record, "Amount"), TokenKind::Variable);
        let class = line("file sealed class");
        assert_eq!(kind_of(class, "file"), TokenKind::Keyword);
        assert_eq!(kind_of(class, "Totals"), TokenKind::Type);
        assert_eq!(kind_of(line("required"), "required"), TokenKind::Keyword);
        assert_eq!(kind_of(line("Describe"), "Describe"), TokenKind::Function);
        assert_eq!(kind_of(line("Describe"), "switch"), TokenKind::Control);
        assert_eq!(kind_of(line("int n and"), "and"), TokenKind::Operator);
        assert_eq!(kind_of(line("{ Length: > 0 } text"), "text"), TokenKind::Variable);
        let list = line("[var first");
        assert_eq!(kind_of(list, "first"), TokenKind::Variable);
        assert_eq!(kind_of(list, ".."), TokenKind::Operator);
        let property = line("InvoiceLine { Amount");
        assert_eq!(kind_of(property, "InvoiceLine"), TokenKind::Type);
        assert_eq!(kind_of(property, "Amount"), TokenKind::Variable);
        assert_eq!(kind_of(property, "\"big\""), TokenKind::String);
        assert_eq!(kind_of(line("{ \"net\": 1 }"), "{ \"net\": 1 }"), TokenKind::String);
        assert_eq!(kind_of(line("SaveAsync"), "SaveAsync"), TokenKind::Function);
        assert_eq!(kind_of(line("await using"), "OpenWrite"), TokenKind::Function);
        assert_eq!(kind_of(line("[var head"), "rest"), TokenKind::Variable);
        assert_eq!(kind_of(line("enum Kind"), "None"), TokenKind::Constant);
        assert_eq!(kind_of(line("SELECT"), "SELECT"), TokenKind::Keyword, "embedded SQL");
        assert_eq!(kind_of(line("GeneratedRegex"), "\"^[0-9]+$\""), TokenKind::String);
    }

    #[test]
    fn highlights_powershell() {
        let lines =
            kinds_of("build.ps1", "# build\n$name = \"App $version\"\nGet-ChildItem -Path $name\n");
        assert_eq!(kind_of(&lines[0], "# build"), TokenKind::Comment);
        assert_eq!(kind_of(&lines[1], "$name"), TokenKind::Variable);
        assert_eq!(kind_of(&lines[2], "Get-ChildItem"), TokenKind::Function);
    }

    /// The Markdown diff colors structure and prose markup, and fenced code in its own language
    /// with the code itself plain (not string-colored as raw markup).
    #[test]
    fn colors_markdown_and_its_fenced_code() {
        let source = concat!(
            "# Title\n",
            "Some **bold** and *it* with `code` and [link](https://x.test).\n",
            "- item\n",
            "1. one\n",
            "\n",
            "> quote\n",
            "\n",
            "---\n",
            "\n",
            "| a | b |\n",
            "| - | - |\n",
            "\n",
            "```rust\n",
            "fn main() { let total = 1; }\n",
            "```\n",
            "\n",
            "```\n",
            "plain words\n",
            "```\n",
        );
        let lines = kinds_of("docs/README.md", source);
        assert_eq!(lines[0], [("# Title".to_owned(), TokenKind::Heading)]);
        assert_eq!(kind_of(&lines[1], "bold"), TokenKind::Strong);
        assert_eq!(kind_of(&lines[1], "it"), TokenKind::Emphasis);
        assert_eq!(kind_of(&lines[1], "**"), TokenKind::Markup);
        assert_eq!(kind_of(&lines[1], "`code`"), TokenKind::String);
        assert_eq!(kind_of(&lines[1], "link"), TokenKind::Constant);
        assert_eq!(kind_of(&lines[1], "https://x.test"), TokenKind::String);
        assert_eq!(kind_of(&lines[2], "-"), TokenKind::Markup);
        assert_eq!(kind_of(&lines[3], "1."), TokenKind::Markup);
        assert_eq!(kind_of(&lines[5], ">"), TokenKind::Markup);
        assert_eq!(kind_of(&lines[7], "---"), TokenKind::Markup);
        assert_eq!(kind_of(&lines[9], "|"), TokenKind::Markup);
        assert_eq!(kind_of(&lines[12], "```"), TokenKind::Markup);
        assert_eq!(kind_of(&lines[12], "rust"), TokenKind::Constant);
        assert_eq!(kind_of(&lines[13], "fn"), TokenKind::Keyword);
        assert_eq!(kind_of(&lines[13], "main"), TokenKind::Function);
        assert_eq!(kind_of(&lines[13], "1"), TokenKind::Number);
        assert_eq!(kind_of(&lines[13], "total"), TokenKind::Plain, "{:?}", lines[13]);
        assert_eq!(
            lines[17],
            [("plain words".to_owned(), TokenKind::Plain)],
            "a fence without a language stays plain"
        );
    }

    #[test]
    fn leaves_very_long_lines_plain() {
        let long = format!("let x = \"{}\";\n", "a".repeat(MAX_LINE_BYTES));
        let source = format!("{long}let y = 1;\n");
        let lines = kinds_of("a.rs", &source);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(kind_of(&lines[1], "let"), TokenKind::Keyword);
    }

    /// TypeScript whose lines read differently by what came before: a block comment, a template
    /// string and a class over several lines, and top-level code, `repeat` times.
    fn typescript_declarations(repeat: usize) -> String {
        let block = concat!(
            "/* a comment\n",
            "   over lines */\n",
            "const text = `template ${value}\n",
            "still the template`;\n",
            "export class Name<T> {\n",
            "    private field = 1;\n",
            "    method(argument: string): void { return; }\n",
            "}\n",
            "function call(a: number) { return a + 0x1f; }\n",
        );
        block.repeat(repeat)
    }

    /// [`typescript_declarations`] all inside one namespace block, indented as usual.
    fn typescript_namespace(repeat: usize) -> String {
        let declarations = typescript_declarations(repeat);
        let inner: String = declarations.lines().flat_map(|line| ["    ", line, "\n"]).collect();
        format!("declare namespace Outer {{\n{inner}}}\n")
    }

    /// TypeScript `source` with a fresh copy of the grammar set.
    fn ts_source(source: &str) -> TextSource {
        let grammar = grammar::for_path("a.ts", "").expect("grammar");
        let text = Arc::new(BlobText::from_bytes(source.as_bytes().to_vec()));
        TextSource { text, syntax: grammar.syntax, working: grammar.working_set() }
    }

    /// Counts the lines its reader read, over all parts, and notes the threads that read parts.
    struct Counting<R> {
        reader: R,
        tally: Arc<Tally>,
    }

    #[derive(Default)]
    struct Tally {
        reads: std::sync::atomic::AtomicUsize,
        part_threads: std::sync::Mutex<HashSet<std::thread::ThreadId>>,
    }

    impl<R: LineReader> LineReader for Counting<R> {
        type State = R::State;
        type Parts = Counting<R::Parts>;

        fn parts(&self) -> Self::Parts {
            Counting { reader: self.reader.parts(), tally: self.tally.clone() }
        }

        fn read(&mut self, index: usize, spans: &mut Vec<Span>) {
            self.tally.reads.fetch_add(1, Ordering::Relaxed);
            self.reader.read(index, spans);
        }

        fn state(&self) -> Self::State {
            self.reader.state()
        }

        fn restore(&mut self, state: &Self::State) {
            self.reader.restore(state);
        }

        fn is_in(&self, state: &Self::State) -> bool {
            self.reader.is_in(state)
        }

        fn start_indent(&self, index: usize) -> Option<usize> {
            self.reader.start_indent(index)
        }
    }

    impl<P: ReadParts> ReadParts for Counting<P> {
        type State = P::State;

        fn read_part(
            &self,
            state: Option<&Self::State>,
            lines: Range<usize>,
            stop: &AtomicBool,
        ) -> Part<Self::State> {
            let part = self.reader.read_part(state, lines, stop);
            self.tally.reads.fetch_add(part.spans.line_count(), Ordering::Relaxed);
            let mut threads = self.tally.part_threads.lock().expect("the thread set");
            threads.insert(std::thread::current().id());
            part
        }
    }

    /// The spans of `source` read in `parts` parts after `head` lines, and how many lines were
    /// read over all parts, by which reader threads.
    fn read_ts(source: &str, parts: usize, head: usize) -> (LineSpans, Arc<Tally>) {
        let source = ts_source(source);
        let text = &source.text;
        let tally = Arc::new(Tally::default());
        let mut spans = LineSpans::default();
        let counting = Counting { reader: TextReader::new(&source), tally: tally.clone() };
        let is_read = read_in_parts(counting, text.line_count(), parts, head, |line| {
            spans.push_line(line);
            true
        });
        assert!(is_read);
        assert_eq!(spans.line_count(), text.line_count());
        (spans, tally)
    }

    /// Reading in parts gives the spans reading in order gives, wherever the parts start.
    #[test]
    fn reads_in_parts_as_in_order() {
        let source = format!("{}{}", typescript_declarations(20), typescript_namespace(20));
        let (in_order, _) = read_ts(&source, 1, 0);
        for (parts, head) in [(2, 0), (3, 50), (7, 0), (7, 200), (40, 10)] {
            assert_eq!(read_ts(&source, parts, head).0, in_order, "{parts} parts after {head}");
        }
    }

    /// Where the parts' states meet the lines before, each line is read about once: the reader
    /// before a part reads on into it for at most a few checks, not all of it. In a file of
    /// declarations from a fresh state, inside a long block from the state the first lines had at
    /// that indent.
    #[test]
    fn parts_meet_the_lines_before() {
        for (source, head) in [(typescript_declarations(40), 0), (typescript_namespace(40), 30)] {
            let lines = source.lines().count();
            let reads = read_ts(&source, 4, head).1.reads.load(Ordering::Relaxed);
            assert!(reads <= lines + 3 * 3 * CHECK_EVERY, "{reads} reads for {lines} lines");
        }
    }

    /// Text after text, the parts are read by the same few kept threads: the regex engine keeps a
    /// search cache for every thread that ever searched with a grammar's regexes, so a fresh
    /// thread per part grew the compiled grammar with every text read.
    #[test]
    fn reads_the_parts_of_every_text_on_the_same_threads() {
        let source = typescript_declarations(20);
        let threads: HashSet<_> = (0..5)
            .flat_map(|_| {
                let (_, tally) = read_ts(&source, MAX_PARTS, 0);
                let threads = tally.part_threads.lock().expect("the thread set");
                threads.iter().copied().collect::<Vec<_>>()
            })
            .collect();
        assert!(threads.len() <= MAX_PARTS, "{} threads read parts", threads.len());
    }

    #[test]
    fn stops_reading_in_parts_when_asked() {
        let source = ts_source(&typescript_declarations(20));
        let mut calls = 0;
        let count = source.text.line_count();
        let is_read = read_in_parts(TextReader::new(&source), count, 4, 5, |_| {
            calls += 1;
            calls < 30
        });
        assert!(!is_read);
        assert_eq!(calls, 30);
    }

    #[test]
    fn delivers_in_chunks_and_stops_when_asked() {
        let source = "let a = 1;\n".repeat(CHUNK_LINES * 2 + 5);
        let text = BlobText::from_bytes(source.into_bytes());
        let grammar = grammar::for_path("a.rs", "").unwrap();
        let working = grammar.working_set();
        let mut chunks = Vec::new();
        highlight(&text, grammar.syntax, &working, |chunk| {
            chunks.push(chunk.line_count());
            true
        });
        assert_eq!(chunks, [CHUNK_LINES, CHUNK_LINES, 5]);
        let mut calls = 0;
        highlight(&text, grammar.syntax, &working, |_| {
            calls += 1;
            false
        });
        assert_eq!(calls, 1);
    }
}
