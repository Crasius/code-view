use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::flat_layout::{self, FlatLayout};
use crate::{AppPaths, atomic_file};

/// "Mark as reviewed" per file of one pull request, keyed by path and remembered with the
/// head blob id: when the author changes the file again, the mark no longer matches and the
/// file shows as unreviewed (like Azure DevOps).
///
/// The same file keeps where the user left the pull request ([`LastPlace`]), so it opens there
/// again after a restart and goes with the marks.
///
/// Stored per connection (`viewed\<connection>\<pull request>.json`) so removing a connection
/// removes its marks; files from before that layout move into their connection's folder on first
/// load ([`FlatLayout`]). Retention drops a pull request's marks after 90 days without use.
pub struct ViewedFiles {
    file: PathBuf,
    stored: StoredViewed,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredViewed {
    /// The head blob id each reviewed file had, by path.
    files: HashMap<String, String>,
    /// Marks from before it have none; a place this version cannot read is dropped, not the marks.
    #[serde(default, deserialize_with = "crate::settings::or_default")]
    place: Option<LastPlace>,
}

/// Where the user left a pull request: the tab shown and the file open in its Files tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastPlace {
    pub tab: LastTab,
    /// `None` while the Files tab showed the folder view, or before it opened a file.
    pub file: Option<String>,
}

/// A pull request page's tab, as [`LastPlace`] stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LastTab {
    Overview,
    Files,
    Commits,
    Pipelines,
}

impl ViewedFiles {
    /// `pull_request_key` identifies the PR across sessions, e.g. `connection|repo|number`.
    pub fn load(paths: &AppPaths, connection_id: &str, pull_request_key: &str) -> io::Result<Self> {
        let viewed_dir = paths.viewed_dir();
        let file = flat_layout::pull_request_file(&viewed_dir, connection_id, pull_request_key);
        FlatLayout::new(paths, viewed_dir).adopt(connection_id, pull_request_key, &file)?;
        let stored: StoredViewed = atomic_file::read_json(&file)?.unwrap_or_default();
        if !stored.files.is_empty() || stored.place.is_some() {
            atomic_file::mark_used(&file);
        }
        Ok(Self { file, stored })
    }

    /// Where the user left the pull request, if they ever did.
    pub fn last_place(&self) -> Option<&LastPlace> {
        self.stored.place.as_ref()
    }

    /// Saves where the user is now; the file is written only when the place changed.
    pub fn set_last_place(&mut self, place: LastPlace) -> io::Result<()> {
        if self.stored.place.as_ref() == Some(&place) {
            return Ok(());
        }
        self.stored.place = Some(place);
        atomic_file::write_json(&self.file, &self.stored)
    }

    pub fn is_viewed(&self, path: &str, blob_id: &str) -> bool {
        self.stored.files.get(path).is_some_and(|stored| stored == blob_id)
    }

    /// Marks or unmarks `path` and saves; the file is written from the marks in place, without a
    /// copy of them per click.
    pub fn set_viewed(&mut self, path: &str, blob_id: &str, is_viewed: bool) -> io::Result<()> {
        self.set_viewed_many([(path, blob_id)], is_viewed)
    }

    /// Marks or unmarks every (path, blob id) of `files` and saves once, for a whole folder.
    pub fn set_viewed_many<'a>(
        &mut self,
        files: impl IntoIterator<Item = (&'a str, &'a str)>,
        is_viewed: bool,
    ) -> io::Result<()> {
        for (path, blob_id) in files {
            if is_viewed {
                self.stored.files.insert(path.to_owned(), blob_id.to_owned());
            } else {
                self.stored.files.remove(path);
            }
        }
        atomic_file::write_json(&self.file, &self.stored)
    }
}

#[cfg(test)]
mod tests {
    use super::{LastPlace, LastTab, ViewedFiles};
    use crate::stable_hash;
    use crate::test_support::{TestDir, write_connections, write_file};

    #[test]
    fn mark_expires_when_blob_changes() {
        let folder = TestDir::new("viewed");
        let paths = folder.paths();
        let dir = &paths.viewed_dir();
        let mut viewed = ViewedFiles::load(&paths, "gh-1", "gh-1|cli/cli|12").unwrap();
        viewed.set_viewed("src/a.rs", "blob1", true).unwrap();
        viewed.set_viewed("src/b.rs", "blob7", true).unwrap();
        viewed.set_viewed("src/b.rs", "blob7", false).unwrap();

        let reloaded = ViewedFiles::load(&paths, "gh-1", "gh-1|cli/cli|12").unwrap();
        assert!(reloaded.is_viewed("src/a.rs", "blob1"));
        assert!(!reloaded.is_viewed("src/a.rs", "blob2"));
        assert!(!reloaded.is_viewed("src/b.rs", "blob7"), "unmarking is saved too");
        assert!(
            dir.join(stable_hash("gh-1"))
                .join(format!("{}.json", stable_hash("gh-1|cli/cli|12")))
                .exists()
        );
    }

    #[test]
    fn a_folder_is_marked_and_unmarked_at_once() {
        let folder = TestDir::new("viewed-many");
        let paths = folder.paths();
        let mut viewed = ViewedFiles::load(&paths, "gh-1", "gh-1|cli/cli|3").unwrap();
        let files = [("src/a.rs", "blob1"), ("src/b.rs", "blob2")];
        viewed.set_viewed_many(files, true).unwrap();
        let reloaded = ViewedFiles::load(&paths, "gh-1", "gh-1|cli/cli|3").unwrap();
        assert!(files.iter().all(|(path, blob)| reloaded.is_viewed(path, blob)), "saved");

        viewed.set_viewed_many(files, false).unwrap();
        let reloaded = ViewedFiles::load(&paths, "gh-1", "gh-1|cli/cli|3").unwrap();
        assert!(files.iter().all(|(path, blob)| !reloaded.is_viewed(path, blob)));
    }

    /// The place goes into the marks' file and comes back with it; a place this version cannot
    /// read leaves the marks readable.
    #[test]
    fn the_last_place_is_kept_beside_the_marks() {
        let folder = TestDir::new("viewed-place");
        let paths = folder.paths();
        let dir = &paths.viewed_dir();
        let mut viewed = ViewedFiles::load(&paths, "gh-1", "gh-1|cli/cli|5").unwrap();
        assert_eq!(viewed.last_place(), None, "never left anywhere");
        viewed.set_viewed("src/a.rs", "blob1", true).unwrap();
        let place = LastPlace { tab: LastTab::Files, file: Some("src/b.rs".into()) };
        viewed.set_last_place(place.clone()).unwrap();
        let reloaded = ViewedFiles::load(&paths, "gh-1", "gh-1|cli/cli|5").unwrap();
        assert_eq!(reloaded.last_place(), Some(&place));
        assert!(reloaded.is_viewed("src/a.rs", "blob1"), "the marks stay");

        let file =
            dir.join(stable_hash("gh-1")).join(format!("{}.json", stable_hash("gh-1|cli/cli|5")));
        std::fs::write(&file, r#"{"files":{"src/a.rs":"blob1"},"place":{"tab":"Later"}}"#).unwrap();
        let unknown = ViewedFiles::load(&paths, "gh-1", "gh-1|cli/cli|5").unwrap();
        assert_eq!(unknown.last_place(), None);
        assert!(unknown.is_viewed("src/a.rs", "blob1"));
    }

    #[test]
    fn marks_saved_before_the_per_connection_layout_are_kept() {
        let folder = TestDir::new("viewed-flat");
        let paths = folder.paths();
        let dir = &paths.viewed_dir();
        write_connections(&paths, &["gh-1"]);
        let flat = write_file(
            &dir.join(format!("{}.json", stable_hash("gh-1|cli/cli|7"))),
            br#"{"files":{"src/a.rs":"blob1"}}"#,
        );

        let viewed = ViewedFiles::load(&paths, "gh-1", "gh-1|cli/cli|7").unwrap();
        assert!(viewed.is_viewed("src/a.rs", "blob1"));
        assert!(!flat.exists());
    }
}
