//! Cheap change detection for an open pull request: the app polls a [`PullRequestRevision`] and
//! reloads the page only when it differs from the one it showed.

use std::hash::{DefaultHasher, Hash, Hasher};

use super::{PullRequestDetail, ReviewThread};

/// What the pull request page shows, reduced to values that change whenever the page would.
/// Only compared with another revision of the same pull request in the same process.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PullRequestRevision {
    /// Latest source commit; another value means new (or force-) pushed commits.
    pub head_sha: String,
    /// Fingerprint of everything else the page shows: title, description, state, votes, labels,
    /// comments. Opaque; only equality means something.
    pub marker: u64,
}

impl PullRequestRevision {
    /// The revision of a loaded detail and its threads; the fallback for platforms without a
    /// cheaper probe.
    pub fn of(detail: &PullRequestDetail, threads: &[ReviewThread]) -> Self {
        let mut fingerprint = Fingerprint::default();
        let summary = &detail.summary;
        fingerprint
            .add(&summary.title)
            .add(&detail.description)
            .add(summary.state)
            .add(summary.is_draft)
            .add(summary.updated_at)
            .add(&detail.target_sha)
            .add(detail.merge_status)
            .add(detail.closed_at);
        for label in &summary.labels {
            fingerprint.add(&label.name);
        }
        for reviewer in &summary.reviewers {
            fingerprint.add(&reviewer.identity.id).add(reviewer.vote).add(reviewer.is_required);
        }
        for thread in threads {
            fingerprint.add(&thread.id).add(thread.status).add(thread.is_outdated);
            for comment in &thread.comments {
                fingerprint.add(&comment.id).add(&comment.body).add(comment.updated_at);
            }
        }
        Self { head_sha: detail.head_sha.clone(), marker: fingerprint.finish() }
    }
}

/// Folds the parts of a [`PullRequestRevision::marker`] into one number.
#[derive(Default)]
pub struct Fingerprint(DefaultHasher);

impl Fingerprint {
    pub fn add(&mut self, part: impl Hash) -> &mut Self {
        part.hash(&mut self.0);
        self
    }

    pub fn finish(&self) -> u64 {
        self.0.finish()
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::*;
    use crate::model::{
        DiffStat, MergeStatus, PullRequestState, PullRequestSummary, ThreadComment, ThreadStatus,
        UserIdentity,
    };

    fn detail() -> PullRequestDetail {
        PullRequestDetail {
            summary: PullRequestSummary {
                repository_key: "o/r".into(),
                repository_name: "r".into(),
                number: 1,
                title: "Title".into(),
                author: UserIdentity::default(),
                state: PullRequestState::Open,
                is_draft: false,
                source_branch: "feature".into(),
                target_branch: "main".into(),
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: None,
                web_url: String::new(),
                labels: Vec::new(),
                reviewers: Vec::new(),
                checks: None,
                diff_stat: DiffStat::default(),
                head_sha: None,
                target_sha: None,
            },
            description: String::new(),
            target_sha: "base".into(),
            head_sha: "head".into(),
            merge_status: MergeStatus::Mergeable,
            git_fetch_refs: Vec::new(),
            closed_at: None,
            auto_complete: None,
        }
    }

    fn thread(comments: usize) -> ReviewThread {
        ReviewThread {
            id: "t1".into(),
            status: ThreadStatus::Active,
            anchor: None,
            is_outdated: false,
            origin: None,
            is_system: false,
            can_reply: true,
            can_resolve: true,
            comments: (0..comments)
                .map(|index| ThreadComment {
                    id: format!("c{index}"),
                    author: UserIdentity::default(),
                    body: "Looks good".into(),
                    created_at: OffsetDateTime::UNIX_EPOCH,
                    updated_at: None,
                })
                .collect(),
        }
    }

    #[test]
    fn changes_with_what_the_page_shows_and_only_then() {
        let base = PullRequestRevision::of(&detail(), &[thread(1)]);
        assert_eq!(
            base,
            PullRequestRevision::of(&detail(), &[thread(1)]),
            "same data, same revision"
        );
        assert_ne!(
            base.marker,
            PullRequestRevision::of(&detail(), &[thread(2)]).marker,
            "a new reply"
        );
        let mut retitled = detail();
        retitled.summary.title = "Other".into();
        assert_ne!(base.marker, PullRequestRevision::of(&retitled, &[thread(1)]).marker);
        let mut pushed = detail();
        pushed.head_sha = "newer".into();
        let pushed = PullRequestRevision::of(&pushed, &[thread(1)]);
        assert_eq!(pushed.marker, base.marker, "a push shows in the head, not the marker");
        assert_eq!(pushed.head_sha, "newer");
    }
}
