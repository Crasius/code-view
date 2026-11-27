use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::flat_layout::{self, FlatLayout};
use crate::{AppPaths, atomic_file};

/// What the app remembers about the conversations of one pull request on this machine: which
/// comments the user has seen (for "new replies" and the unread filter) and unsent drafts, so a
/// crash, a restart or a closed composer never loses text.
///
/// Stored per connection (`threads\<connection>\<pull request>.json`) so removing a connection
/// removes its drafts and read marks, and a new connection that reuses the id starts empty. Files
/// from before that layout move into their connection's folder on first load (`FlatLayout`).
pub struct ThreadStore {
    /// `None` for a store that is never written.
    file: Option<PathBuf>,
    stored: StoredThreads,
    is_dirty: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredThreads {
    /// Unix seconds of the first visit. Comments older than that count as read.
    first_visit: Option<i64>,
    /// Per thread id: unix seconds of the newest comment the user has seen.
    seen: HashMap<String, i64>,
    drafts: BTreeMap<String, Draft>,
}

/// An unsent comment. `anchor` is set for a new inline thread, so its composer can open again on
/// the same lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<DraftAnchor>,
}

/// [`review_core::ThreadAnchor`] in a storable shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftAnchor {
    pub path: String,
    pub old_path: Option<String>,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub range: Option<DraftRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftRange {
    pub start_old_line: Option<u32>,
    pub start_new_line: Option<u32>,
    /// The range was picked in the head file.
    pub is_new_side: bool,
    /// `[start old, start new, end old, end new, start offset, end offset]` of a new range.
    pub detail: Option<[u32; 6]>,
}

impl ThreadStore {
    /// `pull_request_key` identifies the PR across sessions, e.g. `connection|repo|number`.
    pub fn load(paths: &AppPaths, connection_id: &str, pull_request_key: &str) -> io::Result<Self> {
        let file =
            flat_layout::pull_request_file(&paths.threads_dir(), connection_id, pull_request_key);
        FlatLayout::new(paths, paths.threads_dir()).adopt(
            connection_id,
            pull_request_key,
            &file,
        )?;
        let stored: StoredThreads = atomic_file::read_json(&file)?.unwrap_or_default();
        if stored.first_visit.is_some() {
            atomic_file::mark_used(&file);
        }
        Ok(Self { file: Some(file), stored, is_dirty: false })
    }

    /// A store that is never written, for when the file could not be read.
    pub fn in_memory() -> Self {
        Self { file: None, stored: StoredThreads::default(), is_dirty: false }
    }

    /// The first visit: everything there is now counts as read. Later calls change nothing.
    pub fn start_visit(
        &mut self,
        now: i64,
        newest_by_thread: impl IntoIterator<Item = (String, i64)>,
    ) {
        if self.stored.first_visit.is_some() {
            return;
        }
        self.stored.first_visit = Some(now);
        self.stored.seen.extend(newest_by_thread);
        self.is_dirty = true;
    }

    pub fn has_visited(&self) -> bool {
        self.stored.first_visit.is_some()
    }

    /// Unix seconds up to which the comments of `thread_id` are read: the newest seen comment,
    /// else the first visit.
    pub fn seen_until(&self, thread_id: &str) -> Option<i64> {
        self.stored.seen.get(thread_id).copied().or(self.stored.first_visit)
    }

    /// Records that the user saw `thread_id` up to `newest`; never moves backwards.
    pub fn mark_seen(&mut self, thread_id: &str, newest: i64) {
        let entry = self.stored.seen.entry(thread_id.to_owned()).or_insert(i64::MIN);
        if newest > *entry {
            *entry = newest;
            self.is_dirty = true;
        }
    }

    pub fn draft(&self, key: &str) -> Option<&Draft> {
        self.stored.drafts.get(key)
    }

    pub fn drafts(&self) -> impl Iterator<Item = (&str, &Draft)> {
        self.stored.drafts.iter().map(|(key, draft)| (key.as_str(), draft))
    }

    /// Keeps `draft` under `key`; a draft without text is removed.
    pub fn set_draft(&mut self, key: &str, draft: Draft) {
        if draft.text.trim().is_empty() {
            self.remove_draft(key);
            return;
        }
        if self.stored.drafts.get(key) != Some(&draft) {
            self.stored.drafts.insert(key.to_owned(), draft);
            self.is_dirty = true;
        }
    }

    pub fn remove_draft(&mut self, key: &str) {
        if self.stored.drafts.remove(key).is_some() {
            self.is_dirty = true;
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.is_dirty
    }

    /// Writes the changes since the last save, if any.
    pub fn save(&mut self) -> io::Result<()> {
        let Some(file) = &self.file else {
            self.is_dirty = false;
            return Ok(());
        };
        if self.is_dirty {
            atomic_file::write_json(file, &self.stored)?;
            self.is_dirty = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flat_layout::pull_request_file;
    use crate::test_support::{TestDir, write_connections, write_file};

    /// A drafts file in the layout before the per-connection folders.
    fn flat_draft(paths: &AppPaths, key: &str, text: &str) -> PathBuf {
        let flat = FlatLayout::new(paths, paths.threads_dir()).flat_file(key);
        let json = format!(r#"{{"drafts":{{"reply:t1":{{"text":"{text}"}}}}}}"#);
        write_file(&flat, json.as_bytes())
    }

    #[test]
    fn the_first_visit_reads_everything_and_later_comments_are_new() {
        let folder = TestDir::new("threads-seen");
        let paths = folder.paths();
        let mut store = ThreadStore::load(&paths, "gh", "gh|cli/cli|7").unwrap();
        assert_eq!(store.seen_until("t1"), None);
        store.start_visit(1_000, [("t1".to_owned(), 900)]);
        store.start_visit(5_000, [("t1".to_owned(), 4_000)]);
        assert_eq!(store.seen_until("t1"), Some(900), "a second visit keeps the first baseline");
        assert_eq!(
            store.seen_until("later"),
            Some(1_000),
            "a thread opened later is new after the first visit"
        );
        store.mark_seen("t1", 1_200);
        store.mark_seen("t1", 1_100);
        assert_eq!(store.seen_until("t1"), Some(1_200), "seen never moves backwards");
        store.save().unwrap();
        let reloaded = ThreadStore::load(&paths, "gh", "gh|cli/cli|7").unwrap();
        assert_eq!(reloaded.seen_until("t1"), Some(1_200));
        assert!(reloaded.has_visited());
        assert!(pull_request_file(&paths.threads_dir(), "gh", "gh|cli/cli|7").exists());
    }

    #[test]
    fn drafts_survive_a_restart_and_empty_ones_are_dropped() {
        let folder = TestDir::new("threads-drafts");
        let paths = folder.paths();
        let mut store = ThreadStore::load(&paths, "gl", "gl|group/app|3").unwrap();
        let anchor = DraftAnchor {
            path: "src/a.rs".into(),
            old_path: None,
            old_line: None,
            new_line: Some(12),
            range: Some(DraftRange {
                start_old_line: None,
                start_new_line: Some(9),
                is_new_side: true,
                detail: Some([8, 9, 11, 12, 1, 7]),
            }),
        };
        store.set_draft(
            "new:src/a.rs",
            Draft { text: "Why here?".into(), anchor: Some(anchor.clone()) },
        );
        store.set_draft("reply:t1", Draft { text: "Agreed".into(), anchor: None });
        assert!(store.is_dirty());
        store.save().unwrap();
        assert!(!store.is_dirty());

        let mut reloaded = ThreadStore::load(&paths, "gl", "gl|group/app|3").unwrap();
        assert_eq!(
            reloaded.draft("new:src/a.rs").and_then(|draft| draft.anchor.clone()),
            Some(anchor)
        );
        assert_eq!(reloaded.drafts().count(), 2);
        reloaded.set_draft("reply:t1", Draft { text: "  ".into(), anchor: None });
        assert!(reloaded.draft("reply:t1").is_none(), "clearing the text drops the draft");
    }

    /// The upgrade: a draft saved before the per-connection layout opens again for its connection
    /// and moves into that connection's folder; a second load finds it there.
    #[test]
    fn drafts_saved_before_the_per_connection_layout_are_kept() {
        let folder = TestDir::new("threads-flat");
        let paths = folder.paths();
        write_connections(&paths, &["gh-1", "gh-2"]);
        let flat = flat_draft(&paths, "gh-1|cli/cli|7", "Kept");
        let other = flat_draft(&paths, "gh-2|cli/cli|8", "Other");

        let store = ThreadStore::load(&paths, "gh-1", "gh-1|cli/cli|7").unwrap();
        assert_eq!(store.draft("reply:t1").map(|draft| draft.text.as_str()), Some("Kept"));
        assert!(!flat.exists(), "the flat file moved");
        assert!(pull_request_file(&paths.threads_dir(), "gh-1", "gh-1|cli/cli|7").exists());
        assert!(other.exists(), "another pull request's file waits for its own load");

        let reloaded = ThreadStore::load(&paths, "gh-1", "gh-1|cli/cli|7").unwrap();
        assert_eq!(reloaded.draft("reply:t1").map(|draft| draft.text.as_str()), Some("Kept"));
        let store = ThreadStore::load(&paths, "gh-2", "gh-2|cli/cli|8").unwrap();
        assert_eq!(store.draft("reply:t1").map(|draft| draft.text.as_str()), Some("Other"));
    }
}
