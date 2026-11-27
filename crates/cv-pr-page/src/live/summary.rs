//! What a live update brings, worked out from the shown and the newer data, and put into the
//! few words the banner and the "Updated" notice show.

use std::collections::HashSet;

use review_core::{PullRequestDetail, PullRequestState, ReviewThread};
use review_git::CommitInfo;

/// The shown or the newer state of a pull request page.
pub struct PageData<'a> {
    pub detail: &'a PullRequestDetail,
    pub threads: &'a [ReviewThread],
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent fact the banner names as its own part"
)]
pub struct ChangeSummary {
    /// The head moved.
    pub pushed: bool,
    /// The new head does not contain the shown one (rebase, amend).
    pub force_pushed: bool,
    /// Commits in the new head that the shown one lacks; 0 when the head did not move or the
    /// commits are not known yet (the git mirror was still being prepared).
    pub new_commits: usize,
    /// Who wrote those commits, each once, newest first.
    pub commit_authors: Vec<String>,
    /// The author of each comment that was not there before, oldest first.
    pub new_comment_authors: Vec<String>,
    pub reviews_changed: bool,
    pub threads_resolved: bool,
    /// Title, description or labels.
    pub details_changed: bool,
    /// The new state when it changed: merged, closed, reopened, draft or published.
    pub state: Option<StateChange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateChange {
    Merged,
    Closed,
    Reopened,
    MarkedAsDraft,
    Published,
}

pub fn describe(
    shown: &PageData<'_>,
    newer: &PageData<'_>,
    commits: &[CommitInfo],
    force_pushed: bool,
) -> ChangeSummary {
    let (old, new) = (shown.detail, newer.detail);
    let known_comments: HashSet<&str> = shown
        .threads
        .iter()
        .flat_map(|thread| &thread.comments)
        .map(|comment| comment.id.as_str())
        .collect();
    let new_comment_authors = newer
        .threads
        .iter()
        .filter(|thread| !thread.is_system)
        .flat_map(|thread| &thread.comments)
        .filter(|comment| !known_comments.contains(comment.id.as_str()))
        .map(|comment| comment.author.display_name.clone())
        .collect();
    let mut commit_authors: Vec<String> = Vec::new();
    for commit in commits {
        if !commit_authors.contains(&commit.author_name) {
            commit_authors.push(commit.author_name.clone());
        }
    }
    let votes = |detail: &PullRequestDetail| {
        detail
            .summary
            .reviewers
            .iter()
            .map(|reviewer| (reviewer.identity.id.clone(), reviewer.vote))
            .collect::<Vec<_>>()
    };
    let statuses = |threads: &[ReviewThread]| {
        threads.iter().map(|thread| (thread.id.clone(), thread.status)).collect::<HashSet<_>>()
    };
    let resolved_before = statuses(shown.threads);
    let threads_resolved = newer.threads.iter().any(|thread| {
        let was_known = shown.threads.iter().any(|known| known.id == thread.id);
        was_known && !resolved_before.contains(&(thread.id.clone(), thread.status))
    });
    let labels = |detail: &PullRequestDetail| {
        detail.summary.labels.iter().map(|label| label.name.clone()).collect::<Vec<_>>()
    };
    let details_changed = old.summary.title != new.summary.title
        || old.description != new.description
        || labels(old) != labels(new);
    let state = match (old.summary.state, new.summary.state) {
        (before, after) if before == after => match (old.summary.is_draft, new.summary.is_draft) {
            (false, true) => Some(StateChange::MarkedAsDraft),
            (true, false) => Some(StateChange::Published),
            _ => None,
        },
        (_, PullRequestState::Merged) => Some(StateChange::Merged),
        (_, PullRequestState::Closed) => Some(StateChange::Closed),
        (_, PullRequestState::Open) => Some(StateChange::Reopened),
    };
    ChangeSummary {
        pushed: old.head_sha != new.head_sha,
        force_pushed: force_pushed && old.head_sha != new.head_sha,
        new_commits: commits.len(),
        commit_authors,
        new_comment_authors,
        reviews_changed: votes(old) != votes(new),
        threads_resolved,
        details_changed,
        state,
    }
}

impl ChangeSummary {
    /// Nothing the user would notice changed (a recomputed merge status, an edited system note).
    pub fn is_empty(&self) -> bool {
        self.parts().is_empty()
    }

    /// The banner's title: pushes are the news that matter most for a review in progress.
    pub fn title(&self) -> &'static str {
        if self.pushed { "New changes pushed" } else { "New activity on this pull request" }
    }

    /// "2 new commits by Ana", "1 new comment by Ben", "Reviews changed", most important first.
    pub fn parts(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.pushed {
            let commits = match self.new_commits {
                0 => "New commits".to_owned(),
                1 => "1 new commit".to_owned(),
                count => format!("{count} new commits"),
            };
            let force = if self.force_pushed { " (force-pushed)" } else { "" };
            parts.push(format!("{commits}{}{force}", by(&self.commit_authors)));
        }
        if !self.new_comment_authors.is_empty() {
            let count = self.new_comment_authors.len();
            let noun = if count == 1 { "comment" } else { "comments" };
            let mut authors: Vec<String> = Vec::new();
            for author in &self.new_comment_authors {
                if !authors.contains(author) {
                    authors.push(author.clone());
                }
            }
            parts.push(format!("{count} new {noun}{}", by(&authors)));
        }
        if let Some(state) = self.state {
            parts.push(
                match state {
                    StateChange::Merged => "Merged",
                    StateChange::Closed => "Closed",
                    StateChange::Reopened => "Reopened",
                    StateChange::MarkedAsDraft => "Marked as draft",
                    StateChange::Published => "Published",
                }
                .to_owned(),
            );
        }
        if self.reviews_changed {
            parts.push("Reviews changed".to_owned());
        }
        if self.threads_resolved {
            parts.push("Threads resolved or reopened".to_owned());
        }
        if self.details_changed {
            parts.push("Title, description or labels edited".to_owned());
        }
        parts
    }
}

/// " by Ana", " by Ana and Ben", " by Ana and 2 others"; empty without names.
fn by(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => format!(" by {one}"),
        [first, second] => format!(" by {first} and {second}"),
        [first, rest @ ..] => format!(" by {first} and {} others", rest.len()),
    }
}

#[cfg(test)]
mod tests {
    use cv_pr_threads::samples;
    use review_core::{MergeStatus, ReviewVote, Reviewer, ThreadStatus, UserIdentity};

    use super::*;

    fn detail(head: &str) -> PullRequestDetail {
        PullRequestDetail { head_sha: head.into(), ..cv_testkit::samples::detail("o/r", 1) }
    }

    fn thread(id: &str, comments: &[(&str, &str)]) -> ReviewThread {
        let comments = comments
            .iter()
            .map(|(id, author)| samples::comment(id, &samples::person(author, author), 0, "Text"))
            .collect();
        samples::thread(id, ThreadStatus::Active, None, comments)
    }

    fn commit(author: &str) -> CommitInfo {
        CommitInfo {
            sha: "c".into(),
            parents: Vec::new(),
            author_name: author.into(),
            author_email: String::new(),
            authored_at: 0,
            subject: "Change".into(),
        }
    }

    #[test]
    fn names_pushed_commits_and_their_authors() {
        let (old, new) = (detail("a"), detail("b"));
        let summary = describe(
            &PageData { detail: &old, threads: &[] },
            &PageData { detail: &new, threads: &[] },
            &[commit("Ana"), commit("Ana")],
            false,
        );
        assert_eq!(summary.parts(), ["2 new commits by Ana"]);
        assert_eq!(summary.title(), "New changes pushed");
        let rewritten = describe(
            &PageData { detail: &old, threads: &[] },
            &PageData { detail: &new, threads: &[] },
            &[commit("Ana"), commit("Ben"), commit("Cy")],
            true,
        );
        assert_eq!(rewritten.parts(), ["3 new commits by Ana and 2 others (force-pushed)"]);
    }

    #[test]
    fn counts_only_comments_that_were_not_there() {
        let detail = detail("a");
        let shown = [thread("t1", &[("c1", "Ana")])];
        let newer = [thread("t1", &[("c1", "Ana"), ("c2", "Ben")]), thread("t2", &[("c3", "Ben")])];
        let summary = describe(
            &PageData { detail: &detail, threads: &shown },
            &PageData { detail: &detail, threads: &newer },
            &[],
            false,
        );
        assert_eq!(summary.parts(), ["2 new comments by Ben"]);
        assert_eq!(summary.title(), "New activity on this pull request");
    }

    #[test]
    fn notices_votes_states_and_resolved_threads_but_not_noise() {
        let old = detail("a");
        let mut new = detail("a");
        new.merge_status = MergeStatus::Conflicts;
        let quiet = describe(
            &PageData { detail: &old, threads: &[] },
            &PageData { detail: &new, threads: &[] },
            &[],
            false,
        );
        assert!(quiet.is_empty(), "a recomputed merge status is not news");
        new.summary.reviewers.push(Reviewer {
            identity: UserIdentity::default(),
            vote: ReviewVote::Approved,
            is_required: false,
            is_group: false,
        });
        new.summary.state = PullRequestState::Merged;
        let mut resolved = thread("t1", &[("c1", "Ana")]);
        resolved.status = ThreadStatus::Resolved;
        let summary = describe(
            &PageData { detail: &old, threads: &[thread("t1", &[("c1", "Ana")])] },
            &PageData { detail: &new, threads: &[resolved] },
            &[],
            false,
        );
        assert_eq!(summary.parts(), ["Merged", "Reviews changed", "Threads resolved or reopened"]);
    }
}
