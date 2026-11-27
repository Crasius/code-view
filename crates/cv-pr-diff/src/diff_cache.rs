//! Recently shown diffs and the diffs loaded ahead of the user (the Files tab's prefetch), bounded by
//! bytes, so flipping between files is instant without growing memory. A diff loaded ahead counts
//! as unopened until something shows it: at most [`MAX_UNOPENED`] of them stay, the ones the
//! user moved away from are released ([`DiffCache::release_unopened`]), and they go first, oldest
//! first, when the byte budget is spent; the shown diffs go least recently used first.

use std::collections::VecDeque;
use std::sync::Arc;

use review_git::{ChangedFile, DiffOptions, FileDiff};

use cv_runtime::cache::memory::Holder;

pub struct DiffCache {
    entries: VecDeque<Entry>,
    budget_bytes: usize,
    /// Bytes of every entry, kept as entries come and go: the folder summary fills the cache
    /// with many small diffs, and summing them on every insert would grow with their number.
    bytes: usize,
}

struct Entry {
    key: DiffKey,
    diff: Arc<FileDiff>,
    /// Loaded ahead and not shown yet.
    is_unopened: bool,
}

/// A diff by what it shows: the file's two versions (their blob ids, and the path the old one
/// had) and the options. Not by the compared commits: in another comparison where the file did not
/// change (a push to other files, a target branch that moved on) its diff is the same, so it is
/// neither loaded nor laid out again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiffKey {
    pub path: String,
    pub old_path: Option<String>,
    pub old_oid: String,
    pub new_oid: String,
    pub options: DiffOptions,
}

impl DiffKey {
    pub fn of(file: &ChangedFile, options: DiffOptions) -> Self {
        Self {
            path: file.path.clone(),
            old_path: file.old_path.clone(),
            old_oid: file.old_oid.clone(),
            new_oid: file.new_oid.clone(),
            options,
        }
    }

    /// This is the key of `file`'s diff with `options`, compared without building that key.
    pub fn is_of(&self, file: &ChangedFile, options: DiffOptions) -> bool {
        self.path == file.path
            && self.old_path == file.old_path
            && self.old_oid == file.old_oid
            && self.new_oid == file.new_oid
            && self.options == options
    }
}

/// Diffs kept per pull request: our number, the open file, its neighbours loaded ahead and the
/// files shown last (a 15,000-line diff takes about 2 MB), so flipping between them is instant. A
/// diff further away comes again from the local mirror in tens of milliseconds.
const DEFAULT_BUDGET_BYTES: usize = 24 * 1024 * 1024;
/// Diffs loaded ahead and not opened yet: the files around the open one and a hovered one or
/// two, with room for the last plan's files while the next plan's files load.
pub(crate) const MAX_UNOPENED: usize = 8;

impl Default for DiffCache {
    fn default() -> Self {
        Self::with_budget(DEFAULT_BUDGET_BYTES)
    }
}

impl DiffCache {
    fn with_budget(budget_bytes: usize) -> Self {
        Self { entries: VecDeque::new(), budget_bytes, bytes: 0 }
    }

    /// The diff for `key`, which counts as shown from now on.
    pub fn get(&mut self, key: &DiffKey) -> Option<Arc<FileDiff>> {
        let position = self.entries.iter().position(|entry| entry.key == *key)?;
        let mut entry = self.entries.remove(position)?;
        entry.is_unopened = false;
        let diff = entry.diff.clone();
        self.entries.push_back(entry);
        Some(diff)
    }

    /// Whether a diff of `path` is cached, in any comparison.
    #[cfg(any(test, feature = "test-support"))]
    pub fn has_path(&self, path: &str) -> bool {
        self.entries.iter().any(|entry| entry.key.path == path)
    }

    /// Whether `key` is cached, without marking it as recently used.
    pub fn contains(&self, key: &DiffKey) -> bool {
        self.entries.iter().any(|entry| entry.key == *key)
    }

    /// A diff that is shown.
    pub fn insert(&mut self, key: DiffKey, diff: Arc<FileDiff>) {
        if let Some(position) = self.entries.iter().position(|entry| entry.key == key) {
            self.remove(position);
        }
        self.push(Entry { key, diff, is_unopened: false });
    }

    /// A diff loaded ahead, unless the cache has it already: the oldest unopened one goes once
    /// there are more than [`MAX_UNOPENED`].
    pub fn insert_ahead(&mut self, key: DiffKey, diff: Arc<FileDiff>) {
        if self.contains(&key) {
            return;
        }
        self.push(Entry { key, diff, is_unopened: true });
        if self.entries.iter().filter(|entry| entry.is_unopened).count() > MAX_UNOPENED
            && let Some(oldest) = self.entries.iter().position(|entry| entry.is_unopened)
        {
            self.remove(oldest);
        }
    }

    /// Drops the diffs loaded ahead and not opened that `keep` turns down; returns their keys.
    pub fn release_unopened(&mut self, keep: impl Fn(&DiffKey) -> bool) -> Vec<DiffKey> {
        let mut released = Vec::new();
        let mut position = 0;
        while let Some(entry) = self.entries.get(position) {
            if entry.is_unopened && !keep(&entry.key) {
                released.push(entry.key.clone());
                self.remove(position);
            } else {
                position += 1;
            }
        }
        released
    }

    fn push(&mut self, entry: Entry) {
        self.bytes += entry.diff.heap_size();
        Holder::Diffs.add(entry.diff.heap_size());
        self.entries.push_back(entry);
        while self.bytes > self.budget_bytes && self.entries.len() > 1 {
            let unopened = self.entries.iter().position(|entry| entry.is_unopened);
            self.remove(unopened.unwrap_or(0));
        }
    }

    fn remove(&mut self, position: usize) {
        if let Some(removed) = self.entries.remove(position) {
            self.bytes -= removed.diff.heap_size();
            Holder::Diffs.sub(removed.diff.heap_size());
        }
    }
}

impl Drop for DiffCache {
    fn drop(&mut self) {
        Holder::Diffs.sub(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use review_git::{DiffOptions, FileDiff};

    use super::{DiffCache, DiffKey, MAX_UNOPENED};

    fn key(path: &str) -> DiffKey {
        DiffKey {
            path: path.into(),
            old_path: None,
            old_oid: "1".repeat(40),
            new_oid: "2".repeat(40),
            options: DiffOptions::context(3),
        }
    }

    fn diff() -> Arc<FileDiff> {
        Arc::new(FileDiff::parse_unified(
            b"@@ -1 +1 @@
-a
+b
",
        ))
    }

    #[test]
    fn keeps_its_byte_count_and_drops_the_least_recently_used() {
        let diff = diff();
        let size = diff.heap_size();
        let mut cache = DiffCache::with_budget(size * 2);
        cache.insert(key("a"), diff.clone());
        cache.insert(key("b"), diff.clone());
        cache.insert(key("a"), diff.clone());
        assert_eq!(cache.bytes, size * 2, "replacing an entry does not count it twice");
        assert!(cache.get(&key("b")).is_some(), "b is now the most recent");
        cache.insert(key("c"), diff);
        assert_eq!(cache.bytes, size * 2);
        assert!(
            !cache.contains(&key("a")) && cache.contains(&key("b")) && cache.contains(&key("c"))
        );
    }

    #[test]
    fn diffs_loaded_ahead_are_capped_and_go_before_shown_ones() {
        let diff = diff();
        let size = diff.heap_size();
        let mut cache = DiffCache::with_budget(size * (MAX_UNOPENED + 2));
        cache.insert(key("shown"), diff.clone());
        for index in 0..=MAX_UNOPENED {
            cache.insert_ahead(key(&format!("ahead-{index}")), diff.clone());
        }
        assert!(!cache.contains(&key("ahead-0")), "the oldest diff loaded ahead went");
        assert!((1..=MAX_UNOPENED).all(|index| cache.contains(&key(&format!("ahead-{index}")))));
        assert!(cache.contains(&key("shown")), "the cap leaves shown diffs alone");

        assert!(cache.get(&key("ahead-1")).is_some(), "opening a diff loaded ahead finds it");
        cache.insert_ahead(key("ahead-9"), diff.clone());
        assert!(cache.contains(&key("ahead-2")), "an opened diff no longer counts against the cap");

        cache.insert_ahead(key("shown"), diff.clone());
        assert_eq!(
            cache.bytes,
            size * (MAX_UNOPENED + 2),
            "a diff the cache has is not added again"
        );
        cache.insert(key("next"), diff);
        assert!(
            cache.contains(&key("shown")) && !cache.contains(&key("ahead-2")),
            "over the byte budget, the oldest unopened diff goes before an older shown one"
        );
        assert_eq!(cache.bytes, size * (MAX_UNOPENED + 2));
    }

    #[test]
    fn releasing_drops_only_unopened_diffs_and_their_bytes() {
        let diff = diff();
        let size = diff.heap_size();
        let mut cache = DiffCache::default();
        cache.insert(key("shown"), diff.clone());
        cache.insert_ahead(key("ahead-kept"), diff.clone());
        cache.insert_ahead(key("ahead-gone"), diff);
        let released = cache.release_unopened(|key| key.path == "ahead-kept");
        assert_eq!(released, [key("ahead-gone")]);
        assert!(cache.contains(&key("shown")) && cache.contains(&key("ahead-kept")));
        assert_eq!(cache.bytes, size * 2);
        assert_eq!(cache.release_unopened(|_| false).len(), 1, "a shown diff is never released");
    }
}
