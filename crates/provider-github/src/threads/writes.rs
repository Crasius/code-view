//! Creating threads, replying and resolving.

use reqwest::Method;
use review_core::{
    DiffSide, NewThread, ProviderError, Result, ReviewThread, ThreadAnchor, ThreadComment,
    ThreadStatus, http,
};
use serde::de::IgnoredAny;
use serde_json::{Value, json};

use super::{COMMENT_FRAGMENT, THREAD_FRAGMENT, parse_thread_id, review_threads_of};
use crate::client::GitHubClient;
use crate::dto::comment::{IssueCommentDto, ReviewCommentDto};
use crate::dto::review_thread::{AddReplyData, RepositoryData, ReviewThreadsField};
use crate::graphql::{self, ACTOR_FRAGMENT};
use crate::mapping::thread_id::ThreadId;
use crate::mapping::threads as mapping;
use crate::pull_requests;
use crate::repo_key::RepoKey;

/// A new thread is among the newest; look at most this many pages of 20 back for it.
const MAX_LOOKUP_PAGES: usize = 5;

const NEWEST_THREADS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $name) { pullRequest(number: $number) {
    reviewThreads(last: 20, before: $cursor) { pageInfo { hasPreviousPage startCursor } nodes { ...thread } }
  } }
}";

const REPLY_MUTATION: &str = "mutation($thread: ID!, $body: String!) {
  addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $thread, body: $body }) { comment { ...comment } }
}";

const RESOLVE_MUTATION: &str =
    "mutation($id: ID!) { resolveReviewThread(input: { threadId: $id }) { thread { id } } }";
const UNRESOLVE_MUTATION: &str =
    "mutation($id: ID!) { unresolveReviewThread(input: { threadId: $id }) { thread { id } } }";

pub(crate) async fn create(
    client: &GitHubClient,
    key: &str,
    number: u64,
    thread: &NewThread,
) -> Result<ReviewThread> {
    client.require_token("Commenting")?;
    let repo = RepoKey::parse(key)?;
    let Some(anchor) = &thread.anchor else {
        let request = client
            .rest(Method::POST, &format!("{}/comments", repo.issue_path(number)))
            .json(&json!({ "body": thread.body }));
        let created: IssueCommentDto = http::json(request).await?;
        return Ok(mapping::issue_comment_thread(created));
    };
    let head_sha = pull_requests::fetch(client, &repo, number).await?.head.sha;
    let request = client
        .rest(Method::POST, &format!("{}/comments", repo.pull_path(number)))
        .json(&inline_comment_body(&thread.body, anchor, &head_sha));
    let created: ReviewCommentDto = http::json(request).await?;
    if let Some(thread) = find_thread_by_root(client, &repo, number, created.id).await? {
        return Ok(thread);
    }
    // GraphQL has not indexed the thread yet: hand out the REST id, which replies still work with.
    let mut fallback = mapping::rest_threads(vec![created])
        .into_iter()
        .next()
        .ok_or_else(|| ProviderError::Decode("created comment produced no thread".to_owned()))?;
    fallback.can_reply = true;
    Ok(fallback)
}

/// Added lines comment on the head side (`RIGHT`), deleted lines on the base side (`LEFT`);
/// an anchor without lines comments on the file. Context lines take the side the range was
/// picked in, or `RIGHT` for a single line. A range sends its first line as
/// `start_line`/`start_side` and its last as `line`/`side`, as the REST docs ask.
fn inline_comment_body(body: &str, anchor: &ThreadAnchor, head_sha: &str) -> Value {
    let mut payload = json!({ "body": body, "commit_id": head_sha, "path": anchor.path });
    let context_side = anchor.range.as_ref().map_or(DiffSide::New, |range| range.side);
    let Some((side, line)) = github_line(anchor.old_line, anchor.new_line, context_side) else {
        payload["subject_type"] = Value::from("file");
        return payload;
    };
    if let Some(range) = &anchor.range
        && let Some((start_side, start_line)) =
            github_line(range.start_old_line, range.start_new_line, context_side)
    {
        payload["start_line"] = Value::from(start_line);
        payload["start_side"] = Value::from(side_name(start_side));
    }
    payload["line"] = Value::from(line);
    payload["side"] = Value::from(side_name(side));
    payload
}

/// The side and number GitHub counts a line by; a line in both files counts in `context_side`.
fn github_line(
    old_line: Option<u32>,
    new_line: Option<u32>,
    context_side: DiffSide,
) -> Option<(DiffSide, u32)> {
    match (old_line, new_line) {
        (Some(old), Some(new)) => Some(match context_side {
            DiffSide::Old => (DiffSide::Old, old),
            DiffSide::New => (DiffSide::New, new),
        }),
        (None, Some(new)) => Some((DiffSide::New, new)),
        (Some(old), None) => Some((DiffSide::Old, old)),
        (None, None) => None,
    }
}

fn side_name(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Old => "LEFT",
        DiffSide::New => "RIGHT",
    }
}

/// Pages backwards through the newest review threads for the one rooted at `comment_id`.
async fn find_thread_by_root(
    client: &GitHubClient,
    repo: &RepoKey<'_>,
    number: u64,
    comment_id: u64,
) -> Result<Option<ReviewThread>> {
    let document =
        [NEWEST_THREADS_QUERY, THREAD_FRAGMENT, COMMENT_FRAGMENT, ACTOR_FRAGMENT].concat();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_LOOKUP_PAGES {
        let variables =
            json!({ "owner": repo.owner, "name": repo.name, "number": number, "cursor": cursor });
        let data: RepositoryData<ReviewThreadsField> =
            graphql::execute(client, "Loading review threads", &document, variables).await?;
        let (page, nodes) = review_threads_of(data)?.into_nodes();
        let found =
            nodes.into_iter().rev().map(|dto| mapping::review_thread(dto).0).find(|thread| {
                thread.comments.first().is_some_and(|comment| comment.id == comment_id.to_string())
            });
        if found.is_some() {
            return Ok(found);
        }
        cursor = page.start_cursor.filter(|_| page.has_previous_page);
        if cursor.is_none() {
            break;
        }
    }
    Ok(None)
}

pub(crate) async fn reply(
    client: &GitHubClient,
    key: &str,
    number: u64,
    thread_id: &str,
    body: &str,
) -> Result<ThreadComment> {
    client.require_token("Replying")?;
    let repo = RepoKey::parse(key)?;
    match parse_thread_id(thread_id)? {
        ThreadId::ReviewThread(node_id) => {
            let document = [REPLY_MUTATION, COMMENT_FRAGMENT, ACTOR_FRAGMENT].concat();
            let variables = json!({ "thread": node_id, "body": body });
            let data: AddReplyData = graphql::execute(client, "Replying", &document, variables).await?;
            data.add_pull_request_review_thread_reply
                .and_then(|payload| payload.comment)
                .map(mapping::graphql_comment)
                .ok_or_else(|| ProviderError::Decode("reply mutation returned no comment".to_owned()))
        }
        ThreadId::ReviewComment(comment_id) => {
            let path = format!("{}/comments/{comment_id}/replies", repo.pull_path(number));
            let request = client.rest(Method::POST, &path).json(&json!({ "body": body }));
            let created: ReviewCommentDto = http::json(request).await?;
            Ok(mapping::rest_comment(created))
        }
        ThreadId::IssueComment(_) | ThreadId::ReviewBody(_) => Err(ProviderError::Unsupported(
            "GitHub pull request comments outside review threads take no replies; add a new comment.".to_owned(),
        )),
    }
}

pub(crate) async fn set_status(
    client: &GitHubClient,
    thread_id: &str,
    status: ThreadStatus,
) -> Result<()> {
    client.require_token("Resolving threads")?;
    let ThreadId::ReviewThread(node_id) = parse_thread_id(thread_id)? else {
        return Err(ProviderError::Unsupported(
            "Only GitHub review threads can be resolved.".to_owned(),
        ));
    };
    let mutation = match status {
        ThreadStatus::Resolved => RESOLVE_MUTATION,
        ThreadStatus::Active => UNRESOLVE_MUTATION,
    };
    let _: IgnoredAny =
        graphql::execute(client, "Resolving threads", mutation, json!({ "id": node_id })).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::inline_comment_body;
    use review_core::{DiffSide, LineRange, ThreadAnchor};
    use serde_json::json;

    fn anchor(old_line: Option<u32>, new_line: Option<u32>) -> ThreadAnchor {
        ThreadAnchor {
            path: "src/a.rs".to_owned(),
            old_path: None,
            old_line,
            new_line,
            range: None,
        }
    }

    fn range(
        start: (Option<u32>, Option<u32>),
        end: (Option<u32>, Option<u32>),
        side: DiffSide,
    ) -> ThreadAnchor {
        ThreadAnchor {
            range: Some(LineRange {
                start_old_line: start.0,
                start_new_line: start.1,
                side,
                detail: None,
            }),
            ..anchor(end.0, end.1)
        }
    }

    #[test]
    fn ranges_send_their_first_and_last_line() {
        let head = range((Some(4), Some(5)), (None, Some(9)), DiffSide::New);
        assert_eq!(
            inline_comment_body("b", &head, "sha"),
            json!({ "body": "b", "commit_id": "sha", "path": "src/a.rs",
                    "start_line": 5, "start_side": "RIGHT", "line": 9, "side": "RIGHT" })
        );
        // Picked in the base pane: context lines count by their base number.
        let base = range((Some(4), Some(5)), (Some(7), None), DiffSide::Old);
        assert_eq!(
            inline_comment_body("b", &base, "sha"),
            json!({ "body": "b", "commit_id": "sha", "path": "src/a.rs",
                    "start_line": 4, "start_side": "LEFT", "line": 7, "side": "LEFT" })
        );
        // From a deleted line down to an added line, as one column shows them.
        let mixed = range((Some(6), None), (None, Some(8)), DiffSide::New);
        assert_eq!(
            inline_comment_body("b", &mixed, "sha"),
            json!({ "body": "b", "commit_id": "sha", "path": "src/a.rs",
                    "start_line": 6, "start_side": "LEFT", "line": 8, "side": "RIGHT" })
        );
    }

    #[test]
    fn added_and_context_lines_comment_on_the_right() {
        let expected = json!({ "body": "b", "commit_id": "sha", "path": "src/a.rs", "line": 7, "side": "RIGHT" });
        assert_eq!(inline_comment_body("b", &anchor(None, Some(7)), "sha"), expected);
        assert_eq!(inline_comment_body("b", &anchor(Some(5), Some(7)), "sha"), expected);
    }

    #[test]
    fn deleted_lines_comment_on_the_left() {
        assert_eq!(
            inline_comment_body("b", &anchor(Some(5), None), "sha"),
            json!({ "body": "b", "commit_id": "sha", "path": "src/a.rs", "line": 5, "side": "LEFT" })
        );
    }

    #[test]
    fn no_lines_comment_on_the_file() {
        assert_eq!(
            inline_comment_body("b", &anchor(None, None), "sha"),
            json!({ "body": "b", "commit_id": "sha", "path": "src/a.rs", "subject_type": "file" })
        );
    }
}
