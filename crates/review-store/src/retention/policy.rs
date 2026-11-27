//! The age and size rules, free of the file system: which entries go, in which order.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// One thing that is kept or deleted as a whole: a log file, a viewed-state file, a mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub path: PathBuf,
    pub bytes: u64,
    pub last_used: SystemTime,
}

/// Entries unused for longer than `max_age` go; if the rest is still over `max_total`,
/// the least recently used go next until it fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_age: Duration,
    pub max_total: Option<u64>,
}

/// What a pass removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub removed: usize,
    pub freed: u64,
    /// Entries the rules picked but `retire` refused (in use).
    pub kept_in_use: usize,
}

impl Outcome {
    pub(crate) fn add(&mut self, other: Outcome) {
        self.removed += other.removed;
        self.freed += other.freed;
        self.kept_in_use += other.kept_in_use;
    }

    fn record(&mut self, entry: &Entry, is_retired: bool) {
        if is_retired {
            self.removed += 1;
            self.freed += entry.bytes;
        } else {
            self.kept_in_use += 1;
        }
    }
}

/// Applies `limits` at `now`. `retire` removes one entry and returns false when it is in use;
/// a refused entry still counts towards the total, so the next oldest goes instead.
pub(crate) fn apply(
    mut entries: Vec<Entry>,
    now: SystemTime,
    limits: Limits,
    retire: &mut dyn FnMut(&Entry) -> bool,
) -> Outcome {
    let mut outcome = Outcome::default();
    entries.sort_by_key(|entry| entry.last_used);
    let mut kept = Vec::with_capacity(entries.len());
    for entry in entries {
        if is_expired(&entry, now, limits.max_age) {
            let is_retired = retire(&entry);
            outcome.record(&entry, is_retired);
            if !is_retired {
                kept.push(entry);
            }
        } else {
            kept.push(entry);
        }
    }
    let Some(max_total) = limits.max_total else {
        return outcome;
    };
    let mut total: u64 = kept.iter().map(|entry| entry.bytes).sum();
    for entry in &kept {
        if total <= max_total {
            break;
        }
        // Already refused as expired; asking again would count it twice.
        if is_expired(entry, now, limits.max_age) {
            continue;
        }
        let is_retired = retire(entry);
        outcome.record(entry, is_retired);
        if is_retired {
            total -= entry.bytes;
        }
    }
    outcome
}

/// Asks `retire` for every one of `entries`, whatever their age and size: removing a connection's
/// data and clearing the cache.
pub(crate) fn retire_all(
    entries: impl IntoIterator<Item = Entry>,
    retire: &mut dyn FnMut(&Entry) -> bool,
) -> Outcome {
    let mut outcome = Outcome::default();
    for entry in entries {
        outcome.record(&entry, retire(&entry));
    }
    outcome
}

/// A clock set back (last use in the future) never expires anything.
fn is_expired(entry: &Entry, now: SystemTime, max_age: Duration) -> bool {
    now.duration_since(entry.last_used).is_ok_and(|age| age > max_age)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::retention::DAY;

    fn entry(name: &str, bytes: u64, days_ago: u32, now: SystemTime) -> Entry {
        Entry { path: PathBuf::from(name), bytes, last_used: now - DAY * days_ago }
    }

    fn names(retired: &[PathBuf]) -> Vec<&str> {
        retired.iter().map(|path| path.to_str().unwrap()).collect()
    }

    #[test]
    fn removes_only_entries_older_than_the_age_limit() {
        let now = SystemTime::UNIX_EPOCH + DAY * 1000;
        let entries =
            vec![entry("fresh", 10, 1, now), entry("old", 20, 8, now), entry("edge", 5, 7, now)];
        let mut retired = Vec::new();
        let limits = Limits { max_age: DAY * 7, max_total: None };
        let outcome = apply(entries, now, limits, &mut |entry| {
            retired.push(entry.path.clone());
            true
        });
        assert_eq!(names(&retired), ["old"]);
        assert_eq!(outcome, Outcome { removed: 1, freed: 20, kept_in_use: 0 });
    }

    #[test]
    fn over_the_size_limit_least_recently_used_go_first() {
        let now = SystemTime::UNIX_EPOCH + DAY * 1000;
        let entries = vec![
            entry("newest", 40, 0, now),
            entry("oldest", 40, 3, now),
            entry("middle", 40, 2, now),
        ];
        let mut retired = Vec::new();
        let limits = Limits { max_age: DAY * 30, max_total: Some(50) };
        let outcome = apply(entries, now, limits, &mut |entry| {
            retired.push(entry.path.clone());
            true
        });
        assert_eq!(names(&retired), ["oldest", "middle"]);
        assert_eq!(outcome.freed, 80);
    }

    #[test]
    fn an_entry_in_use_is_skipped_and_the_next_oldest_goes() {
        let now = SystemTime::UNIX_EPOCH + DAY * 1000;
        let entries =
            vec![entry("open", 60, 5, now), entry("idle", 60, 4, now), entry("new", 60, 0, now)];
        let mut retired = Vec::new();
        let limits = Limits { max_age: DAY * 30, max_total: Some(100) };
        let outcome = apply(entries, now, limits, &mut |entry| {
            if entry.path == Path::new("open") {
                return false;
            }
            retired.push(entry.path.clone());
            true
        });
        assert_eq!(names(&retired), ["idle", "new"]);
        assert_eq!(outcome, Outcome { removed: 2, freed: 120, kept_in_use: 1 });
    }

    #[test]
    fn an_expired_entry_in_use_is_asked_once() {
        let now = SystemTime::UNIX_EPOCH + DAY * 1000;
        let entries = vec![entry("open", 100, 40, now)];
        let mut asked = 0;
        let limits = Limits { max_age: DAY * 30, max_total: Some(10) };
        let outcome = apply(entries, now, limits, &mut |_| {
            asked += 1;
            false
        });
        assert_eq!(asked, 1);
        assert_eq!(outcome.kept_in_use, 1);
    }

    #[test]
    fn retiring_all_counts_what_went_and_what_was_in_use() {
        let now = SystemTime::UNIX_EPOCH + DAY * 1000;
        let entries = [entry("open", 7, 0, now), entry("idle", 5, 0, now)];
        let outcome = retire_all(entries, &mut |entry| entry.path != Path::new("open"));
        assert_eq!(outcome, Outcome { removed: 1, freed: 5, kept_in_use: 1 });
    }

    #[test]
    fn a_last_use_in_the_future_never_expires() {
        let now = SystemTime::UNIX_EPOCH + DAY * 1000;
        let future = Entry { path: PathBuf::from("future"), bytes: 1, last_used: now + DAY };
        let limits = Limits { max_age: DAY, max_total: None };
        let outcome = apply(vec![future], now, limits, &mut |_| true);
        assert_eq!(outcome.removed, 0);
    }
}
