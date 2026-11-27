//! What the app holds in memory: the heap's live bytes ([`cv_base::platform::heap`]) and, by
//! holder, the bytes of the caches that grow with use, for the frame stats overlay and the memory
//! walkthrough, and for the memory journal (`cv_services::memory_journal`).
//!
//! Every cache that grows with use has a budget of its own (our numbers, each named where the
//! cache lives) and gives back what it holds when it is dropped. [`BoundedCache`] is the shared
//! shape of such a cache: entries weighed in bytes, the least recently used ones evicted once the
//! budget is spent, its bytes counted under its [`Holder`] while it lives. Several instances of one
//! holder (one per open pull request) add up. Stores of textures count theirs with
//! [`TextureMemory`] and let go of a texture nobody drew for [`UNSEEN_TEXTURE_SECONDS`].
//!
//! # Cache policy
//!
//! A cache earns its bytes by how often its entries are asked for again times what a miss costs
//! then. Budgets are our numbers; revisit rates are our estimates; miss costs marked "measured"
//! come from a release build on the development machine, the others are estimates. Per PR means
//! per open pull request page (up to three are kept). Hot is memory; cold is the disk tier
//! ([`super::cold`], 64 MB for all kinds, least recently used first), there only where a miss
//! after a restart costs far more than reading the file back.
//!
//! | Cache | Holds | Revisit | Miss costs | Budget | Goes when | Tier |
//! |---|---|---|---|---|---|---|
//! | page answers | last list and PR part answers | often (list, overview) | a host round trip; from disk 0.3 ms for 100 rows (measured) | 4 MB | least recently used; an answer asked for earlier never replaces a later one; the user's writes change it in place and make it stale | hot; lists, overviews, threads also cold |
//! | http answers | host answers with their validators | often (refresh) | a full answer, not "unchanged" | 32 MB | least recently used | hot |
//! | grammars | compiled syntax, per language | every file | up to 1 s and about 30 MB (TypeScript) | 96 MB, process | 5 min unused, then oldest | hot |
//! | diffs | the open file, neighbours, last shown | flipping files | tens of ms from the mirror | 24 MB per PR | oldest, ahead-loaded first | hot (the mirror is the cold copy) |
//! | file texts | old and new text with colors | going back to a file | load and color again, background | 24 MB per PR | least recently used | hot |
//! | diff colors | colors by diff line | flipping files | 8 s for 15,000 TypeScript lines; from disk 1.3 ms (measured) | 8 MB per PR | least recently used | hot; above 150 ms also cold |
//! | code colors | fenced blocks in Markdown | rereading a card | milliseconds | 4 MB, process | least recently used | hot |
//! | markdown | parsed descriptions and comments | every frame shown | 0.3 ms for 4 KB (measured), a disk read costs as much | 8 MB, process | least recently used | hot |
//! | images | decoded textures | while on screen | decode from the image cache, background | 64 MB | 30 s not drawn | hot (files are cold) |
//! | diagrams | rendered Mermaid textures | while on screen | render again, background | 48 MB | 30 s not drawn | hot |
//!
//! Pipeline logs and job lists are held by their view only and go with it: they are rarely
//! reopened, and a reopen is one request.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt::{self, Debug, Write as _};
use std::hash::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};

use egui::TextureHandle;

/// How long a texture stays after it was last drawn. One seen again decodes again (from the disk
/// cache, the mirror or its SVG, in the background) and shows at the size it had meanwhile.
pub const UNSEEN_TEXTURE_SECONDS: f64 = 30.0;

/// A kind of cache whose bytes the overlay shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    /// Decoded Markdown images, as textures.
    Images,
    /// Mermaid diagrams, as textures.
    Diagrams,
    /// The last answers of lists and pull request parts ([`super::page_cache`]).
    PageAnswers,
    /// Parsed Markdown documents (descriptions, comments, previews, answers).
    Markdown,
    /// Compiled syntax grammars, once per language for the process (estimated).
    Grammars,
    /// Loaded file diffs of the open pull requests.
    Diffs,
    /// Whole file texts of the open pull requests (for colors and expanded context).
    FileTexts,
    /// Syntax colors of diff rows.
    DiffColors,
    /// Syntax colors of whole files.
    CodeColors,
}

impl Holder {
    const ALL: [Holder; 9] = [
        Holder::Images,
        Holder::Diagrams,
        Holder::PageAnswers,
        Holder::Markdown,
        Holder::Grammars,
        Holder::Diffs,
        Holder::FileTexts,
        Holder::DiffColors,
        Holder::CodeColors,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Holder::Images => "images",
            Holder::Diagrams => "diagrams",
            Holder::PageAnswers => "page answers",
            Holder::Markdown => "markdown",
            Holder::Grammars => "grammars",
            Holder::Diffs => "diffs",
            Holder::FileTexts => "file texts",
            Holder::DiffColors => "diff colors",
            Holder::CodeColors => "code colors",
        }
    }

    fn counter(self) -> &'static AtomicUsize {
        static BYTES: [AtomicUsize; Holder::ALL.len()] =
            [const { AtomicUsize::new(0) }; Holder::ALL.len()];
        &BYTES[self as usize]
    }

    /// Bytes held under this holder now, over all its caches.
    pub fn bytes(self) -> usize {
        self.counter().load(Ordering::Relaxed)
    }

    pub fn add(self, bytes: usize) {
        self.counter().fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn sub(self, bytes: usize) {
        self.counter().fetch_sub(bytes, Ordering::Relaxed);
    }

    /// For a holder with one owner that knows its total, such as the grammar registry.
    pub fn set(self, bytes: usize) {
        self.counter().store(bytes, Ordering::Relaxed);
    }
}

/// The holders and the HTTP answer cache with their bytes, the largest first.
pub fn holders() -> Vec<(&'static str, usize)> {
    let mut holders = readings();
    holders.sort_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));
    holders
}

/// The holders and their bytes in a fixed order, the HTTP answer cache last: the memory journal's
/// columns (`cv_services::memory_journal`).
pub fn readings() -> Vec<(&'static str, usize)> {
    let mut holders: Vec<(&'static str, usize)> =
        Holder::ALL.iter().map(|holder| (holder.label(), holder.bytes())).collect();
    holders.push(("http answers", review_core::http::cached_bytes()));
    holders
}

/// The texture bytes one store holds, counted under its holder, against the store's budget.
pub struct TextureMemory {
    holder: Holder,
    limit: usize,
    held: usize,
    /// Input time of the last look for textures nobody drew lately.
    swept_at: f64,
}

impl TextureMemory {
    pub fn new(holder: Holder, limit: usize) -> Self {
        Self { holder, limit, held: 0, swept_at: f64::NEG_INFINITY }
    }

    pub fn add(&mut self, texture: &TextureHandle) {
        let bytes = texture_bytes(texture);
        self.held += bytes;
        self.holder.add(bytes);
    }

    pub fn remove(&mut self, texture: &TextureHandle) {
        let bytes = texture_bytes(texture).min(self.held);
        self.held -= bytes;
        self.holder.sub(bytes);
    }

    pub fn is_over_limit(&self) -> bool {
        self.held > self.limit
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn held(&self) -> usize {
        self.held
    }

    /// Whether to look for textures nobody drew for [`UNSEEN_TEXTURE_SECONDS`] at input time
    /// `now`: once a second while any texture is held.
    pub fn is_sweep_due(&mut self, now: f64) -> bool {
        if self.held == 0 || now - self.swept_at < 1.0 {
            return false;
        }
        self.swept_at = now;
        true
    }
}

impl Drop for TextureMemory {
    fn drop(&mut self) {
        self.holder.sub(self.held);
    }
}

/// What a texture takes: RGBA plus about a third for the mipmaps.
pub fn texture_bytes(texture: &TextureHandle) -> usize {
    let [width, height] = texture.size();
    width * height * 4 * 4 / 3
}

/// An estimate of the bytes `value` holds: its `Debug` length plus its inline size, counted without
/// allocating. It overstates the real heap use (field names, punctuation), which keeps a bound on
/// the safe side, and costs one walk over the value, as much as building it did.
pub fn debug_weight<T: Debug + ?Sized>(value: &T) -> usize {
    struct Counter(usize);
    impl fmt::Write for Counter {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            self.0 += text.len();
            Ok(())
        }
    }
    let mut counter = Counter(0);
    // The counter never fails; a `Debug` impl that gives up early only makes the estimate smaller.
    let _partial = write!(counter, "{value:?}");
    counter.0 + size_of_val(value)
}

/// A page the app keeps out of sight for a quick return: it lets go of what loads again (its
/// caches) and keeps what the user did on it. Pull request pages implement it
/// (`files::tab::hibernate`).
pub trait Hibernate {
    fn hibernate(&mut self);
}

/// A cache bounded by bytes: each entry is weighed when it comes in, and the least recently used
/// entries leave once the total is over the budget. An entry larger than half the budget is not
/// kept: it would push out everything else.
pub struct BoundedCache<K, V> {
    holder: Holder,
    budget: usize,
    used: usize,
    /// Increases with every use; the smallest value is the least recently used entry.
    clock: u64,
    entries: HashMap<K, Entry<V>>,
}

struct Entry<V> {
    value: V,
    bytes: usize,
    last_used: u64,
}

impl<K: Eq + Hash + Clone, V> BoundedCache<K, V> {
    pub fn new(holder: Holder, budget: usize) -> Self {
        Self { holder, budget, used: 0, clock: 0, entries: HashMap::new() }
    }

    /// The value for `key`, marked as recently used.
    pub fn get<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.last_used = self.clock;
        Some(&entry.value)
    }

    /// The value for `key`, without marking it as used.
    pub fn peek<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.entries.get(key).map(|entry| &entry.value)
    }

    /// The value for `key` to change in place, without marking it as used; its weight stays.
    pub fn peek_mut<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        self.entries.get_mut(key).map(|entry| &mut entry.value)
    }

    /// Every key held, in no order.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.keys()
    }

    /// Keeps `value`, which takes `bytes`, as the newest entry for `key`, and evicts the least
    /// recently used entries beyond the budget.
    pub fn insert(&mut self, key: K, value: V, bytes: usize) {
        self.remove(&key);
        if bytes > self.budget / 2 {
            return;
        }
        self.clock += 1;
        self.used += bytes;
        self.holder.add(bytes);
        self.entries.insert(key, Entry { value, bytes, last_used: self.clock });
        while self.used > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.remove(&oldest);
        }
    }

    pub fn remove<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let entry = self.entries.remove(key)?;
        self.used -= entry.bytes;
        self.holder.sub(entry.bytes);
        Some(entry.value)
    }

    /// Bytes held.
    #[cfg(any(test, feature = "test-support"))]
    pub fn used(&self) -> usize {
        self.used
    }
}

impl<K, V> Drop for BoundedCache<K, V> {
    fn drop(&mut self) {
        self.holder.sub(self.used);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_the_least_recently_used_beyond_the_budget() {
        let mut cache = BoundedCache::new(Holder::Markdown, 300);
        cache.insert("a", 1, 100);
        cache.insert("b", 2, 100);
        cache.insert("c", 3, 100);
        assert_eq!(cache.get(&"a"), Some(&1), "a is now the most recently used");
        cache.insert("d", 4, 100);
        assert_eq!(cache.peek(&"b"), None, "b, used longest ago, left");
        assert!(cache.peek(&"a").is_some() && cache.peek(&"c").is_some());
        assert_eq!(cache.used(), 300);
        cache.insert("huge", 5, 151);
        assert_eq!(cache.peek(&"huge"), None, "more than half the budget is not kept");
        assert_eq!(cache.keys().count(), 3);
    }

    #[test]
    fn replacing_and_removing_keep_the_count_exact() {
        let mut cache = BoundedCache::new(Holder::Markdown, 1_000);
        cache.insert("a", 1, 100);
        cache.insert("a", 2, 40);
        assert_eq!((cache.used(), cache.peek(&"a")), (40, Some(&2)));
        assert_eq!(cache.remove(&"a"), Some(2));
        assert_eq!(cache.used(), 0);
    }
}
