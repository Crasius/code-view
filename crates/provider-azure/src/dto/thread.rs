use std::collections::HashMap;

use review_core::{
    DiffSide, FilePositions, LineRange, NewThread, ReviewThread, ThreadAnchor, ThreadComment,
    ThreadOrigin, ThreadStatus, Timestamp,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::PropertyValue;
use super::identity::IdentityRef;
use super::iteration::IterationCommits;

#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct Position {
    pub line: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadContext {
    pub file_path: Option<String>,
    pub left_file_start: Option<Position>,
    pub left_file_end: Option<Position>,
    pub right_file_start: Option<Position>,
    pub right_file_end: Option<Position>,
}

/// First and last line of a thread's span in one file; a missing end means one line.
fn span(start: Option<&Position>, end: Option<&Position>) -> Option<(u32, u32)> {
    let start = start?.line;
    Some((start, end.map_or(start, |end| end.line.max(start))))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRequestThreadContext {
    /// The iterations the author's diff compared when the thread was created.
    pub iteration_context: Option<IterationContext>,
    /// Filled once Azure DevOps tracked the thread away from where it was created.
    pub tracking_criteria: Option<TrackingCriteria>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IterationContext {
    pub first_comparing_iteration: u32,
    pub second_comparing_iteration: u32,
}

/// Where a tracked thread was created: its file and span before tracking moved them.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(clippy::struct_field_names, reason = "fields mirror the `orig*` JSON keys")]
pub(crate) struct TrackingCriteria {
    pub orig_file_path: Option<String>,
    pub orig_left_file_start: Option<Position>,
    pub orig_left_file_end: Option<Position>,
    pub orig_right_file_start: Option<Position>,
    pub orig_right_file_end: Option<Position>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(clippy::struct_field_names, reason = "fields mirror the Comment JSON keys")]
pub(crate) struct Comment {
    pub id: u32,
    #[serde(default)]
    pub author: IdentityRef,
    pub content: Option<String>,
    #[serde(deserialize_with = "super::date::required")]
    pub published_date: Timestamp,
    #[serde(default, deserialize_with = "super::date::optional")]
    pub last_content_updated_date: Option<Timestamp>,
    pub comment_type: Option<String>,
    #[serde(default)]
    pub is_deleted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_field_names,
    reason = "fields mirror the GitPullRequestCommentThread JSON keys"
)]
pub(crate) struct Thread {
    pub id: u64,
    pub status: Option<String>,
    #[serde(deserialize_with = "super::date::required")]
    pub published_date: Timestamp,
    /// Moves with every reply, edit and status change; the live-update probe watches it.
    #[serde(default, deserialize_with = "super::date::optional")]
    pub last_updated_date: Option<Timestamp>,
    pub thread_context: Option<ThreadContext>,
    pub pull_request_thread_context: Option<PullRequestThreadContext>,
    pub properties: Option<HashMap<String, PropertyValue>>,
    #[serde(default)]
    pub comments: Vec<Comment>,
    #[serde(default)]
    pub is_deleted: bool,
}

impl Thread {
    /// Platform notices (votes, reviewer and ref updates, status changes) carry a
    /// `CodeReviewThreadType` property or consist of `system` comments.
    pub fn is_system(&self) -> bool {
        let has_thread_type = self
            .properties
            .as_ref()
            .is_some_and(|properties| properties.contains_key("CodeReviewThreadType"));
        has_thread_type
            || self
                .comments
                .first()
                .is_some_and(|comment| comment.comment_type.as_deref() == Some("system"))
    }

    pub fn first_comment_id(&self) -> Option<u32> {
        self.comments.iter().filter(|comment| !comment.is_deleted).map(|comment| comment.id).min()
    }

    /// Azure DevOps tracked the anchor onto a newer iteration because the code around it changed;
    /// that is the closest signal to "outdated".
    pub fn is_tracked(&self) -> bool {
        self.pull_request_thread_context
            .as_ref()
            .is_some_and(|context| context.tracking_criteria.is_some())
    }

    /// The diff a tracked thread was created on and its span there; `None` for threads that were
    /// not tracked or whose iterations are not in `iterations`.
    fn origin(&self, iterations: &IterationCommits) -> Option<ThreadOrigin> {
        let context = self.pull_request_thread_context.as_ref()?;
        let (criteria, iteration) =
            (context.tracking_criteria.as_ref()?, context.iteration_context.as_ref()?);
        let (base_sha, head_sha) = iterations
            .compared(iteration.first_comparing_iteration, iteration.second_comparing_iteration)?;
        let current = self.thread_context.as_ref();
        let original = ThreadContext {
            file_path: criteria
                .orig_file_path
                .clone()
                .or_else(|| current.and_then(|context| context.file_path.clone())),
            left_file_start: criteria.orig_left_file_start,
            left_file_end: criteria.orig_left_file_end,
            right_file_start: criteria.orig_right_file_start,
            right_file_end: criteria.orig_right_file_end,
        };
        Some(ThreadOrigin {
            base_sha: Some(base_sha),
            head_sha,
            anchor: anchor_from_context(&original)?,
            diff_hunk: None,
            web_url: None,
        })
    }

    /// `None` for deleted threads and threads whose comments are all deleted. `iterations` gives
    /// tracked threads the commits they were created on.
    pub fn to_thread(&self, iterations: &IterationCommits) -> Option<ReviewThread> {
        if self.is_deleted {
            return None;
        }
        let mut comments: Vec<ThreadComment> = self
            .comments
            .iter()
            .filter(|comment| !comment.is_deleted)
            .map(Comment::to_comment)
            .collect();
        if comments.is_empty() {
            return None;
        }
        comments.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
        let is_system = self.is_system();
        Some(ReviewThread {
            id: self.id.to_string(),
            status: status_from_ado(self.status.as_deref()),
            anchor: self.thread_context.as_ref().and_then(anchor_from_context),
            is_outdated: self.is_tracked(),
            origin: self.origin(iterations),
            is_system,
            can_reply: !is_system,
            can_resolve: !is_system,
            comments,
        })
    }
}

impl Comment {
    pub fn to_comment(&self) -> ThreadComment {
        ThreadComment {
            id: self.id.to_string(),
            author: self.author.to_user(),
            body: self.content.clone().unwrap_or_default(),
            created_at: self.published_date,
            updated_at: self
                .last_content_updated_date
                .filter(|updated| *updated != self.published_date),
        }
    }
}

/// Right side → `new_line`, left side → `old_line`. A comment on an unchanged context line only
/// carries the right side in Azure DevOps, so `old_line` stays `None` there (deriving it would need the diff).
/// A span over several lines anchors on its last line and keeps its first as the range start.
pub(crate) fn anchor_from_context(context: &ThreadContext) -> Option<ThreadAnchor> {
    let path = context.file_path.as_deref()?.trim_start_matches('/');
    if path.is_empty() {
        return None;
    }
    let right = span(context.right_file_start.as_ref(), context.right_file_end.as_ref());
    let left = span(context.left_file_start.as_ref(), context.left_file_end.as_ref());
    let range = match (right, left) {
        (Some((start, end)), _) if start != end => Some(LineRange {
            start_old_line: None,
            start_new_line: Some(start),
            side: DiffSide::New,
            detail: None,
        }),
        (None, Some((start, end))) if start != end => Some(LineRange {
            start_old_line: Some(start),
            start_new_line: None,
            side: DiffSide::Old,
            detail: None,
        }),
        _ => None,
    };
    Some(ThreadAnchor {
        path: path.to_owned(),
        old_path: None,
        old_line: left.map(|(_, end)| end),
        new_line: right.map(|(_, end)| end),
        range,
    })
}

/// A `CommentPosition`: 1-based line and character offset.
type LineOffset = (u32, u32);

/// The file a new thread sits in, with its first and last position.
struct ThreadSpan {
    side: DiffSide,
    start: LineOffset,
    end: LineOffset,
}

/// Where a new thread sits. A single line spans offset 1 to 1; a range uses the character offsets
/// the diff picked.
fn thread_span(anchor: &ThreadAnchor) -> Option<ThreadSpan> {
    let Some(range) = &anchor.range else {
        let line = |side, line| ThreadSpan { side, start: (line, 1), end: (line, 1) };
        return match (anchor.new_line, anchor.old_line) {
            (Some(new), _) => Some(line(DiffSide::New, new)),
            (None, Some(old)) => Some(line(DiffSide::Old, old)),
            (None, None) => None,
        };
    };
    let side = range.side;
    // A line missing from this file counts where it sits in it (only the diff knows that).
    let in_side = |positions: Option<FilePositions>| {
        positions.map(|positions| match side {
            DiffSide::Old => positions.old,
            DiffSide::New => positions.new,
        })
    };
    let end = anchor.line_in(side).or_else(|| in_side(range.detail.map(|detail| detail.end)))?;
    let start = range
        .start_line_in(side)
        .or_else(|| in_side(range.detail.map(|detail| detail.start)))
        .unwrap_or(end);
    let (start_offset, end_offset) =
        range.detail.map_or((1, 1), |detail| (detail.start_offset, detail.end_offset));
    Some(ThreadSpan { side, start: (start.min(end), start_offset), end: (end, end_offset) })
}

pub(crate) fn status_from_ado(status: Option<&str>) -> ThreadStatus {
    match status {
        Some("fixed" | "wontFix" | "closed" | "byDesign") => ThreadStatus::Resolved,
        _ => ThreadStatus::Active,
    }
}

pub(crate) fn status_to_ado(status: ThreadStatus) -> &'static str {
    match status {
        ThreadStatus::Active => "active",
        ThreadStatus::Resolved => "fixed",
    }
}

/// Which iteration and file change an inline comment attaches to in the Azure DevOps web UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IterationAnchor {
    pub latest_iteration: u32,
    pub change_tracking_id: Option<u32>,
}

pub(crate) fn comment_body(parent_comment_id: u32, content: &str) -> Value {
    json!({ "parentCommentId": parent_comment_id, "content": content, "commentType": 1 })
}

/// Request body of `POST .../threads`. Lines without a side default to the right (new) file.
#[expect(
    clippy::indexing_slicing,
    reason = "indexes only the JSON objects this function builds with json!, never provider data"
)]
pub(crate) fn new_thread_body(thread: &NewThread, iteration: Option<IterationAnchor>) -> Value {
    let mut body = json!({ "comments": [comment_body(0, &thread.body)], "status": "active" });
    let Some(anchor) = &thread.anchor else {
        return body;
    };
    let position = |(line, offset): LineOffset| json!({ "line": line, "offset": offset });
    let mut context = json!({ "filePath": format!("/{}", anchor.path.trim_start_matches('/')) });
    match thread_span(anchor) {
        Some(ThreadSpan { side: DiffSide::New, start, end }) => {
            context["rightFileStart"] = position(start);
            context["rightFileEnd"] = position(end);
        }
        Some(ThreadSpan { side: DiffSide::Old, start, end }) => {
            context["leftFileStart"] = position(start);
            context["leftFileEnd"] = position(end);
        }
        None => {}
    }
    body["threadContext"] = context;
    if let Some(iteration) = iteration {
        // The app diffs the merge base against the head: the latest iteration against itself,
        // which Azure DevOps reads as its common commit on the left. `1` would put the left lines
        // in iteration 1's source file.
        let mut pr_context = json!({
            "iterationContext": {
                "firstComparingIteration": iteration.latest_iteration,
                "secondComparingIteration": iteration.latest_iteration,
            }
        });
        if let Some(change_tracking_id) = iteration.change_tracking_id {
            pr_context["changeTrackingId"] = json!(change_tracking_id);
        }
        body["pullRequestThreadContext"] = pr_context;
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use review_core::RangeDetail;

    fn thread(json: &str) -> Thread {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn maps_inline_thread() {
        let thread = thread(
            r#"{"id":148,"status":"fixed","publishedDate":"2026-01-01T10:00:00Z",
            "threadContext":{"filePath":"/src/main.rs","rightFileStart":{"line":5,"offset":1},"rightFileEnd":{"line":5,"offset":13}},
            "pullRequestThreadContext":{"changeTrackingId":1,"iterationContext":{"firstComparingIteration":1,"secondComparingIteration":2}},
            "properties":{"Microsoft.TeamFoundation.Discussion.SupportsMarkdown":{"$type":"System.Int32","$value":1}},
            "comments":[
              {"id":2,"parentCommentId":1,"author":{"id":"u2","displayName":"B"},"content":"reply","publishedDate":"2026-01-01T11:00:00Z","commentType":"text"},
              {"id":1,"parentCommentId":0,"author":{"id":"u1","displayName":"A"},"content":"first","publishedDate":"2026-01-01T10:00:00Z",
               "lastContentUpdatedDate":"2026-01-01T10:30:00Z","commentType":"text"},
              {"id":3,"parentCommentId":1,"publishedDate":"2026-01-01T12:00:00Z","isDeleted":true,"commentType":"text"}
            ]}"#,
        );
        let mapped = thread.to_thread(&IterationCommits::default()).unwrap();
        assert_eq!(mapped.id, "148");
        assert_eq!(mapped.status, ThreadStatus::Resolved);
        assert!(!mapped.is_system && mapped.can_reply && mapped.can_resolve && !mapped.is_outdated);
        let anchor = mapped.anchor.unwrap();
        assert_eq!(
            (anchor.path.as_str(), anchor.old_line, anchor.new_line),
            ("src/main.rs", None, Some(5))
        );
        assert_eq!(
            mapped.comments.iter().map(|comment| comment.body.as_str()).collect::<Vec<_>>(),
            ["first", "reply"]
        );
        assert!(mapped.comments[0].updated_at.is_some());
        assert!(mapped.comments[1].updated_at.is_none());
        assert_eq!(thread.first_comment_id(), Some(1));
    }

    #[test]
    fn deleted_line_maps_to_old_line_and_tracking_marks_outdated() {
        let thread = thread(
            r#"{"id":7,"status":"active","publishedDate":"2026-01-01T10:00:00Z",
            "threadContext":{"filePath":"/a.txt","leftFileStart":{"line":9,"offset":1}},
            "pullRequestThreadContext":{"trackingCriteria":{"firstComparingIteration":1,"secondComparingIteration":3}},
            "comments":[{"id":1,"content":"x","publishedDate":"2026-01-01T10:00:00Z","commentType":"text"}]}"#,
        );
        let mapped = thread.to_thread(&IterationCommits::default()).unwrap();
        let anchor = mapped.anchor.unwrap();
        assert_eq!((anchor.old_line, anchor.new_line), (Some(9), None));
        assert!(mapped.is_outdated);
        assert_eq!(mapped.status, ThreadStatus::Active);
    }

    /// Iterations in the shape `GET .../iterations?includeCommits=false` answers (trimmed), and a
    /// thread tracked away from where it was created, as the threads list returns it.
    const ITERATIONS: &str = r#"{"value":[
        {"id":1,"author":{"displayName":"A"},"createdDate":"2026-01-01T09:00:00Z","reason":"create",
         "sourceRefCommit":{"commitId":"aaa1"},"targetRefCommit":{"commitId":"ttt1"},"commonRefCommit":{"commitId":"ccc1"}},
        {"id":2,"createdDate":"2026-01-02T09:00:00Z","reason":"push",
         "sourceRefCommit":{"commitId":"aaa2"},"targetRefCommit":{"commitId":"ttt2"},"commonRefCommit":{"commitId":"ccc2"}},
        {"id":3,"createdDate":"2026-01-03T09:00:00Z","reason":"forcePush",
         "sourceRefCommit":{"commitId":"aaa3"},"targetRefCommit":{"commitId":"ttt2"},"commonRefCommit":{"commitId":"ccc2"}}
    ],"count":3}"#;

    fn iterations() -> IterationCommits {
        let list: crate::dto::ValueList<crate::dto::iteration::Iteration> =
            serde_json::from_str(ITERATIONS).unwrap();
        IterationCommits::new(&list.value)
    }

    #[test]
    fn a_tracked_thread_keeps_the_diff_it_was_written_on() {
        let tracked = thread(
            r#"{"id":31,"status":"active","publishedDate":"2026-01-02T10:00:00Z",
            "threadContext":{"filePath":"/src/renamed.rs","rightFileStart":{"line":40,"offset":1},"rightFileEnd":{"line":42,"offset":5}},
            "pullRequestThreadContext":{"changeTrackingId":4,
              "iterationContext":{"firstComparingIteration":2,"secondComparingIteration":2},
              "trackingCriteria":{"firstComparingIteration":2,"secondComparingIteration":3,"origFilePath":"/src/old.rs",
                "origRightFileStart":{"line":12,"offset":1},"origRightFileEnd":{"line":14,"offset":7}}},
            "comments":[{"id":1,"content":"Why?","publishedDate":"2026-01-02T10:00:00Z","commentType":"text"}]}"#,
        );
        let mapped = tracked.to_thread(&iterations()).unwrap();
        assert!(mapped.is_outdated);
        assert_eq!(
            mapped.anchor.as_ref().map(|anchor| (anchor.path.as_str(), anchor.new_line)),
            Some(("src/renamed.rs", Some(42))),
            "the current anchor stays"
        );
        let origin = mapped.origin.expect("origin");
        assert_eq!(
            (origin.base_sha.as_deref(), origin.head_sha.as_str()),
            (Some("ccc2"), "aaa2"),
            "an iteration against itself starts at the common commit"
        );
        assert_eq!(
            (origin.anchor.path.as_str(), origin.anchor.old_line, origin.anchor.new_line),
            ("src/old.rs", None, Some(14))
        );
        assert_eq!(origin.anchor.range.as_ref().and_then(|range| range.start_new_line), Some(12));
        assert_eq!((origin.diff_hunk, origin.web_url), (None, None));

        let between = thread(
            r#"{"id":32,"publishedDate":"2026-01-03T10:00:00Z",
            "threadContext":{"filePath":"/a.txt","leftFileStart":{"line":3,"offset":1}},
            "pullRequestThreadContext":{"iterationContext":{"firstComparingIteration":1,"secondComparingIteration":3},
              "trackingCriteria":{"firstComparingIteration":1,"secondComparingIteration":3,"origLeftFileStart":{"line":5,"offset":1}}},
            "comments":[{"id":1,"content":"x","publishedDate":"2026-01-03T10:00:00Z","commentType":"text"}]}"#,
        );
        let origin = between.to_thread(&iterations()).unwrap().origin.expect("origin");
        assert_eq!(
            (origin.base_sha.as_deref(), origin.head_sha.as_str()),
            (Some("aaa1"), "aaa3"),
            "two iterations compare their source commits"
        );
        assert_eq!(
            (origin.anchor.path.as_str(), origin.anchor.old_line, origin.anchor.new_line),
            ("a.txt", Some(5), None),
            "the current path when the file kept it"
        );

        assert!(
            between.to_thread(&IterationCommits::default()).unwrap().origin.is_none(),
            "without the iterations there is no origin"
        );
        let current = thread(
            r#"{"id":33,"publishedDate":"2026-01-03T10:00:00Z","threadContext":{"filePath":"/a.txt","rightFileStart":{"line":3,"offset":1}},
            "pullRequestThreadContext":{"iterationContext":{"firstComparingIteration":3,"secondComparingIteration":3}},
            "comments":[{"id":1,"content":"x","publishedDate":"2026-01-03T10:00:00Z","commentType":"text"}]}"#,
        );
        let mapped = current.to_thread(&iterations()).unwrap();
        assert!(
            !mapped.is_outdated && mapped.origin.is_none(),
            "a thread that was not tracked is where it was written"
        );
    }

    #[test]
    fn system_threads_cannot_be_replied_or_resolved() {
        let by_property = thread(
            r#"{"id":8,"publishedDate":"2026-02-18T04:35:03.63Z","threadContext":null,
            "properties":{"CodeReviewThreadType":{"$type":"System.String","$value":"VoteUpdate"}},
            "comments":[{"id":1,"content":"A voted 10","publishedDate":"2026-02-18T04:35:03.63Z","commentType":"text"}]}"#,
        );
        let by_comment_type = thread(
            r#"{"id":9,"publishedDate":"2026-02-18T04:35:03.63Z","properties":null,
            "comments":[{"id":1,"content":"x","publishedDate":"2026-02-18T04:35:03.63Z","commentType":"system"}]}"#,
        );
        for thread in [by_property, by_comment_type] {
            let mapped = thread.to_thread(&IterationCommits::default()).unwrap();
            assert!(mapped.is_system && !mapped.can_reply && !mapped.can_resolve);
            assert!(mapped.anchor.is_none());
        }
    }

    #[test]
    fn skips_deleted_threads() {
        let deleted = thread(
            r#"{"id":11,"status":"closed","publishedDate":"2026-02-18T04:35:08.573Z","isDeleted":true,
            "comments":[{"id":1,"publishedDate":"2026-02-18T04:35:08.573Z","isDeleted":true,"commentType":"text"}]}"#,
        );
        assert!(deleted.to_thread(&IterationCommits::default()).is_none());
        let emptied = thread(
            r#"{"id":12,"publishedDate":"2026-02-18T04:35:08.573Z",
            "comments":[{"id":1,"publishedDate":"2026-02-18T04:35:08.573Z","isDeleted":true}]}"#,
        );
        assert!(emptied.to_thread(&IterationCommits::default()).is_none());
    }

    #[test]
    fn maps_thread_statuses() {
        for status in ["fixed", "wontFix", "closed", "byDesign"] {
            assert_eq!(status_from_ado(Some(status)), ThreadStatus::Resolved);
        }
        for status in [Some("active"), Some("pending"), Some("unknown"), None] {
            assert_eq!(status_from_ado(status), ThreadStatus::Active);
        }
        assert_eq!(status_to_ado(ThreadStatus::Resolved), "fixed");
    }

    #[test]
    fn builds_inline_thread_body() {
        let thread = NewThread {
            body: "Why?".to_owned(),
            anchor: Some(ThreadAnchor {
                path: "src/a.rs".to_owned(),
                old_path: None,
                old_line: Some(3),
                new_line: Some(4),
                range: None,
            }),
        };
        let body = new_thread_body(
            &thread,
            Some(IterationAnchor { latest_iteration: 3, change_tracking_id: Some(7) }),
        );
        assert_eq!(body["threadContext"]["filePath"], "/src/a.rs");
        assert_eq!(body["threadContext"]["rightFileStart"], json!({"line": 4, "offset": 1}));
        assert!(body["threadContext"].get("leftFileStart").is_none());
        assert_eq!(body["pullRequestThreadContext"]["changeTrackingId"], 7);
        assert_eq!(
            body["pullRequestThreadContext"]["iterationContext"]["secondComparingIteration"],
            3
        );
        assert_eq!(
            body["comments"][0],
            json!({"parentCommentId": 0, "content": "Why?", "commentType": 1})
        );
        assert_eq!(body["status"], "active");
    }

    /// The app's diff is the merge base against the head. A thread written there names the latest
    /// iteration on both sides, so a deleted line counts in the common commit and the origin of
    /// the thread, once tracked, is that same diff.
    #[test]
    fn new_threads_compare_the_latest_iteration_with_its_common_commit() {
        let deleted = NewThread {
            body: "x".to_owned(),
            anchor: Some(ThreadAnchor {
                path: "a.txt".to_owned(),
                old_path: None,
                old_line: Some(5),
                new_line: None,
                range: None,
            }),
        };
        let body = new_thread_body(
            &deleted,
            Some(IterationAnchor { latest_iteration: 3, change_tracking_id: Some(2) }),
        );
        let context = &body["pullRequestThreadContext"]["iterationContext"];
        assert_eq!(*context, json!({"firstComparingIteration": 3, "secondComparingIteration": 3}));

        let mut created = body.clone();
        created["id"] = json!(40);
        created["publishedDate"] = json!("2026-01-03T10:00:00Z");
        created["comments"][0]["id"] = json!(1);
        created["comments"][0]["publishedDate"] = json!("2026-01-03T10:00:00Z");
        // Requests send the comment type as a number, responses name it.
        created["comments"][0]["commentType"] = json!("text");
        created["pullRequestThreadContext"]["trackingCriteria"] = json!({
            "firstComparingIteration": 3, "secondComparingIteration": 3,
            "origLeftFileStart": {"line": 5, "offset": 1}, "origLeftFileEnd": {"line": 5, "offset": 1}
        });
        let tracked: Thread = serde_json::from_value(created).unwrap();
        let origin = tracked.to_thread(&iterations()).unwrap().origin.expect("origin");
        assert_eq!(
            (origin.base_sha.as_deref(), origin.head_sha.as_str()),
            (Some("ccc2"), "aaa3"),
            "the merge base and the head the comment was written on"
        );
    }

    #[test]
    fn builds_deleted_line_and_general_bodies() {
        let deleted = NewThread {
            body: "x".to_owned(),
            anchor: Some(ThreadAnchor {
                path: "a".to_owned(),
                old_path: None,
                old_line: Some(2),
                new_line: None,
                range: None,
            }),
        };
        let body = new_thread_body(&deleted, None);
        assert_eq!(body["threadContext"]["leftFileEnd"], json!({"line": 2, "offset": 1}));
        assert!(body.get("pullRequestThreadContext").is_none());
        let general = new_thread_body(&NewThread { body: "x".to_owned(), anchor: None }, None);
        assert!(general.get("threadContext").is_none());
    }

    fn range_thread(
        end: (Option<u32>, Option<u32>),
        start: (Option<u32>, Option<u32>),
        side: DiffSide,
    ) -> NewThread {
        let detail = RangeDetail {
            start: FilePositions { old: 6, new: 7 },
            end: FilePositions { old: 9, new: 11 },
            start_offset: 3,
            end_offset: 18,
        };
        let range = LineRange {
            start_old_line: start.0,
            start_new_line: start.1,
            side,
            detail: Some(detail),
        };
        NewThread {
            body: "x".to_owned(),
            anchor: Some(ThreadAnchor {
                path: "a".to_owned(),
                old_path: None,
                old_line: end.0,
                new_line: end.1,
                range: Some(range),
            }),
        }
    }

    #[test]
    fn builds_range_bodies_on_the_picked_side() {
        let left = new_thread_body(
            &range_thread((Some(9), None), (Some(6), Some(7)), DiffSide::Old),
            None,
        );
        assert_eq!(
            left["threadContext"],
            json!({ "filePath": "/a", "leftFileStart": {"line": 6, "offset": 3}, "leftFileEnd": {"line": 9, "offset": 18} })
        );
        // A deleted first line has no head number: it counts where it was removed in the head file.
        let mixed =
            new_thread_body(&range_thread((None, Some(11)), (Some(6), None), DiffSide::New), None);
        assert_eq!(
            mixed["threadContext"],
            json!({ "filePath": "/a", "rightFileStart": {"line": 7, "offset": 3}, "rightFileEnd": {"line": 11, "offset": 18} })
        );
    }

    #[test]
    fn maps_a_span_over_several_lines_to_a_range() {
        let right = thread(
            r#"{"id":20,"publishedDate":"2026-01-01T10:00:00Z",
            "threadContext":{"filePath":"/a.rs","rightFileStart":{"line":3,"offset":1},"rightFileEnd":{"line":6,"offset":9}},
            "comments":[{"id":1,"content":"x","publishedDate":"2026-01-01T10:00:00Z","commentType":"text"}]}"#,
        );
        let anchor = right.to_thread(&IterationCommits::default()).unwrap().anchor.unwrap();
        assert_eq!((anchor.old_line, anchor.new_line), (None, Some(6)));
        assert_eq!(
            anchor.range,
            Some(LineRange {
                start_old_line: None,
                start_new_line: Some(3),
                side: DiffSide::New,
                detail: None
            })
        );
        let left = thread(
            r#"{"id":21,"publishedDate":"2026-01-01T10:00:00Z",
            "threadContext":{"filePath":"/a.rs","leftFileStart":{"line":2,"offset":1},"leftFileEnd":{"line":4,"offset":1}},
            "comments":[{"id":1,"content":"x","publishedDate":"2026-01-01T10:00:00Z","commentType":"text"}]}"#,
        );
        let anchor = left.to_thread(&IterationCommits::default()).unwrap().anchor.unwrap();
        assert_eq!(
            (anchor.old_line, anchor.range.map(|range| (range.start_old_line, range.side))),
            (Some(4), Some((Some(2), DiffSide::Old)))
        );
    }
}
