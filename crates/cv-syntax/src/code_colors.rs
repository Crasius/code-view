//! Syntax colors for code shown outside the diff: the fenced blocks of rendered Markdown (file
//! previews, descriptions, comments). The diff's grammars and token kinds, worked out on the
//! coloring thread once per language and text and kept: painting a block again (a hover, a
//! scroll, the other pane of a rendered diff) never highlights it again. Until the colors are
//! there the block shows plain, in the same font, so nothing moves when they arrive.

#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use cv_base::theme::syntax::TokenKind;
use cv_runtime::cache::memory::{BoundedCache, Holder};

use crate::blob_text::BlobText;
use crate::color_worker;
use crate::grammar;
use crate::highlight::{self, LineSpans};

/// Blocks longer than this stay plain: a pasted log is not worth a grammar's time.
const MAX_BYTES: usize = 256 * 1024;
/// Colors kept for the process, the least recently asked for dropped first: our number, the blocks
/// of the descriptions, comments and previews read lately (a 300-line block takes about 30 KB). A
/// block shown again after it went colors again on the coloring thread, in milliseconds.
const BUDGET_BYTES: usize = 4 * 1024 * 1024;
/// What an entry takes besides its colors: the key, the entry and the cache's bookkeeping.
const ENTRY_BYTES: usize = 64;

/// The token kinds of one block, line by line.
pub struct CodeColors {
    spans: LineSpans,
}

impl CodeColors {
    /// Where each token of line `index` (0-based) starts, in bytes, and what it is. Text before
    /// the first token is plain.
    pub fn line(&self, index: usize) -> impl Iterator<Item = (usize, TokenKind)> + '_ {
        self.spans.line(index).unwrap_or_default().iter().map(|span| (span.start(), span.kind()))
    }
}

enum Entry {
    /// Queued on the coloring thread.
    Pending,
    /// `None`: no grammar for the language, or too long to color.
    Done(Option<Arc<CodeColors>>),
}

struct Cache {
    entries: BoundedCache<u64, Entry>,
    /// How often each key was highlighted, for the tests that check nothing is done twice.
    #[cfg(any(test, feature = "test-support"))]
    computed: HashMap<u64, usize>,
}

fn shared() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(Cache {
            entries: BoundedCache::new(Holder::CodeColors, BUDGET_BYTES),
            #[cfg(any(test, feature = "test-support"))]
            computed: HashMap::new(),
        })
    })
}

fn key(language: &str, code: &str) -> u64 {
    egui::epaint::util::hash((language.trim().to_ascii_lowercase(), code))
}

/// The colors of `code` in `language` (a fenced block's info string) once they are worked out;
/// `None` while they are on their way and for code that stays plain. The first call queues the
/// work and `ctx` repaints when it is done.
pub fn colors(ctx: &egui::Context, language: &str, code: &str) -> Option<Arc<CodeColors>> {
    if language.trim().is_empty() || code.is_empty() || code.len() > MAX_BYTES {
        return None;
    }
    let key = key(language, code);
    let mut cache = shared().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match cache.entries.get(&key) {
        Some(Entry::Done(colors)) => return colors.clone(),
        Some(Entry::Pending) => return None,
        None => {}
    }
    cache.entries.insert(key, Entry::Pending, ENTRY_BYTES);
    drop(cache);
    let (language, code, ctx) = (language.to_owned(), code.to_owned(), ctx.clone());
    color_worker::submit(move || {
        let colors = grammar::for_language(&language).map(|grammar| {
            let text = BlobText::from_bytes(code.into_bytes());
            let mut spans = LineSpans::default();
            let working = grammar::compiled(grammar);
            highlight::highlight(&text, grammar.syntax, &working, |chunk| {
                spans.append(chunk);
                true
            });
            spans.shrink_to_fit();
            Arc::new(CodeColors { spans })
        });
        let mut cache = shared().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        #[cfg(any(test, feature = "test-support"))]
        {
            *cache.computed.entry(key).or_default() += 1;
        }
        // Dropped from the cache meanwhile: whoever asks again queues it again.
        if cache.entries.peek(&key).is_some() {
            let bytes = ENTRY_BYTES + colors.as_ref().map_or(0, |colors| colors.spans.heap_size());
            cache.entries.insert(key, Entry::Done(colors), bytes);
        }
        drop(cache);
        ctx.request_repaint();
    });
    None
}

/// How often `code` in `language` was highlighted.
#[cfg(any(test, feature = "test-support"))]
pub fn computed_count(language: &str, code: &str) -> usize {
    let cache = shared().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.computed.get(&key(language, code)).copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use cv_testkit::WAIT_LIMIT;

    use super::*;
    use crate::color_worker::observe_repaints;

    /// Asks for the colors again each time the coloring thread wakes `ctx`, until it finished them;
    /// fails after [`cv_testkit::WAIT_LIMIT`].
    fn wait_for(ctx: &egui::Context, language: &str, code: &str) -> Option<Arc<CodeColors>> {
        let repaints = observe_repaints(ctx);
        let deadline = Instant::now() + WAIT_LIMIT;
        loop {
            if let Some(colors) = colors(ctx, language, code) {
                return Some(colors);
            }
            if computed_count(language, code) > 0 {
                return colors(ctx, language, code);
            }
            let left = deadline.checked_duration_since(Instant::now()).unwrap_or_default();
            assert!(
                repaints.recv_timeout(left).is_ok(),
                "the coloring thread did not answer for {language} within {} s",
                WAIT_LIMIT.as_secs()
            );
        }
    }

    #[test]
    fn colors_a_block_once_in_its_language() {
        let ctx = egui::Context::default();
        let code = "fn colors_a_block_once() {\n    let total = 42;\n}";
        let colors = wait_for(&ctx, "rust", code).expect("Rust has a grammar");
        let first: Vec<(usize, TokenKind)> = colors.line(0).collect();
        assert_eq!(first.first(), Some(&(0, TokenKind::Keyword)), "{first:?}");
        assert!(colors.line(1).any(|(_, kind)| kind == TokenKind::Number));
        for _ in 0..10 {
            assert!(super::colors(&ctx, "rust", code).is_some());
        }
        assert_eq!(computed_count("rust", code), 1, "asking again reuses the colors");
    }

    #[test]
    fn unknown_and_missing_languages_stay_plain() {
        let ctx = egui::Context::default();
        assert!(colors(&ctx, "", "plain").is_none());
        let code = "unknown_and_missing_languages_stay_plain";
        assert!(wait_for(&ctx, "no-such-language", code).is_none());
        assert_eq!(computed_count("no-such-language", code), 1);
    }
}
