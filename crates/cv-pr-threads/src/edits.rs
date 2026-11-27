//! The user's own comments while they are edited or deleted. An edit opens the composer on the
//! comment's text in place of its body; once saved, the card shows the new text at once and then
//! the platform's copy, until the reloaded threads carry it. A deletion is confirmed inside the
//! card, hides the comment once the platform has it, and the reloaded threads drop it (and the
//! thread with its last comment, as the platform does). A failure keeps the text or the comment
//! on screen with the reason and Retry.
//!
//! Changes are few at a time, so they sit in a short list that a card scans without allocating.

use std::sync::Arc;

use cv_pr_session::session::PullRequestSession;
use cv_runtime::load::Load;
use cv_runtime::toasts::Toasts;
use cv_services::Services;
use cv_services::connection::access_issues::AccessIssues;
use review_core::provider::CommentActions;
use review_core::{Provider, ReviewThread, ThreadComment, UserIdentity};

/// Why the last save or deletion did not go through. `needs_token`: the platform refused the
/// token, so "Fix token" is the way out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub message: String,
    pub needs_token: bool,
}

/// How one comment shows this frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum View {
    Plain,
    /// The composer replaces the body; its text is [`CommentEdits::text_mut`].
    Editing {
        failure: Option<Failure>,
    },
    /// The new text on its way: shown at once, inert.
    Saving {
        body: String,
    },
    /// The platform's copy of the edited comment, until the reloaded threads carry it.
    Saved(ThreadComment),
    /// The inline "Delete this comment?" question, with the reason when the last try failed.
    Confirming {
        failure: Option<Failure>,
    },
    Deleting,
    /// The platform deleted it; hidden until the reloaded threads drop it.
    Deleted,
}

enum Change {
    /// `wants_focus`: the composer takes the keyboard focus next frame.
    Editing {
        text: String,
        failure: Option<Failure>,
        wants_focus: bool,
    },
    Saving {
        text: String,
        load: Load<ThreadComment>,
    },
    Saved {
        comment: ThreadComment,
        after_version: u64,
    },
    /// `wants_focus`: the question's Cancel takes the keyboard focus next frame.
    Confirming {
        failure: Option<Failure>,
        wants_focus: bool,
    },
    Deleting {
        load: Load<()>,
    },
    Deleted {
        after_version: u64,
    },
}

struct Entry {
    thread_id: String,
    comment_id: String,
    change: Change,
}

#[derive(Default)]
pub(crate) struct CommentEdits {
    /// What the platform lets the user do per thread; set by the first frame with a session.
    provider: Option<Arc<dyn Provider>>,
    entries: Vec<Entry>,
}

impl CommentEdits {
    pub(crate) fn set_provider(&mut self, provider: &Arc<dyn Provider>) {
        if self.provider.is_none() {
            self.provider = Some(Arc::clone(provider));
        }
    }

    /// The platform of the pull request, once a frame with its session ran.
    pub(crate) fn provider(&self) -> Option<&dyn Provider> {
        self.provider.as_deref()
    }

    /// What the menu of `comment` offers: what the platform lets the user do in `thread`, for
    /// [`may_change`] comments only.
    pub(crate) fn actions(
        &self,
        thread: &ReviewThread,
        comment: &ThreadComment,
        me: Option<&UserIdentity>,
        can_write: bool,
    ) -> CommentActions {
        match &self.provider {
            Some(provider) if may_change(thread, comment, me, can_write) => {
                provider.comment_actions(thread)
            }
            _ => CommentActions::NONE,
        }
    }

    pub(crate) fn view(&self, thread_id: &str, comment_id: &str) -> View {
        let Some(entry) = self.entry(thread_id, comment_id) else {
            return View::Plain;
        };
        match &entry.change {
            Change::Editing { failure, .. } => View::Editing { failure: failure.clone() },
            Change::Saving { text, .. } => View::Saving { body: text.clone() },
            Change::Saved { comment, .. } => View::Saved(comment.clone()),
            Change::Confirming { failure, .. } => View::Confirming { failure: failure.clone() },
            Change::Deleting { .. } => View::Deleting,
            Change::Deleted { .. } => View::Deleted,
        }
    }

    pub(crate) fn is_deleted(&self, thread_id: &str, comment_id: &str) -> bool {
        self.entry(thread_id, comment_id)
            .is_some_and(|entry| matches!(entry.change, Change::Deleted { .. }))
    }

    /// Every comment of `thread` is deleted: the platform drops the thread, so its card goes too.
    pub(crate) fn hides(&self, thread: &ReviewThread) -> bool {
        !self.entries.is_empty()
            && !thread.comments.is_empty()
            && thread.comments.iter().all(|comment| self.is_deleted(&thread.id, &comment.id))
    }

    /// Whether a composer is open on a comment: live updates hold back what would move it.
    pub(crate) fn is_editing(&self) -> bool {
        self.entries.iter().any(|entry| matches!(entry.change, Change::Editing { .. }))
    }

    /// Opens the composer on the comment's current text.
    pub(crate) fn start_edit(&mut self, thread_id: &str, comment_id: &str, body: &str) {
        let change = Change::Editing { text: body.to_owned(), failure: None, wants_focus: true };
        self.put(thread_id, comment_id, change);
    }

    /// The composer's text while the comment is edited.
    pub(crate) fn text_mut(&mut self, thread_id: &str, comment_id: &str) -> Option<&mut String> {
        match &mut self.entry_mut(thread_id, comment_id)?.change {
            Change::Editing { text, .. } => Some(text),
            _ => None,
        }
    }

    pub(crate) fn ask_delete(&mut self, thread_id: &str, comment_id: &str) {
        self.put(thread_id, comment_id, Change::Confirming { failure: None, wants_focus: true });
    }

    /// True once after the composer or the delete question opened: it takes the keyboard focus.
    pub(crate) fn take_focus(&mut self, thread_id: &str, comment_id: &str) -> bool {
        match self.entry_mut(thread_id, comment_id).map(|entry| &mut entry.change) {
            Some(Change::Editing { wants_focus, .. } | Change::Confirming { wants_focus, .. }) => {
                std::mem::take(wants_focus)
            }
            _ => false,
        }
    }

    /// The user goes on after a failure: its reason leaves, the composer or question stays.
    pub(crate) fn clear_failure(&mut self, thread_id: &str, comment_id: &str) {
        if let Some(Change::Editing { failure, .. } | Change::Confirming { failure, .. }) =
            self.entry_mut(thread_id, comment_id).map(|entry| &mut entry.change)
        {
            *failure = None;
        }
    }

    /// Closes the composer or the delete question; the comment shows as the platform has it.
    pub(crate) fn cancel(&mut self, thread_id: &str, comment_id: &str) {
        self.entries.retain(|entry| {
            let is_open =
                matches!(entry.change, Change::Editing { .. } | Change::Confirming { .. });
            !(is_open && entry.thread_id == thread_id && entry.comment_id == comment_id)
        });
    }

    pub(crate) fn save(
        &mut self,
        thread_id: &str,
        comment_id: &str,
        body: String,
        session: &PullRequestSession,
        services: &Services,
    ) {
        let (provider, repository, number) =
            (session.provider.clone(), session.key.repository_key.clone(), session.key.number);
        let (thread, comment, text) = (thread_id.to_owned(), comment_id.to_owned(), body.clone());
        let mut load = Load::Idle;
        load.start(&services.background, async move {
            provider.edit_comment(&repository, number, &thread, &comment, &text).await
        });
        self.put(thread_id, comment_id, Change::Saving { text: body, load });
    }

    pub(crate) fn delete(
        &mut self,
        thread_id: &str,
        comment_id: &str,
        session: &PullRequestSession,
        services: &Services,
    ) {
        let (provider, repository, number) =
            (session.provider.clone(), session.key.repository_key.clone(), session.key.number);
        let (thread, comment) = (thread_id.to_owned(), comment_id.to_owned());
        let mut load = Load::Idle;
        load.start(&services.background, async move {
            provider.delete_comment(&repository, number, &thread, &comment).await
        });
        self.put(thread_id, comment_id, Change::Deleting { load });
    }

    /// Moves finished saves and deletions on; returns true when one reached the platform (the
    /// threads need a reload). `version` is the threads' version now.
    pub(crate) fn poll(
        &mut self,
        issues: &AccessIssues,
        connection_id: &str,
        version: u64,
        toasts: &mut Toasts,
    ) -> bool {
        let mut is_delivered = false;
        for entry in &mut self.entries {
            let answer = match &mut entry.change {
                Change::Saving { load, .. } => {
                    load.take_finished().map(|result| result.map(Answer::Saved))
                }
                Change::Deleting { load } => {
                    load.take_finished().map(|result| result.map(|()| Answer::Deleted))
                }
                _ => None,
            };
            let Some(answer) = answer else {
                continue;
            };
            let needs_token = issues.get(connection_id).is_some();
            let answer = answer.map_err(|message| Failure { message, needs_token });
            if let Some(done) = finish(&mut entry.change, answer, version) {
                toasts.success(done);
                is_delivered = true;
            }
        }
        is_delivered
    }

    /// Drops the saved and deleted comments the threads loaded at `version` already show as they are.
    pub(crate) fn forget_landed(&mut self, version: u64) {
        self.entries.retain(|entry| match entry.change {
            Change::Saved { after_version, .. } | Change::Deleted { after_version } => {
                version <= after_version
            }
            _ => true,
        });
    }

    fn entry(&self, thread_id: &str, comment_id: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.thread_id == thread_id && entry.comment_id == comment_id)
    }

    fn entry_mut(&mut self, thread_id: &str, comment_id: &str) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .find(|entry| entry.thread_id == thread_id && entry.comment_id == comment_id)
    }

    fn put(&mut self, thread_id: &str, comment_id: &str, change: Change) {
        match self.entry_mut(thread_id, comment_id) {
            Some(entry) => entry.change = change,
            None => self.entries.push(Entry {
                thread_id: thread_id.to_owned(),
                comment_id: comment_id.to_owned(),
                change,
            }),
        }
    }
}

/// What the platform answered for a save or a deletion.
enum Answer {
    Saved(ThreadComment),
    Deleted,
}

/// Moves a save or deletion on with the platform's answer at threads `version`: a save shows the
/// platform's copy, a deletion hides the comment; a failure opens the composer again with the text
/// (or asks again) with the reason. Returns the confirmation when the change reached the platform.
/// Only a change waiting for the platform takes an answer, and only one of its own kind.
fn finish(
    change: &mut Change,
    answer: Result<Answer, Failure>,
    version: u64,
) -> Option<&'static str> {
    let (next, done) = match (&mut *change, answer) {
        (Change::Saving { .. }, Ok(Answer::Saved(comment))) => {
            (Change::Saved { comment, after_version: version }, Some("Comment updated."))
        }
        (Change::Saving { text, .. }, Err(failure)) => {
            let text = std::mem::take(text);
            (Change::Editing { text, failure: Some(failure), wants_focus: false }, None)
        }
        (Change::Deleting { .. }, Ok(Answer::Deleted)) => {
            (Change::Deleted { after_version: version }, Some("Comment deleted."))
        }
        (Change::Deleting { .. }, Err(failure)) => {
            (Change::Confirming { failure: Some(failure), wants_focus: false }, None)
        }
        _ => return None,
    };
    *change = next;
    done
}

/// Only the signed-in user's own comments, outside platform notices, and only with a token: other
/// people's comments never offer Edit or Delete, even where the platform would let a maintainer.
fn may_change(
    thread: &ReviewThread,
    comment: &ThreadComment,
    me: Option<&UserIdentity>,
    can_write: bool,
) -> bool {
    let is_mine = me.is_some_and(|me| !me.id.is_empty() && me.id == comment.author.id);
    can_write && is_mine && !thread.is_system
}

#[cfg(test)]
impl CommentEdits {
    /// A save or deletion on its way, without a platform behind it.
    pub fn send_for_tests(&mut self, thread_id: &str, comment_id: &str, body: Option<&str>) {
        let change = match body {
            Some(text) => Change::Saving { text: text.to_owned(), load: Load::Idle },
            None => Change::Deleting { load: Load::Idle },
        };
        self.put(thread_id, comment_id, change);
    }

    /// The platform answered at threads `version`, as [`CommentEdits::poll`] hands the answer on;
    /// returns the confirmation it shows.
    fn answer_for_tests(
        &mut self,
        thread_id: &str,
        comment_id: &str,
        answer: Result<Answer, Failure>,
        version: u64,
    ) -> Option<&'static str> {
        finish(&mut self.entry_mut(thread_id, comment_id)?.change, answer, version)
    }
}

#[cfg(test)]
mod tests {
    use review_core::ThreadStatus;

    use super::super::samples::{self, comment_at, person};
    use super::*;

    fn comment(id: &str, author: &str, body: &str) -> ThreadComment {
        comment_at(id, &person(author, author), 0, body)
    }

    fn refused(message: &str, needs_token: bool) -> Result<Answer, Failure> {
        Err(Failure { message: message.to_owned(), needs_token })
    }

    #[test]
    fn a_failed_save_reopens_the_composer_with_the_text_and_the_reason() {
        let mut edits = CommentEdits::default();
        edits.start_edit("t1", "c1", "old");
        *edits.text_mut("t1", "c1").expect("open composer") = "new".into();
        assert!(edits.take_focus("t1", "c1"), "the opened composer takes the focus once");
        edits.send_for_tests("t1", "c1", Some("new"));
        assert_eq!(edits.view("t1", "c1"), View::Saving { body: "new".into() });
        assert_eq!(edits.answer_for_tests("t1", "c1", refused("Forbidden", true), 3), None);
        let View::Editing { failure: Some(failure) } = edits.view("t1", "c1") else {
            panic!("the composer is open again");
        };
        assert_eq!(failure, Failure { message: "Forbidden".into(), needs_token: true });
        assert_eq!(edits.text_mut("t1", "c1").map(|text| text.as_str()), Some("new"));
        assert!(!edits.take_focus("t1", "c1"), "the reopened composer leaves the focus alone");
    }

    #[test]
    fn a_saved_edit_shows_the_platform_copy_until_the_reloaded_threads_carry_it() {
        let mut edits = CommentEdits::default();
        edits.send_for_tests("t1", "c1", Some("new"));
        let stored = comment("c1", "me", "new (stored)");
        let done = edits.answer_for_tests("t1", "c1", Ok(Answer::Saved(stored.clone())), 3);
        assert_eq!(done, Some("Comment updated."));
        assert_eq!(edits.view("t1", "c1"), View::Saved(stored));
        edits.forget_landed(3);
        assert!(matches!(edits.view("t1", "c1"), View::Saved(_)), "threads from before the save");
        edits.forget_landed(4);
        assert_eq!(edits.view("t1", "c1"), View::Plain);
    }

    #[test]
    fn deleting_the_last_comment_hides_the_thread() {
        let mut edits = CommentEdits::default();
        let comments = vec![comment("c1", "me", "a"), comment("c2", "me", "b")];
        let two = samples::thread("t1", ThreadStatus::Active, None, comments);
        edits.ask_delete("t1", "c2");
        edits.send_for_tests("t1", "c2", None);
        let done = edits.answer_for_tests("t1", "c2", Ok(Answer::Deleted), 1);
        assert_eq!(done, Some("Comment deleted."));
        assert!(edits.is_deleted("t1", "c2"));
        assert!(!edits.hides(&two), "one comment is left");
        edits.send_for_tests("t1", "c1", None);
        edits.answer_for_tests("t1", "c1", refused("Not found", false), 1);
        assert!(
            matches!(edits.view("t1", "c1"), View::Confirming { failure: Some(_) }),
            "a failed deletion asks again, with the reason"
        );
        edits.send_for_tests("t1", "c1", None);
        edits.answer_for_tests("t1", "c1", Ok(Answer::Deleted), 1);
        assert!(edits.hides(&two));
    }

    #[test]
    fn cancel_closes_only_an_open_composer_or_question() {
        let mut edits = CommentEdits::default();
        edits.start_edit("t1", "c1", "old");
        edits.cancel("t1", "c1");
        assert_eq!(edits.view("t1", "c1"), View::Plain);
        edits.send_for_tests("t1", "c2", None);
        edits.cancel("t1", "c2");
        assert_eq!(edits.view("t1", "c2"), View::Deleting, "a deletion on its way goes on");
    }

    #[test]
    fn only_own_comments_outside_notices_may_change() {
        let (me, mine, theirs) =
            (person("me", "me"), comment("c1", "me", "a"), comment("c2", "cy", "b"));
        let comments = vec![mine.clone(), theirs.clone()];
        let user_thread = samples::thread("t1", ThreadStatus::Active, None, comments);
        assert!(may_change(&user_thread, &mine, Some(&me), true));
        assert!(!may_change(&user_thread, &theirs, Some(&me), true), "someone else's comment");
        assert!(!may_change(&user_thread, &mine, None, true), "before the platform said who I am");
        assert!(!may_change(&user_thread, &mine, Some(&me), false), "without a token");
        let notice = ReviewThread { is_system: true, ..user_thread };
        assert!(!may_change(&notice, &mine, Some(&me), true), "a platform notice");
    }
}
