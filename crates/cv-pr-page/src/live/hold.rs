//! Whether an update that was read ahead may go on screen while the user writes a comment.
//!
//! Only a draft the update would move is a reason to wait: a new comment in a file whose diff
//! changes, or a reply whose thread changes or sits in such a file. A draft anywhere else, the
//! general comment, the label and reviewer editors and every other field keep their state through
//! the swap (they live outside the reloaded data), so they never hold an update back.

use std::collections::HashSet;

use review_core::ReviewThread;

/// The comments the user is writing where an update could move them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Drafts {
    /// Files whose diff holds an open new-comment composer.
    pub files: Vec<String>,
    /// Threads with an open reply box.
    pub replies: Vec<String>,
}

/// What an update changes on the page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Reach {
    /// Paths whose diff changes (both paths of a rename); `None` when the commits moved but the
    /// files could not be worked out (the page's own git data was not ready), so any may have.
    pub files: Option<HashSet<String>>,
    /// Threads that came, went, or whose comments, status or place changed.
    pub threads: HashSet<String>,
}

impl Reach {
    pub(crate) fn new(
        files: Option<HashSet<String>>,
        shown: &[ReviewThread],
        newer: &[ReviewThread],
    ) -> Self {
        let changed = |from: &[ReviewThread], to: &[ReviewThread]| {
            from.iter()
                .filter(|thread| !to.iter().any(|other| other == *thread))
                .map(|thread| thread.id.clone())
                .collect::<Vec<_>>()
        };
        let threads = changed(shown, newer).into_iter().chain(changed(newer, shown)).collect();
        Self { files, threads }
    }

    fn touches_file(&self, path: &str) -> bool {
        self.files.as_ref().is_none_or(|files| files.contains(path))
    }

    /// True when applying the update would move or change something the user is writing in.
    /// `threads` are the shown ones, which place each reply.
    pub(crate) fn disturbs(&self, drafts: &Drafts, threads: &[ReviewThread]) -> bool {
        let file_of = |id: &str| {
            threads
                .iter()
                .find(|thread| thread.id == id)
                .and_then(|thread| thread.anchor.as_ref())
                .map(|anchor| anchor.path.as_str())
        };
        drafts.files.iter().any(|path| self.touches_file(path))
            || drafts.replies.iter().any(|id| {
                self.threads.contains(id) || file_of(id).is_some_and(|path| self.touches_file(path))
            })
    }
}

#[cfg(test)]
mod tests {
    use cv_pr_threads::samples;
    use review_core::{ThreadStatus, UserIdentity};

    use super::*;

    /// The comments are written at a fixed time: `Reach::new` compares whole threads, so a thread
    /// built twice has to come out equal.
    fn thread(id: &str, path: Option<&str>, comments: &[&str]) -> ReviewThread {
        let author = UserIdentity::default();
        let comments =
            comments.iter().map(|id| samples::comment_at(id, &author, 0, "Text")).collect();
        let anchor = path.map(|path| samples::lines(path, 3, 3));
        samples::thread(id, ThreadStatus::Active, anchor, comments)
    }

    fn files(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|path| (*path).to_owned()).collect()
    }

    fn composer_in(path: &str) -> Drafts {
        Drafts { files: vec![path.into()], replies: Vec::new() }
    }

    fn reply_on(id: &str) -> Drafts {
        Drafts { files: Vec::new(), replies: vec![id.into()] }
    }

    #[test]
    fn a_new_comment_waits_only_for_a_push_that_changes_its_file() {
        let pushed = Reach::new(Some(files(&["src/lib.rs"])), &[], &[]);
        assert!(pushed.disturbs(&composer_in("src/lib.rs"), &[]));
        assert!(!pushed.disturbs(&composer_in("src/other.rs"), &[]), "another file stays put");
        let unknown = Reach::new(None, &[], &[]);
        assert!(unknown.disturbs(&composer_in("src/other.rs"), &[]), "unknown files may all move");
        let comments_only =
            Reach::new(Some(files(&[])), &[], &[thread("t1", Some("src/lib.rs"), &["c1"])]);
        assert!(!comments_only.disturbs(&composer_in("src/lib.rs"), &[]), "no push, no move");
    }

    #[test]
    fn a_reply_waits_for_its_thread_or_its_file_but_not_for_others() {
        let shown = [thread("t1", Some("src/lib.rs"), &["c1"]), thread("t2", None, &["c2"])];
        let answered =
            [thread("t1", Some("src/lib.rs"), &["c1", "c3"]), thread("t2", None, &["c2"])];
        let reach = Reach::new(Some(files(&[])), &shown, &answered);
        assert!(reach.disturbs(&reply_on("t1"), &shown), "its own thread got a comment");
        assert!(!reach.disturbs(&reply_on("t2"), &shown), "another thread changed");
        let pushed = Reach::new(Some(files(&["src/lib.rs"])), &shown, &shown);
        assert!(pushed.disturbs(&reply_on("t1"), &shown), "its file's diff changed");
        assert!(!pushed.disturbs(&reply_on("t2"), &shown), "a general thread has no file");
        let new_thread = Reach::new(Some(files(&[])), &shown[..1], &shown);
        assert!(new_thread.threads.contains("t2"), "a thread that came in counts as changed");
        assert!(!pushed.disturbs(&Drafts::default(), &shown), "nobody writes: nothing waits");
    }
}
