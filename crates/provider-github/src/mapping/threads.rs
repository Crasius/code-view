//! Review threads (GraphQL or REST), issue comments and review bodies → [`ReviewThread`].

use std::collections::HashMap;

use review_core::{
    DiffSide, LineRange, ReviewThread, ThreadAnchor, ThreadComment, ThreadOrigin, ThreadStatus,
};

use crate::dto::comment::{IssueCommentDto, ReviewCommentDto};
use crate::dto::review::ReviewDto;
use crate::dto::review_thread::{ReviewThreadDto, ThreadCommentDto};
use crate::graphql::PageInfo;
use crate::mapping::identity;
use crate::mapping::thread_id::ThreadId;

/// GraphQL thread with its first comment page; the page info tells whether more comments follow.
/// Outdated threads anchor on `originalLine`, current ones on `line`.
pub(crate) fn review_thread(dto: ReviewThreadDto) -> (ReviewThread, PageInfo) {
    let (line, start_line) = if dto.is_outdated {
        (dto.original_line.or(dto.line), dto.original_start_line.or(dto.start_line))
    } else {
        (dto.line, dto.start_line)
    };
    let end = SideLine::new(dto.diff_side.as_deref(), line);
    let start = SideLine::new(dto.start_diff_side.as_deref(), start_line);
    let anchor = anchor(dto.path, end, start);
    // Outdated threads anchor on their original lines already, which are the origin's lines.
    let origin = dto
        .origin
        .filter(|_| dto.is_outdated)
        .and_then(|origin| origin.into_nodes().1.into_iter().next())
        .and_then(|first| {
            Some(origin_of(first.original_commit?.oid, &anchor, first.diff_hunk, first.url))
        });
    let (page_info, comments) = dto.comments.into_nodes();
    let thread = ReviewThread {
        id: ThreadId::ReviewThread(&dto.id).encode(),
        status: if dto.is_resolved { ThreadStatus::Resolved } else { ThreadStatus::Active },
        anchor: Some(anchor),
        is_outdated: dto.is_outdated,
        origin,
        is_system: false,
        can_reply: true,
        can_resolve: true,
        comments: comments.into_iter().map(graphql_comment).collect(),
    };
    (thread, page_info)
}

/// GitHub keeps the head commit a comment was written on (`original_commit_id`), not the base:
/// the merge base with the target branch stands in for it.
fn origin_of(
    head_sha: String,
    anchor: &ThreadAnchor,
    diff_hunk: Option<String>,
    web_url: Option<String>,
) -> ThreadOrigin {
    ThreadOrigin {
        base_sha: None,
        head_sha,
        anchor: anchor.clone(),
        diff_hunk: diff_hunk.filter(|hunk| !hunk.is_empty()),
        web_url,
    }
}

pub(crate) fn graphql_comment(dto: ThreadCommentDto) -> ThreadComment {
    ThreadComment {
        id: dto.database_id.map_or(dto.id, |id| id.to_string()),
        author: identity::actor(dto.author),
        body: dto.body,
        created_at: dto.created_at,
        updated_at: dto.last_edited_at,
    }
}

/// A line as GitHub reports it: a side (`LEFT` base, `RIGHT` head, `RIGHT` when missing) and a number.
#[derive(Clone, Copy)]
pub(crate) struct SideLine {
    side: DiffSide,
    line: Option<u32>,
}

impl SideLine {
    pub(crate) fn new(side: Option<&str>, line: Option<u32>) -> Self {
        let is_left = side.is_some_and(|side| side.eq_ignore_ascii_case("LEFT"));
        let side = if is_left { DiffSide::Old } else { DiffSide::New };
        Self { side, line }
    }

    /// `(old_line, new_line)`: GitHub counts a line on one side only, so at most one is set.
    fn numbers(self) -> (Option<u32>, Option<u32>) {
        match self.side {
            DiffSide::Old => (self.line, None),
            DiffSide::New => (None, self.line),
        }
    }
}

/// `LEFT` lines exist only in the base (`old_line`), `RIGHT` lines in the head (`new_line`).
/// GitHub anchors context lines on one side only, so exactly one number is set. A `start` that
/// differs from `end` makes a multi-line anchor whose last line is `end`.
pub(crate) fn anchor(path: String, end: SideLine, start: SideLine) -> ThreadAnchor {
    let (old_line, new_line) = end.numbers();
    let is_range = start.line.is_some() && (start.line, start.side) != (end.line, end.side);
    let range = is_range.then(|| {
        let (start_old_line, start_new_line) = start.numbers();
        LineRange { start_old_line, start_new_line, side: end.side, detail: None }
    });
    ThreadAnchor { path, old_path: None, old_line, new_line, range }
}

/// Anonymous fallback: REST review comments grouped by `in_reply_to_id`. REST does not expose
/// resolution, so these threads are `Active`, and without a token nothing can be written.
pub(crate) fn rest_threads(comments: Vec<ReviewCommentDto>) -> Vec<ReviewThread> {
    let mut threads: Vec<ReviewThread> = Vec::new();
    let mut index_by_root: HashMap<u64, usize> = HashMap::new();
    let (roots, replies): (Vec<_>, Vec<_>) =
        comments.into_iter().partition(|comment| comment.in_reply_to_id.is_none());
    for root in roots {
        index_by_root.insert(root.id, threads.len());
        threads.push(rest_thread(root));
    }
    for reply in replies {
        let root_index = reply.in_reply_to_id.and_then(|root| index_by_root.get(&root).copied());
        match root_index.and_then(|index| threads.get_mut(index)) {
            Some(thread) => thread.comments.push(rest_comment(reply)),
            None => threads.push(rest_thread(reply)),
        }
    }
    for thread in &mut threads {
        thread.comments.sort_by_key(|comment| comment.created_at);
    }
    threads
}

fn rest_thread(root: ReviewCommentDto) -> ReviewThread {
    let is_file_level = root.subject_type.as_deref() == Some("file");
    let is_outdated = !is_file_level && root.line.is_none();
    let (line, start_line) = if is_outdated {
        (root.original_line, root.original_start_line)
    } else {
        (root.line, root.start_line)
    };
    let end = SideLine::new(root.side.as_deref(), line);
    let start = SideLine::new(root.start_side.as_deref(), start_line);
    let anchor = anchor(root.path.clone(), end, start);
    let origin = root
        .original_commit_id
        .clone()
        .filter(|_| is_outdated)
        .map(|head| origin_of(head, &anchor, root.diff_hunk.clone(), root.html_url.clone()));
    ReviewThread {
        id: ThreadId::ReviewComment(root.id).encode(),
        status: ThreadStatus::Active,
        anchor: Some(anchor),
        is_outdated,
        origin,
        is_system: false,
        can_reply: false,
        can_resolve: false,
        comments: vec![rest_comment(root)],
    }
}

pub(crate) fn rest_comment(dto: ReviewCommentDto) -> ThreadComment {
    ThreadComment {
        id: dto.id.to_string(),
        author: identity::optional_user(dto.user),
        body: dto.body,
        created_at: dto.created_at,
        updated_at: dto.updated_at.filter(|updated| *updated != dto.created_at),
    }
}

/// A general comment as a single-comment thread that cannot be replied to or resolved.
pub(crate) fn issue_comment_thread(dto: IssueCommentDto) -> ReviewThread {
    let id = ThreadId::IssueComment(dto.id).encode();
    single_comment_thread(id, issue_comment(dto))
}

pub(crate) fn issue_comment(dto: IssueCommentDto) -> ThreadComment {
    ThreadComment {
        id: dto.id.to_string(),
        author: identity::optional_user(dto.user),
        body: dto.body.unwrap_or_default(),
        created_at: dto.created_at,
        updated_at: dto.updated_at.filter(|updated| *updated != dto.created_at),
    }
}

/// A submitted review with a non-empty body; pending and empty reviews yield `None`.
pub(crate) fn review_body_thread(dto: ReviewDto) -> Option<ReviewThread> {
    let body = dto.body.filter(|body| !body.trim().is_empty())?;
    let created_at = dto.submitted_at?;
    let comment = ThreadComment {
        id: dto.id.to_string(),
        author: identity::optional_user(dto.user),
        body,
        created_at,
        updated_at: None,
    };
    Some(single_comment_thread(ThreadId::ReviewBody(dto.id).encode(), comment))
}

fn single_comment_thread(id: String, comment: ThreadComment) -> ReviewThread {
    ReviewThread {
        id,
        status: ThreadStatus::Active,
        anchor: None,
        is_outdated: false,
        origin: None,
        is_system: false,
        can_reply: false,
        can_resolve: false,
        comments: vec![comment],
    }
}

/// Oldest first by first comment; threads without comments sort last. Stable for equal times.
pub(crate) fn sort_oldest_first(threads: &mut [ReviewThread]) {
    threads.sort_by_key(|thread| {
        let first = thread.comments.first().map(|comment| comment.created_at);
        (first.is_none(), first)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::review_thread::{PullRequestData, RepositoryData, ReviewThreadsField};
    use crate::mapping::fixtures::parse;

    fn graphql_threads() -> Vec<ReviewThreadDto> {
        let data: RepositoryData<ReviewThreadsField> =
            parse(include_str!("../../tests/fixtures/review_threads.json"));
        let pull: PullRequestData<ReviewThreadsField> = data.repository.expect("repository");
        pull.pull_request.expect("pull request").review_threads.into_nodes().1
    }

    #[test]
    fn graphql_threads_anchor_by_side() {
        let mut threads = graphql_threads().into_iter().map(|dto| review_thread(dto).0);
        let right = threads.next().expect("right thread");
        assert_eq!(right.id, "PRRT_right");
        assert_eq!(right.status, ThreadStatus::Resolved);
        let anchor = right.anchor.as_ref().expect("anchor");
        assert_eq!(
            (anchor.path.as_str(), anchor.old_line, anchor.new_line),
            ("src/main.rs", None, Some(12))
        );
        assert_eq!(right.comments[0].id, "1001");
        assert!(right.can_reply && right.can_resolve);

        let left = threads.next().expect("outdated left thread");
        assert!(left.is_outdated);
        let anchor = left.anchor.as_ref().expect("anchor");
        assert_eq!((anchor.old_line, anchor.new_line), (Some(4), None));
        assert_eq!(left.comments[0].author.login.as_deref(), Some("ghost"));
    }

    #[test]
    fn multi_line_threads_keep_their_first_line() {
        let dto: ReviewThreadDto = parse(
            r#"{"id":"PRRT_range","isResolved":false,"isOutdated":false,"path":"src/a.rs",
                "line":14,"originalLine":14,"diffSide":"RIGHT","startLine":10,"originalStartLine":10,
                "startDiffSide":"LEFT","comments":{"nodes":[]}}"#,
        );
        let anchor = review_thread(dto).0.anchor.expect("anchor");
        assert_eq!((anchor.old_line, anchor.new_line), (None, Some(14)));
        let range = anchor.range.expect("range");
        assert_eq!(
            (range.start_old_line, range.start_new_line, range.side),
            (Some(10), None, DiffSide::New)
        );

        let outdated: ReviewThreadDto = parse(
            r#"{"id":"PRRT_old","isResolved":false,"isOutdated":true,"path":"src/a.rs",
                "line":null,"originalLine":8,"diffSide":"LEFT","startLine":null,"originalStartLine":6,
                "startDiffSide":"LEFT","comments":{"nodes":[]}}"#,
        );
        let anchor = review_thread(outdated).0.anchor.expect("anchor");
        let range = anchor.range.expect("range");
        assert_eq!(
            (anchor.old_line, range.start_old_line, range.side),
            (Some(8), Some(6), DiffSide::Old)
        );

        let rest: Vec<ReviewCommentDto> = parse(
            r#"[{"id":40,"path":"src/a.rs","line":5,"side":"RIGHT","start_line":3,"start_side":"RIGHT",
                 "subject_type":"line","body":"x","created_at":"2024-06-01T10:00:00Z"},
                {"id":41,"path":"src/a.rs","line":5,"side":"RIGHT","start_line":null,"start_side":null,
                 "subject_type":"line","body":"y","created_at":"2024-06-01T10:00:00Z"}]"#,
        );
        let threads = rest_threads(rest);
        let range =
            threads[0].anchor.as_ref().and_then(|anchor| anchor.range.clone()).expect("range");
        assert_eq!((range.start_new_line, range.side), (Some(3), DiffSide::New));
        assert!(threads[1].anchor.as_ref().is_some_and(|anchor| anchor.range.is_none()));
    }

    /// A real anonymous `GET /repos/{owner}/{repo}/pulls/{number}/comments` row (trimmed,
    /// renamed): outdated, so `line` is null and only the original line and commit say where it
    /// was written.
    #[test]
    fn rest_outdated_comments_keep_their_origin() {
        let comments: Vec<ReviewCommentDto> =
            parse(include_str!("../../tests/fixtures/review_comment_outdated.json"));
        let thread = rest_threads(comments).remove(0);
        assert!(thread.is_outdated);
        let origin = thread.origin.expect("origin");
        assert_eq!(origin.head_sha, "efe3c7630da3296a9435293c1a7b0e5358a022e0");
        assert_eq!(origin.base_sha, None, "GitHub does not keep the base");
        assert_eq!(
            (origin.anchor.path.as_str(), origin.anchor.old_line, origin.anchor.new_line),
            (".github/workflows/issue-triage.md", None, Some(121))
        );
        assert!(
            origin.diff_hunk.as_deref().is_some_and(|hunk| hunk.starts_with("@@ -116,7 +118,7 @@"))
        );
        assert_eq!(
            origin.web_url.as_deref(),
            Some("https://github.com/contoso/app/pull/14682#discussion_r4270374354")
        );

        let current: Vec<ReviewCommentDto> =
            parse(include_str!("../../tests/fixtures/review_comments.json"));
        assert!(
            rest_threads(current)[0].origin.is_none(),
            "a current comment is where it was written"
        );
    }

    #[test]
    fn graphql_outdated_threads_keep_their_origin() {
        let dto: ReviewThreadDto = parse(
            r#"{"id":"PRRT_old","isResolved":false,"isOutdated":true,"path":"src/a.rs",
                "line":null,"originalLine":8,"diffSide":"RIGHT","startLine":null,"originalStartLine":null,
                "startDiffSide":null,"comments":{"nodes":[]},
                "origin":{"nodes":[{"originalCommit":{"oid":"abc123"},"diffHunk":"@@ -5,3 +5,4 @@\n a\n+b",
                  "url":"https://github.com/o/r/pull/1#discussion_r9"}]}}"#,
        );
        let origin = review_thread(dto).0.origin.expect("origin");
        assert_eq!((origin.head_sha.as_str(), origin.anchor.new_line), ("abc123", Some(8)));
        assert_eq!(origin.web_url.as_deref(), Some("https://github.com/o/r/pull/1#discussion_r9"));

        let current: ReviewThreadDto = parse(
            r#"{"id":"PRRT_new","isResolved":false,"isOutdated":false,"path":"src/a.rs","line":3,
                "diffSide":"RIGHT","comments":{"nodes":[]},
                "origin":{"nodes":[{"originalCommit":{"oid":"abc123"},"diffHunk":"","url":null}]}}"#,
        );
        assert!(review_thread(current).0.origin.is_none());
    }

    #[test]
    fn graphql_page_info_signals_more_comments() {
        let (_, page) = review_thread(graphql_threads().remove(1));
        assert!(page.has_next_page);
        assert_eq!(page.end_cursor.as_deref(), Some("c2"));
    }

    #[test]
    fn rest_comments_group_by_reply_target() {
        let comments: Vec<ReviewCommentDto> =
            parse(include_str!("../../tests/fixtures/review_comments.json"));
        let threads = rest_threads(comments);
        assert_eq!(threads.len(), 3);
        assert_eq!(threads[0].id, "review-comment:1");
        assert_eq!(
            threads[0].comments.iter().map(|comment| comment.id.as_str()).collect::<Vec<_>>(),
            vec!["1", "3"]
        );
        assert!(!threads[0].can_reply);
        assert!(threads[1].is_outdated);
        assert_eq!(threads[1].anchor.as_ref().and_then(|anchor| anchor.old_line), Some(8));
        let file_level = threads[2].anchor.as_ref().expect("anchor");
        assert!(!threads[2].is_outdated);
        assert_eq!((file_level.old_line, file_level.new_line), (None, None));
    }

    #[test]
    fn issue_comments_and_review_bodies_are_single_threads() {
        let issue = issue_comment_thread(parse(
            r#"{"id":5,"user":{"id":1,"login":"mona"},"body":"LGTM","created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}"#,
        ));
        assert_eq!(issue.id, "issue-comment:5");
        assert!(issue.anchor.is_none() && !issue.can_reply && !issue.can_resolve);
        assert_eq!(issue.comments[0].updated_at, None);

        let review: Option<ReviewThread> = review_body_thread(parse(
            r#"{"id":8,"user":{"id":1,"login":"mona"},"body":"Looks good","state":"APPROVED","submitted_at":"2024-01-02T00:00:00Z"}"#,
        ));
        assert_eq!(review.map(|thread| thread.id), Some("review:8".to_owned()));
        let empty = review_body_thread(parse(
            r#"{"id":9,"body":"","state":"APPROVED","submitted_at":"2024-01-02T00:00:00Z"}"#,
        ));
        assert!(empty.is_none());
        let pending = review_body_thread(parse(
            r#"{"id":10,"body":"draft","state":"PENDING","submitted_at":null}"#,
        ));
        assert!(pending.is_none());
    }

    #[test]
    fn sorts_oldest_first() {
        let comments: Vec<ReviewCommentDto> =
            parse(include_str!("../../tests/fixtures/review_comments.json"));
        let mut threads = rest_threads(comments);
        threads.reverse();
        threads.push(ReviewThread { comments: Vec::new(), ..threads[0].clone() });
        sort_oldest_first(&mut threads);
        let ids: Vec<&str> = threads.iter().map(|thread| thread.id.as_str()).collect();
        assert_eq!(ids[..3], ["review-comment:1", "review-comment:2", "review-comment:4"]);
        assert!(threads[3].comments.is_empty(), "{:?}", threads[3].comments);
    }
}
