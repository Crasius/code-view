//! Which rows of a listing changed since the user last saw them; the row's left edge says so
//! (`list_rows::edge`). A row counts as changed when an answer of the host shows it new, or with
//! another head, update time, state, draft flag or votes than the rows shown before
//! ([`super::statuses::fingerprint`]). The first rows a listing shows (from the page cache or its
//! first answer) are the baseline and mark nothing, and the user's own writes move the baseline
//! with them, so a vote of theirs never marks its row. A row stays marked until the pointer rests
//! on it or it is opened, or until the host stops listing it.

use std::collections::{HashMap, HashSet};

use review_core::PullRequestSummary;

use super::statuses::fingerprint;

/// A row of one listing: its repository and number.
type RowKey = (String, u64);

#[derive(Default)]
pub(crate) struct RowChanges {
    /// The fingerprint of every row shown; `None` before the first rows.
    shown: Option<HashMap<RowKey, u64>>,
    changed: HashSet<RowKey>,
}

impl RowChanges {
    /// `rows`, if any, are the baseline.
    pub(crate) fn new(rows: Option<&[PullRequestSummary]>) -> Self {
        Self { shown: rows.map(fingerprints), changed: HashSet::new() }
    }

    /// The rows on screen changed by the user's own writes: the new baseline, nothing marked.
    pub(crate) fn follow(&mut self, rows: &[PullRequestSummary]) {
        self.shown = Some(fingerprints(rows));
    }

    /// An answer of the host: marks the rows it shows new or changed against the rows shown before.
    pub(crate) fn answered(&mut self, rows: &[PullRequestSummary]) {
        let now = fingerprints(rows);
        if let Some(before) = &self.shown {
            let changed = now.iter().filter(|(key, print)| before.get(*key) != Some(*print));
            self.changed.extend(changed.map(|(key, _)| key.clone()));
        }
        self.changed.retain(|key| now.contains_key(key));
        self.shown = Some(now);
    }

    pub(crate) fn is_changed(&self, repository_key: &str, number: u64) -> bool {
        // Few rows are marked at a time, so a scan beats building a key per painted row.
        self.changed
            .iter()
            .any(|(repository, marked)| *marked == number && repository == repository_key)
    }

    /// The user saw the row: it is no longer marked.
    pub(crate) fn seen(&mut self, repository_key: &str, number: u64) {
        self.changed
            .retain(|(repository, marked)| *marked != number || repository != repository_key);
    }
}

fn fingerprints(rows: &[PullRequestSummary]) -> HashMap<RowKey, u64> {
    rows.iter().map(|row| ((row.repository_key.clone(), row.number), fingerprint(row))).collect()
}

#[cfg(test)]
mod tests {
    use review_core::{ReviewVote, Reviewer, UserIdentity};

    use super::RowChanges;
    use crate::pull_request_list::list_rows::tests::row;

    #[test]
    fn rows_the_host_shows_new_or_changed_stay_marked_until_seen() {
        let mut changes = RowChanges::new(Some(&[row(1), row(2), row(3)]));
        assert!(!changes.is_changed("group/app", 1), "the cached rows are the baseline");
        let pushed = review_core::PullRequestSummary { head_sha: Some("pushed".into()), ..row(2) };
        changes.answered(&[row(1), pushed.clone(), row(3), row(4)]);
        assert!(!changes.is_changed("group/app", 1), "unchanged");
        assert!(changes.is_changed("group/app", 2), "a push");
        assert!(changes.is_changed("group/app", 4), "new in the list");

        changes.seen("group/app", 2);
        assert!(!changes.is_changed("group/app", 2), "the pointer rested on it");
        changes.answered(&[row(1), pushed.clone(), row(3)]);
        assert!(!changes.is_changed("group/app", 4), "the host no longer lists it");
        assert!(!changes.is_changed("group/app", 2), "seen and unchanged since");

        // The user's own vote moves the baseline, so the host's answer showing it marks nothing.
        let me = Reviewer {
            identity: UserIdentity { id: "me".into(), ..UserIdentity::default() },
            vote: ReviewVote::Approved,
            is_required: false,
            is_group: false,
        };
        let voted = review_core::PullRequestSummary { reviewers: vec![me], ..row(1) };
        changes.follow(&[voted.clone(), pushed.clone(), row(3)]);
        changes.answered(&[voted, pushed, row(3)]);
        assert!(!changes.is_changed("group/app", 1), "the user's own vote");
    }

    #[test]
    fn the_first_answer_without_cached_rows_marks_nothing() {
        let mut changes = RowChanges::new(None);
        changes.answered(&[row(1), row(2)]);
        assert!(!changes.is_changed("group/app", 1) && !changes.is_changed("group/app", 2));
    }
}
