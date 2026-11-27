//! Editing and deleting the user's own comments. The thread id says which REST resource a comment
//! is: review thread comments are pull request review comments (by their database id), general
//! comments are issue comments, and a submitted review's body belongs to the review, which can be
//! updated but never deleted.

use reqwest::Method;
use review_core::provider::CommentActions;
use review_core::{ProviderError, Result, ReviewThread, ThreadComment, http};
use serde_json::json;

use super::parse_thread_id;
use crate::client::GitHubClient;
use crate::dto::comment::{IssueCommentDto, ReviewCommentDto};
use crate::dto::review::ReviewDto;
use crate::mapping::thread_id::ThreadId;
use crate::mapping::threads as mapping;
use crate::repo_key::RepoKey;

/// What the REST API accepts for the comments of `thread`; threads not yet on GitHub (ids the
/// app made up while a post is on its way) get nothing.
pub(crate) fn actions(thread: &ReviewThread) -> CommentActions {
    match ThreadId::parse(&thread.id) {
        Some(
            ThreadId::ReviewThread(_) | ThreadId::ReviewComment(_) | ThreadId::IssueComment(_),
        ) => CommentActions::EDIT_AND_DELETE,
        Some(ThreadId::ReviewBody(_)) => CommentActions::EDIT_ONLY,
        None => CommentActions::NONE,
    }
}

pub(crate) async fn edit(
    client: &GitHubClient,
    key: &str,
    number: u64,
    thread_id: &str,
    comment_id: &str,
    body: &str,
) -> Result<ThreadComment> {
    client.require_token("Editing comments")?;
    let repo = RepoKey::parse(key)?;
    let payload = json!({ "body": body });
    match resource(thread_id, comment_id)? {
        Resource::ReviewComment(id) => {
            let path = format!("{}/pulls/comments/{id}", repo.api_path());
            let edited: ReviewCommentDto =
                http::json(client.rest(Method::PATCH, &path).json(&payload)).await?;
            Ok(mapping::rest_comment(edited))
        }
        Resource::IssueComment(id) => {
            let path = format!("{}/issues/comments/{id}", repo.api_path());
            let edited: IssueCommentDto =
                http::json(client.rest(Method::PATCH, &path).json(&payload)).await?;
            Ok(mapping::issue_comment(edited))
        }
        Resource::ReviewBody(id) => {
            let path = format!("{}/reviews/{id}", repo.pull_path(number));
            let edited: ReviewDto =
                http::json(client.rest(Method::PUT, &path).json(&payload)).await?;
            mapping::review_body_thread(edited)
                .and_then(|thread| thread.comments.into_iter().next())
                .ok_or_else(|| ProviderError::Decode("the edited review has no body".to_owned()))
        }
    }
}

pub(crate) async fn delete(
    client: &GitHubClient,
    key: &str,
    thread_id: &str,
    comment_id: &str,
) -> Result<()> {
    client.require_token("Deleting comments")?;
    let repo = RepoKey::parse(key)?;
    let path = match resource(thread_id, comment_id)? {
        Resource::ReviewComment(id) => format!("{}/pulls/comments/{id}", repo.api_path()),
        Resource::IssueComment(id) => format!("{}/issues/comments/{id}", repo.api_path()),
        Resource::ReviewBody(_) => {
            return Err(ProviderError::Unsupported(
                "GitHub keeps submitted reviews; edit the review's text instead.".to_owned(),
            ));
        }
    };
    http::unit(client.rest(Method::DELETE, &path)).await
}

/// The REST resource behind one comment, by its numeric id.
enum Resource {
    ReviewComment(u64),
    IssueComment(u64),
    ReviewBody(u64),
}

fn resource(thread_id: &str, comment_id: &str) -> Result<Resource> {
    let thread = parse_thread_id(thread_id)?;
    // GraphQL comments carry their REST database id; one without it cannot be addressed by REST.
    let id: u64 = comment_id.parse().map_err(|_| {
        ProviderError::Local(format!("'{comment_id}' is not a GitHub REST comment id."))
    })?;
    Ok(match thread {
        ThreadId::ReviewThread(_) | ThreadId::ReviewComment(_) => Resource::ReviewComment(id),
        ThreadId::IssueComment(_) => Resource::IssueComment(id),
        ThreadId::ReviewBody(_) => Resource::ReviewBody(id),
    })
}

#[cfg(test)]
mod tests {
    use review_core::provider::CommentActions;
    use review_core::{ReviewThread, ThreadStatus};

    use super::actions;

    fn thread(id: &str) -> ReviewThread {
        ReviewThread {
            id: id.to_owned(),
            status: ThreadStatus::Active,
            anchor: None,
            is_outdated: false,
            origin: None,
            is_system: false,
            can_reply: false,
            can_resolve: false,
            comments: Vec::new(),
        }
    }

    #[test]
    fn review_bodies_can_be_edited_but_not_deleted() {
        assert_eq!(actions(&thread("PRRT_kwDOA")), CommentActions::EDIT_AND_DELETE);
        assert_eq!(actions(&thread("issue-comment:5")), CommentActions::EDIT_AND_DELETE);
        assert_eq!(actions(&thread("review-comment:5")), CommentActions::EDIT_AND_DELETE);
        assert_eq!(actions(&thread("review:8")), CommentActions::EDIT_ONLY);
        assert_eq!(actions(&thread("pending:general")), CommentActions::NONE);
    }
}
