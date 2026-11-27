//! Stale-while-revalidate memory for pages: the last answer of every list and pull request part a
//! page showed, so revisiting it (or a page that was dropped to free memory) paints that answer at
//! once while a background refresh brings the current one.
//!
//! A [`BoundedCache`] of an estimated byte budget, evicted least recently used first; values are
//! weighed by [`memory::debug_weight`].
//!
//! Lists, overviews and threads ([`PageValue::to_cold`]) also go to the disk tier
//! ([`ColdTier`]) as they arrive, so after a restart a page paints the last answer at once and
//! refreshes in place (it counts as stale, so the refresh starts and the page shows it is
//! refreshing). The answers saved last are read once at start, the most recently written first,
//! up to `SAVED_BYTES`, and decoded when a page first asks. Pipelines, file lists and commits
//! stay in memory: rarely revisited, or listed again by the mirror in milliseconds.
//!
//! Keys name a route part ([`list_part`], [`pull_request_part`]).
//!
//! Every answer is stamped with the time it was asked for as well as the time it arrived, and
//! [`PageCache::put_answer`] never lets an answer replace one that was asked for later. A warm-up
//! from the list fetches a detail before its git work and finishes long after; without the stamp
//! it would overwrite the newer detail its page wrote meanwhile (one after a merge, say) and count
//! as fresh. Freshness counts from the arrival, as a revisit sees it.
//!
//! The user's own writes (`cv_services::writes`) change remembered answers in place
//! ([`PageCache::update`]) and mark them outdated ([`PageCache::mark_outdated`]): they show what
//! the write did at once, and the next visit still asks the host for what the write did not model.
//!
//! A row two lists share is one pull request: an answer for one list patches the row in the
//! others ([`PageCache::patch`], `pull_request_list::spread`) without changing their age.

use std::any::Any;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use review_core::{
    PipelineRun, PullRequestDetail, PullRequestScope, PullRequestSummary, RepositoryInfo,
    ReviewThread, StateFilter,
};
use review_git::{ChangedFile, CommitInfo};

use super::cold::ColdTier;
use super::memory::{self, BoundedCache, Holder};

/// A few MB for everything remembered; the app aims for a small memory footprint.
const BUDGET_BYTES: usize = 4 * 1024 * 1024;
/// A revisit within this time shows the remembered answer without asking the host again.
pub const REVISIT_FRESH: Duration = Duration::from_secs(20);
/// The saved answers read at start: our number, as much as the memory tier holds.
const SAVED_BYTES: u64 = BUDGET_BYTES as u64;
/// The disk tier's kind for page answers, and the version of their format; a change to a stored
/// type that old files cannot decode into only costs one miss each.
const COLD_KIND: &str = "page";
const COLD_VERSION: u32 = 1;

/// A value the page cache keeps. Those with a disk form say so; the others stay in memory only.
pub trait PageValue: Any + Clone + Send + Sync + Debug {
    /// The value as stored on disk, `None` for one kept in memory only.
    fn to_cold(&self) -> Option<Vec<u8>> {
        None
    }

    /// The value from its disk form; `None` when it does not decode (another layout).
    fn from_cold(_bytes: &[u8]) -> Option<Self> {
        None
    }
}

/// Lists, overviews and threads: the pages a person comes back to, whose host answers take a
/// round trip each.
macro_rules! saved_as_json {
    ($($value:ty),* $(,)?) => {$(
        impl PageValue for $value {
            fn to_cold(&self) -> Option<Vec<u8>> {
                serde_json::to_vec(self).ok()
            }

            fn from_cold(bytes: &[u8]) -> Option<Self> {
                serde_json::from_slice(bytes).ok()
            }
        }
    )*};
}

saved_as_json!(Vec<PullRequestSummary>, PullRequestDetail, Vec<ReviewThread>, Vec<RepositoryInfo>);
impl PageValue for Arc<[PipelineRun]> {}
impl PageValue for Arc<Vec<ChangedFile>> {}
impl PageValue for Vec<CommitInfo> {}
/// The row a pull request was opened from (a list click or the title bar's search), under its
/// `summary` part: the header paints it until the detail is in. Memory only: the detail replaces it
/// within one round trip.
impl PageValue for PullRequestSummary {}

/// The key of one part (`detail`, `threads`, `files`, …) of the pull request whose storage key is
/// `storage_key`: what its page shows and what the list's line counting prepares for it.
pub fn pull_request_part(storage_key: &str, part: &str) -> String {
    format!("pr|{storage_key}|{part}")
}

/// The key of a pull request list of the connection `stamp` names (its id first): `listed` says
/// what it lists (a repository, `*` for all, the favorites it asks together), then its role and
/// state filter.
pub fn list_part(stamp: &str, listed: &str, scope: PullRequestScope, state: StateFilter) -> String {
    format!("pulls|{stamp}|{listed}|{scope:?}|{state:?}")
}

/// The start of every [`list_part`] key of the connection `connection_id`.
pub fn lists_of(connection_id: &str) -> String {
    format!("pulls|{connection_id}|")
}

/// The state filter of the list a [`list_part`] key names.
pub fn list_state(key: &str) -> Option<StateFilter> {
    match key.rsplit_once('|')?.1 {
        "Open" => Some(StateFilter::Open),
        "Merged" => Some(StateFilter::Merged),
        "Closed" => Some(StateFilter::Closed),
        "All" => Some(StateFilter::All),
        _ => None,
    }
}

pub struct PageCache {
    entries: BoundedCache<String, Entry>,
    /// Answers saved by an earlier run, as stored, until a page asks for them.
    saved: HashMap<String, Vec<u8>>,
    cold: Option<ColdTier>,
    /// See [`Self::patches`].
    patches: u64,
}

struct Entry {
    value: Arc<dyn Any + Send + Sync>,
    stamps: Stamps,
}

/// When an answer was asked for and arrived, and whether a write outdated it.
#[derive(Clone, Copy)]
struct Stamps {
    /// Input time (seconds) the value was asked for from the host: no answer asked for earlier
    /// replaces it.
    asked_at: f64,
    /// Input time (seconds) the value arrived, which its freshness counts from.
    stored_at: f64,
    /// A write of the user's changed what the host would answer now: stale whatever its age.
    is_outdated: bool,
}

impl Stamps {
    /// An answer saved by an earlier run: stale, and replaced by any answer.
    const SAVED: Stamps =
        Stamps { asked_at: f64::NEG_INFINITY, stored_at: f64::NEG_INFINITY, is_outdated: false };
}

impl Default for PageCache {
    fn default() -> Self {
        Self {
            entries: BoundedCache::new(Holder::PageAnswers, BUDGET_BYTES),
            saved: HashMap::new(),
            cold: None,
            patches: 0,
        }
    }
}

impl PageCache {
    /// The page cache with its disk tier, holding the answers saved by an earlier run. Reads the
    /// disk once, at start, like the settings.
    pub fn with_cold(cold: ColdTier) -> Self {
        let saved = cold.read_all(COLD_KIND, COLD_VERSION, SAVED_BYTES).into_iter().collect();
        Self { saved, cold: Some(cold), ..Self::default() }
    }

    /// Remembers `value` as the answer for `key` that arrived (and, as far as the caller knows,
    /// was asked for) at input time `now`: [`Self::put_answer`].
    pub fn put<T: PageValue>(&mut self, key: &str, value: T, now: f64) {
        self.put_answer(key, value, now, now);
    }

    /// Remembers `value`, asked for at input time `asked_at` and arrived at `now`, as the answer
    /// for `key`, and saves it to the disk tier when it has a disk form; an answer asked for before
    /// the one held is dropped (see the module doc). Values larger than half the budget are not
    /// kept in memory: they would push out everything else.
    pub fn put_answer<T: PageValue>(&mut self, key: &str, value: T, asked_at: f64, now: f64) {
        let is_older = self.entries.peek(key).is_some_and(|entry| entry.stamps.asked_at > asked_at);
        if is_older {
            return;
        }
        self.keep(key, value, Stamps { asked_at, stored_at: now, is_outdated: false });
    }

    fn keep<T: PageValue>(&mut self, key: &str, value: T, stamps: Stamps) {
        let weight = memory::debug_weight(&value);
        let value = Arc::new(value);
        self.saved.remove(key);
        if let Some(cold) = &self.cold {
            let saved = value.clone();
            cold.write(COLD_KIND, COLD_VERSION, key.to_owned(), move || saved.to_cold());
        }
        self.entries.insert(key.to_owned(), Entry { value, stamps }, weight);
    }

    /// A copy of the remembered answer for `key`, marked as recently used; an answer saved by an
    /// earlier run is decoded now and counts as stale.
    pub fn get<T: PageValue>(&mut self, key: &str) -> Option<T> {
        if let Some(bytes) = self.saved.remove(key) {
            let value = T::from_cold(&bytes)?;
            let weight = memory::debug_weight(&value);
            let entry = Entry { value: Arc::new(value.clone()), stamps: Stamps::SAVED };
            self.entries.insert(key.to_owned(), entry, weight);
            return Some(value);
        }
        self.entries.get(key)?.value.downcast_ref::<T>().cloned()
    }

    /// Changes the remembered answer for `key` with `change`, which says whether it changed
    /// anything, after a write of the user's that succeeded at input time `written_at`. A changed
    /// answer goes to the disk tier too, counts as asked for when the write succeeded (an answer
    /// asked for before cannot show it) and is outdated. True when it changed.
    pub fn update<T: PageValue>(
        &mut self,
        key: &str,
        written_at: f64,
        change: impl FnOnce(&mut T) -> bool,
    ) -> bool {
        let Some(mut value) = self.get::<T>(key) else {
            return false;
        };
        if !change(&mut value) {
            return false;
        }
        let held = self.entries.peek(key).map_or(Stamps::SAVED, |entry| entry.stamps);
        let asked_at = held.asked_at.max(written_at);
        self.keep(key, value, Stamps { asked_at, is_outdated: true, ..held });
        true
    }

    /// Changes the remembered answer for `key` with `change`, which says whether it changed
    /// anything, from what an answer for another key asked for at input time `asked_at` showed (a
    /// row two lists share). Skipped when the held answer was asked for at or after `asked_at`; a
    /// changed answer keeps its stamps, so it neither turns fresh nor stale, and goes to the disk
    /// tier too. True when it changed; [`Self::patches`] counts them.
    pub fn patch<T: PageValue>(
        &mut self,
        key: &str,
        asked_at: f64,
        change: impl FnOnce(&mut T) -> bool,
    ) -> bool {
        let Some(mut value) = self.get::<T>(key) else {
            return false;
        };
        let held = self.entries.peek(key).map_or(Stamps::SAVED, |entry| entry.stamps);
        if held.asked_at >= asked_at || !change(&mut value) {
            return false;
        }
        self.keep(key, value, held);
        self.patches += 1;
        true
    }

    /// How many answers [`Self::patch`] changed so far: a view showing a copy of one reads it again
    /// once this moves.
    pub fn patches(&self) -> u64 {
        self.patches
    }

    /// Marks every answer whose key starts with `prefix` as outdated, so the next visit asks the
    /// host again however young the answer is. Answers saved by an earlier run are stale already.
    pub fn mark_outdated(&mut self, prefix: &str) {
        let keys: Vec<String> =
            self.entries.keys().filter(|key| key.starts_with(prefix)).cloned().collect();
        for key in keys {
            if let Some(entry) = self.entries.peek_mut(key.as_str()) {
                entry.stamps.is_outdated = true;
            }
        }
    }

    /// The keys of the answers held that start with `prefix`, also those saved by an earlier run.
    pub fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        let held = self.entries.keys().chain(self.saved.keys());
        held.filter(|key| key.starts_with(prefix)).cloned().collect()
    }

    /// True when the answer for `key` is missing, outdated by a write or arrived
    /// [`REVISIT_FRESH`] or longer ago.
    pub fn is_stale(&self, key: &str, now: f64) -> bool {
        self.entries.peek(key).is_none_or(|entry| {
            let Stamps { stored_at, is_outdated, .. } = entry.stamps;
            is_outdated || now - stored_at >= REVISIT_FRESH.as_secs_f64()
        })
    }

    /// The input time the answer held for `key` arrived at; `None` when none is held, or only one
    /// saved by an earlier run.
    #[cfg(any(test, feature = "test-support"))]
    pub fn stored_at(&self, key: &str) -> Option<f64> {
        self.entries.peek(key).map(|entry| entry.stamps.stored_at).filter(|at| at.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl PageValue for String {}
    impl PageValue for Vec<String> {}
    impl PageValue for u32 {}

    #[test]
    fn returns_what_was_stored_and_tracks_age() {
        let mut cache = PageCache::default();
        cache.put("a", vec!["one".to_owned()], 10.0);
        assert_eq!(cache.stored_at("a"), Some(10.0));
        assert_eq!(cache.stored_at("missing"), None);
        assert_eq!(cache.get::<Vec<String>>("a"), Some(vec!["one".to_owned()]));
        assert_eq!(cache.get::<u32>("a"), None, "another type under the same key is a miss");
        assert!(!cache.is_stale("a", 10.0 + REVISIT_FRESH.as_secs_f64() - 1.0));
        assert!(cache.is_stale("a", 10.0 + REVISIT_FRESH.as_secs_f64()));
        assert!(cache.is_stale("missing", 0.0));
    }

    #[test]
    fn saved_answers_come_back_stale_and_memory_only_ones_stay_out() {
        let dir = cv_testkit::ScratchDir::new("page-cache-cold");
        let cold = ColdTier::new(&dir.app_paths());
        let repositories = vec![RepositoryInfo {
            key: "group/app".into(),
            name: "app".into(),
            description: None,
            web_url: "https://example.com/group/app".into(),
            default_branch: Some("main".into()),
        }];
        let mut cache = PageCache::with_cold(cold.clone());
        cache.put("runs", Arc::<[PipelineRun]>::from([]), 10.0);
        cache.put("repositories", repositories.clone(), 10.0);
        cold.flush();
        assert!(cold.read(COLD_KIND, COLD_VERSION, "repositories").is_some(), "on the disk");
        assert_eq!(cold.read(COLD_KIND, COLD_VERSION, "runs"), None, "pipelines stay in memory");
        let mut restarted = PageCache::with_cold(cold);
        assert_eq!(restarted.get::<Vec<RepositoryInfo>>("repositories"), Some(repositories));
        assert!(restarted.is_stale("repositories", 0.0), "a saved answer refreshes at once");
        assert_eq!(restarted.stored_at("repositories"), None, "it arrived in no frame of this run");
        assert_eq!(restarted.get::<Arc<[PipelineRun]>>("runs"), None);
    }

    /// The warm-up's detail, asked for before the page's own and arriving after it, never replaces
    /// it; the page's answer stays fresh.
    #[test]
    fn an_answer_asked_for_earlier_never_replaces_a_later_one() {
        let mut cache = PageCache::default();
        cache.put_answer("detail", "the page's, asked at 12".to_owned(), 12.0, 13.0);
        cache.put_answer("detail", "the warm-up's, asked at 10".to_owned(), 10.0, 40.0);
        assert_eq!(cache.get::<String>("detail").as_deref(), Some("the page's, asked at 12"));
        assert!(cache.is_stale("detail", 40.0), "the warm-up made nothing fresh");
        cache.put_answer("detail", "a refresh, asked at 30".to_owned(), 30.0, 41.0);
        assert_eq!(cache.get::<String>("detail").as_deref(), Some("a refresh, asked at 30"));
        assert!(!cache.is_stale("detail", 42.0), "fresh from its arrival");
    }

    /// A write changes the answer in place and makes it stale; an answer asked for before the
    /// write cannot show it and stays out.
    #[test]
    fn an_update_after_a_write_is_outdated_and_shuts_out_older_answers() {
        let mut cache = PageCache::default();
        cache.put("pulls|gh|Open", vec!["open".to_owned()], 10.0);
        cache.put("pulls|gl|Open", vec!["open".to_owned()], 10.0);
        let changed = cache.update("pulls|gh|Open", 15.0, |rows: &mut Vec<String>| {
            rows[0] = "merged".to_owned();
            true
        });
        assert!(changed);
        assert!(!cache.update("pulls|gh|Open", 15.0, |_: &mut Vec<String>| false));
        assert!(cache.is_stale("pulls|gh|Open", 15.5), "the next visit asks the host");
        cache.put("pulls|gh|Open", vec!["open".to_owned()], 12.0);
        assert_eq!(cache.get::<Vec<String>>("pulls|gh|Open"), Some(vec!["merged".to_owned()]));

        assert!(!cache.is_stale("pulls|gl|Open", 15.5));
        cache.mark_outdated("pulls|gl|");
        assert!(cache.is_stale("pulls|gl|Open", 15.5));
        assert_eq!(cache.keys_with_prefix("pulls|gl|"), ["pulls|gl|Open"]);
    }

    /// Another list's answer patches a held one in place without changing its age, and only when
    /// it was asked for later.
    #[test]
    fn a_patch_keeps_the_age_and_never_comes_from_an_older_answer() {
        let mut cache = PageCache::default();
        cache.put_answer("pulls|gh|Open", vec!["old".to_owned()], 10.0, 11.0);
        let rename = |rows: &mut Vec<String>| {
            rows[0] = "new".to_owned();
            true
        };
        assert!(!cache.patch("pulls|gh|Open", 9.0, rename), "asked for before the held answer");
        assert!(!cache.patch("pulls|gh|missing", 20.0, rename), "nothing held");
        assert_eq!(cache.patches(), 0);
        assert!(cache.patch("pulls|gh|Open", 20.0, rename));
        assert_eq!(cache.get::<Vec<String>>("pulls|gh|Open"), Some(vec!["new".to_owned()]));
        assert_eq!(cache.patches(), 1);
        assert!(!cache.is_stale("pulls|gh|Open", 11.0 + REVISIT_FRESH.as_secs_f64() - 1.0));
        assert!(cache.is_stale("pulls|gh|Open", 11.0 + REVISIT_FRESH.as_secs_f64()), "its age");
    }

    #[test]
    fn a_list_key_tells_its_state_filter() {
        let key =
            list_part("gh|00ff", "acme/app", PullRequestScope::Repository, StateFilter::Merged);
        assert!(key.starts_with(&lists_of("gh")), "{key}");
        assert!(!key.starts_with(&lists_of("g")), "another connection's id is no prefix");
        assert_eq!(list_state(&key), Some(StateFilter::Merged));
        assert_eq!(list_state(&pull_request_part("gh|acme/app|1", "detail")), None);
    }
}
