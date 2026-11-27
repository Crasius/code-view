//! Pull request iterations: their file changes (for anchoring new inline threads) and their
//! commits (for the version an outdated thread was written on).

use std::collections::HashMap;

use review_core::ThreadAnchor;
use serde::Deserialize;

use super::CommitRef;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Iteration {
    pub id: u32,
    /// The source branch's commit of this iteration.
    pub source_ref_commit: Option<CommitRef>,
    /// The first common commit of the source and target branches.
    pub common_ref_commit: Option<CommitRef>,
}

/// The two commits of one iteration.
#[derive(Debug)]
struct IterationShas {
    source: Option<String>,
    common: Option<String>,
}

/// The commits of each iteration, by iteration id.
#[derive(Debug, Default)]
pub(crate) struct IterationCommits(HashMap<u32, IterationShas>);

impl IterationCommits {
    pub fn new(iterations: &[Iteration]) -> Self {
        let sha =
            |commit: &Option<CommitRef>| commit.as_ref().map(|commit| commit.commit_id.clone());
        Self(
            iterations
                .iter()
                .map(|iteration| {
                    let shas = IterationShas {
                        source: sha(&iteration.source_ref_commit),
                        common: sha(&iteration.common_ref_commit),
                    };
                    (iteration.id, shas)
                })
                .collect(),
        )
    }

    /// The (left, right) commits of the diff of iteration `first` against iteration `second`.
    /// Equal iterations compare the common commit with that iteration's source commit.
    pub fn compared(&self, first: u32, second: u32) -> Option<(String, String)> {
        let head = self.0.get(&second)?;
        let base =
            if first == second { head.common.clone() } else { self.0.get(&first)?.source.clone() };
        Some((base?, head.source.clone()?))
    }
}

/// The most change entries one `iterations/{id}/changes` page returns (`$top`).
pub(crate) const CHANGES_PAGE_SIZE: u32 = 2000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChangeItem {
    pub path: Option<String>,
    /// `blob` for files, `tree` for folders; often left out for files.
    pub git_object_type: Option<String>,
    #[serde(default)]
    pub is_folder: bool,
}

impl ChangeItem {
    fn is_file(&self) -> bool {
        !self.is_folder && self.git_object_type.as_deref() != Some("tree")
    }
}

/// Changed files among `entries`, folders left out.
pub(crate) fn count_files(entries: &[ChangeEntry]) -> u64 {
    entries.iter().filter(|entry| entry.item.as_ref().is_none_or(ChangeItem::is_file)).count()
        as u64
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChangeEntry {
    pub change_tracking_id: u32,
    pub item: Option<ChangeItem>,
    pub original_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IterationChanges {
    #[serde(default)]
    pub change_entries: Vec<ChangeEntry>,
    pub next_skip: Option<u32>,
}

/// The change whose current path (or, for deleted lines, original path) is `anchor`'s file.
pub(crate) fn find_change_tracking_id(
    entries: &[ChangeEntry],
    anchor: &ThreadAnchor,
) -> Option<u32> {
    let wanted = |path: &str| path.trim_start_matches('/') == anchor.path.trim_start_matches('/');
    let wanted_old = |path: &str| {
        anchor.old_path.as_deref().is_some_and(|old| path.trim_start_matches('/') == old)
    };
    entries
        .iter()
        .find(|entry| entry.item.as_ref().and_then(|item| item.path.as_deref()).is_some_and(wanted))
        .or_else(|| {
            entries.iter().find(|entry| {
                entry.original_path.as_deref().is_some_and(|path| wanted(path) || wanted_old(path))
            })
        })
        .map(|entry| entry.change_tracking_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_change_tracking_id_by_path() {
        let changes: IterationChanges = serde_json::from_str(
            r#"{"changeEntries":[
                {"changeTrackingId":1,"changeId":1,"item":{"path":"/README.md"},"changeType":"edit"},
                {"changeTrackingId":2,"changeId":2,"item":{"path":"/src/new.rs"},"originalPath":"/src/old.rs","changeType":"rename"}
            ],"nextSkip":0}"#,
        )
        .unwrap();
        let anchor = |path: &str, old_path: Option<&str>| ThreadAnchor {
            path: path.to_owned(),
            old_path: old_path.map(str::to_owned),
            old_line: None,
            new_line: Some(1),
            range: None,
        };
        assert_eq!(
            find_change_tracking_id(&changes.change_entries, &anchor("README.md", None)),
            Some(1)
        );
        assert_eq!(
            find_change_tracking_id(&changes.change_entries, &anchor("x", Some("src/old.rs"))),
            Some(2)
        );
        assert_eq!(
            find_change_tracking_id(&changes.change_entries, &anchor("missing", None)),
            None
        );
    }

    #[test]
    fn counts_changed_files_without_folders() {
        let changes: IterationChanges = serde_json::from_str(
            r#"{"changeEntries":[
                {"changeTrackingId":1,"changeId":1,"item":{"objectId":"a","path":"/README.md"},"changeType":"edit"},
                {"changeTrackingId":2,"changeId":2,"item":{"path":"/docs","gitObjectType":"tree","isFolder":true},"changeType":"add"},
                {"changeTrackingId":3,"changeId":3,"item":{"path":"/docs/guide.md","gitObjectType":"blob"},"changeType":"add"},
                {"changeTrackingId":4,"changeId":4,"item":{"path":"/old.rs"},"originalPath":"/old.rs","changeType":"delete"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(count_files(&changes.change_entries), 3);
        assert_eq!(changes.next_skip, None, "one page holds them all");
    }
}
