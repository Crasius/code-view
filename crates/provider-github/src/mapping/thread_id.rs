//! [`review_core::ReviewThread::id`] encoding. GraphQL review threads keep their bare node id
//! (what `addPullRequestReviewThreadReply` and `resolveReviewThread` take); everything else is prefixed.

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
    pub fn parse(id: &'a str) -> Option<Self> {
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

    pub fn encode(self) -> String {
        match self {
            ThreadId::ReviewThread(node_id) => node_id.to_owned(),
            ThreadId::ReviewComment(id) => format!("{REVIEW_COMMENT_PREFIX}{id}"),
            ThreadId::IssueComment(id) => format!("{ISSUE_COMMENT_PREFIX}{id}"),
            ThreadId::ReviewBody(id) => format!("{REVIEW_BODY_PREFIX}{id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ThreadId;

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
