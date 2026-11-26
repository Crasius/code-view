//! What a thread card asks for, and where it asks the Files tab to go. The page that shows the
//! card hands the event to [`super::ThreadsUi::run`].

use review_core::ThreadStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadEvent {
    /// Post `body` as a reply; with `resolve` the thread is resolved right after.
    Reply {
        thread_id: String,
        body: String,
        resolve: bool,
    },
    SetStatus {
        thread_id: String,
        status: ThreadStatus,
    },
    /// Show the thread in its file.
    Reveal {
        thread_id: String,
    },
    /// Show the version of the file the (outdated) thread was written on.
    ShowOrigin {
        thread_id: String,
    },
    /// Replace the body of the user's own comment.
    EditComment {
        thread_id: String,
        comment_id: String,
        body: String,
    },
    /// Delete the user's own comment (its deletion was confirmed).
    DeleteComment {
        thread_id: String,
        comment_id: String,
    },
}

/// Where a card asked the Files tab to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Jump {
    /// The thread in its file.
    Reveal { path: String, thread_id: String },
    /// The version of the file the thread was written on.
    Origin { thread_id: String },
}
