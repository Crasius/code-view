//! [`review_core::ReviewThread::id`] encoding. GraphQL review threads keep their bare node id
//! (what `addPullRequestReviewThreadReply` and `resolveReviewThread` take); everything else is prefixed.

use review_core::ReviewThread;

const REVIEW_COMMENT_PREFIX: &str = "review-comment:";
const ISSUE_COMMENT_PREFIX: &str = "issue-comment:";
const REVIEW_BODY_PREFIX: &str = "review:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadId<'a> {
    /// GraphQL `PullRequestReviewThread` node id.
    ReviewThread(&'a str),
    /// REST root review comment id; used when threads were loaded without a token.
    ReviewComment(u64),
    /// General pull request comment.
    IssueComment(u64),
    /// Body of a submitted review.
    ReviewBody(u64),
}

impl<'a> ThreadId<'a> {
    pub(crate) fn parse(id: &'a str) -> Option<Self> {
        let numeric =
            |prefix: &str| id.strip_prefix(prefix).and_then(|rest| rest.parse::<u64>().ok());
        if let Some(comment_id) = numeric(REVIEW_COMMENT_PREFIX) {
            return Some(ThreadId::ReviewComment(comment_id));
        }
        if let Some(comment_id) = numeric(ISSUE_COMMENT_PREFIX) {
            return Some(ThreadId::IssueComment(comment_id));
        }
        if let Some(review_id) = numeric(REVIEW_BODY_PREFIX) {
            return Some(ThreadId::ReviewBody(review_id));
        }
        let is_node_id = !id.is_empty() && !id.contains(':');
        is_node_id.then_some(ThreadId::ReviewThread(id))
    }

    pub(crate) fn encode(self) -> String {
        match self {
            ThreadId::ReviewThread(node_id) => node_id.to_owned(),
            ThreadId::ReviewComment(id) => format!("{REVIEW_COMMENT_PREFIX}{id}"),
            ThreadId::IssueComment(id) => format!("{ISSUE_COMMENT_PREFIX}{id}"),
            ThreadId::ReviewBody(id) => format!("{REVIEW_BODY_PREFIX}{id}"),
        }
    }
}

/// The comment anchor of `thread` on the pull request's page: review threads and their REST root
/// comments link to the first comment, general comments to theirs, review bodies to the review.
pub(crate) fn page_suffix(thread: &ReviewThread) -> Option<String> {
    let first = thread.comments.first()?;
    let suffix = match ThreadId::parse(&thread.id)? {
        ThreadId::ReviewThread(_) | ThreadId::ReviewComment(_) => {
            format!("#discussion_r{}", first.id)
        }
        ThreadId::IssueComment(_) => format!("#issuecomment-{}", first.id),
        ThreadId::ReviewBody(review) => format!("#pullrequestreview-{review}"),
    };
    Some(suffix)
}

#[cfg(test)]
mod tests {
    use review_core::{ReviewThread, ThreadComment, ThreadStatus, UserIdentity};

    use super::{ThreadId, page_suffix};

    fn thread(id: &str, comments: &[&str]) -> ReviewThread {
        let comment = |comment_id: &&str| ThreadComment {
            id: (*comment_id).to_owned(),
            author: UserIdentity::default(),
            body: String::new(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: None,
        };
        ReviewThread {
            id: id.to_owned(),
            status: ThreadStatus::Active,
            anchor: None,
            is_outdated: false,
            origin: None,
            is_system: false,
            can_reply: true,
            can_resolve: true,
            comments: comments.iter().map(comment).collect(),
        }
    }

    #[test]
    fn links_each_kind_of_thread() {
        let cases = [
            ("PRRT_kw", &["991"][..], Some("#discussion_r991")),
            ("review-comment:991", &["991"][..], Some("#discussion_r991")),
            ("issue-comment:5", &["5"][..], Some("#issuecomment-5")),
            ("review:7", &["8"][..], Some("#pullrequestreview-7")),
            ("PRRT_kw", &[][..], None),
            ("other:1", &["1"][..], None),
        ];
        for (id, comments, expected) in cases {
            assert_eq!(page_suffix(&thread(id, comments)).as_deref(), expected, "{id}");
        }
    }

    #[test]
    fn round_trips() {
        for id in [
            ThreadId::ReviewThread("PRRT_kwDOA"),
            ThreadId::ReviewComment(5),
            ThreadId::IssueComment(6),
            ThreadId::ReviewBody(7),
        ] {
            assert_eq!(ThreadId::parse(&id.encode()), Some(id));
        }
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(ThreadId::parse(""), None);
        assert_eq!(ThreadId::parse("issue-comment:x"), None);
        assert_eq!(ThreadId::parse("other:1"), None);
    }
}
