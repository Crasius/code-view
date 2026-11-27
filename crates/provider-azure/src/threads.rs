use reqwest::Method;
use review_core::provider::CommentActions;
use review_core::{
    NewThread, ProviderError, Result, ReviewThread, ThreadAnchor, ThreadComment, ThreadStatus,
};
use serde_json::json;

use crate::client::{Client, RepoKey};
use crate::dto::ValueList;
use crate::dto::iteration::{IterationCommits, find_change_tracking_id};
use crate::dto::thread::{
    Comment, IterationAnchor, Thread, comment_body, new_thread_body, status_to_ado,
};
use crate::iterations;

/// The threads; when any was tracked away from where it was created, one more request reads the
/// iterations' commits for the version it was written on. Iterations need sign-in even on public
/// projects, so anonymous connections get tracked threads without their origin.
pub(crate) async fn list(client: &Client, repo: &str, number: u64) -> Result<Vec<ReviewThread>> {
    let key = RepoKey::parse(repo)?;
    let threads: ValueList<Thread> =
        client.get(&client.pull_request_api(key, number, "threads"), &[]).await?;
    let has_tracked = threads.value.iter().any(|thread| !thread.is_deleted && thread.is_tracked());
    let iterations = if has_tracked && client.has_token() {
        IterationCommits::new(&iterations::list(client, key, number).await?)
    } else {
        IterationCommits::default()
    };
    let mut sorted: Vec<&Thread> = threads.value.iter().collect();
    sorted.sort_by(|a, b| a.published_date.cmp(&b.published_date).then_with(|| a.id.cmp(&b.id)));
    Ok(sorted.into_iter().filter_map(|thread| thread.to_thread(&iterations)).collect())
}

pub(crate) async fn create(
    client: &Client,
    repo: &str,
    number: u64,
    thread: &NewThread,
) -> Result<ReviewThread> {
    client.require_token("Commenting")?;
    let key = RepoKey::parse(repo)?;
    let iteration = match &thread.anchor {
        Some(anchor) => Some(iteration_anchor(client, key, number, anchor).await?),
        None => None,
    };
    let body = new_thread_body(thread, iteration);
    let created: Thread = client
        .send_json(Method::POST, &client.pull_request_api(key, number, "threads"), &body)
        .await?;
    created.to_thread(&IterationCommits::default()).ok_or_else(|| {
        ProviderError::Decode(format!(
            "Azure DevOps returned thread {} without comments",
            created.id
        ))
    })
}

/// Latest iteration plus the file's change tracking id, so the web UI shows the comment on the file.
async fn iteration_anchor(
    client: &Client,
    repo: RepoKey<'_>,
    number: u64,
    anchor: &ThreadAnchor,
) -> Result<IterationAnchor> {
    let latest_iteration = iterations::latest(&iterations::list(client, repo, number).await?)
        .ok_or_else(|| ProviderError::Decode(format!("pull request {number} has no iterations")))?;
    let change_tracking_id =
        iterations::changes_until(client, repo, number, latest_iteration, |entries| {
            find_change_tracking_id(entries, anchor)
        })
        .await?;
    Ok(IterationAnchor { latest_iteration, change_tracking_id })
}

pub(crate) async fn reply(
    client: &Client,
    repo: &str,
    number: u64,
    thread_id: &str,
    body: &str,
) -> Result<ThreadComment> {
    client.require_token("Replying")?;
    let key = RepoKey::parse(repo)?;
    let thread_url =
        client.pull_request_api(key, number, &format!("threads/{}", parse_thread_id(thread_id)?));
    let thread: Thread = client.get(&thread_url, &[]).await?;
    let parent = thread.first_comment_id().unwrap_or(1);
    let comment: Comment = client
        .send_json(Method::POST, &format!("{thread_url}/comments"), &comment_body(parent, body))
        .await?;
    Ok(comment.to_comment())
}

pub(crate) async fn set_status(
    client: &Client,
    repo: &str,
    number: u64,
    thread_id: &str,
    status: ThreadStatus,
) -> Result<()> {
    client.require_token("Resolving a thread")?;
    let key = RepoKey::parse(repo)?;
    let url =
        client.pull_request_api(key, number, &format!("threads/{}", parse_thread_id(thread_id)?));
    client.send_unit(Method::PATCH, &url, Some(&json!({ "status": status_to_ado(status) }))).await
}

/// System threads (votes, pushes, status notices) are Azure DevOps' own; every comment of another
/// thread can be edited and deleted. A system comment inside a user thread is never the user's.
pub(crate) fn comment_actions(thread: &ReviewThread) -> CommentActions {
    if thread.is_system { CommentActions::NONE } else { CommentActions::EDIT_AND_DELETE }
}

/// The pull request page's query that opens the thread; a thread without comments has none.
pub(crate) fn page_suffix(thread: &ReviewThread) -> Option<String> {
    thread.comments.first().map(|_| format!("?discussionId={}", thread.id))
}

pub(crate) async fn edit_comment(
    client: &Client,
    repo: &str,
    number: u64,
    thread_id: &str,
    comment_id: &str,
    body: &str,
) -> Result<ThreadComment> {
    client.require_token("Editing comments")?;
    let url = comment_url(client, repo, number, thread_id, comment_id)?;
    let comment: Comment =
        client.send_json(Method::PATCH, &url, &json!({ "content": body })).await?;
    Ok(comment.to_comment())
}

/// Azure DevOps keeps a deleted comment with `isDeleted: true`; the listing leaves it out, and a
/// thread whose comments are all deleted with it.
pub(crate) async fn delete_comment(
    client: &Client,
    repo: &str,
    number: u64,
    thread_id: &str,
    comment_id: &str,
) -> Result<()> {
    client.require_token("Deleting comments")?;
    let url = comment_url(client, repo, number, thread_id, comment_id)?;
    client.send_unit::<()>(Method::DELETE, &url, None).await
}

fn comment_url(
    client: &Client,
    repo: &str,
    number: u64,
    thread_id: &str,
    comment_id: &str,
) -> Result<String> {
    let key = RepoKey::parse(repo)?;
    let comment: u32 = comment_id.parse().map_err(|_| {
        ProviderError::Local(format!("'{comment_id}' is not an Azure DevOps comment id."))
    })?;
    let path = format!("threads/{}/comments/{comment}", parse_thread_id(thread_id)?);
    Ok(client.pull_request_api(key, number, &path))
}

fn parse_thread_id(thread_id: &str) -> Result<u64> {
    thread_id.parse().map_err(|_| {
        ProviderError::Local(format!("'{thread_id}' is not an Azure DevOps thread id."))
    })
}

#[cfg(test)]
mod tests {
    use review_core::{ReviewThread, ThreadComment, ThreadStatus, UserIdentity};

    use super::page_suffix;

    fn thread(id: &str, comment_ids: &[&str]) -> ReviewThread {
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
            comments: comment_ids.iter().map(comment).collect(),
        }
    }

    #[test]
    fn opens_the_thread_by_its_id() {
        let cases = [("17", &["1"][..], Some("?discussionId=17")), ("17", &[][..], None)];
        for (id, comment_ids, expected) in cases {
            assert_eq!(page_suffix(&thread(id, comment_ids)).as_deref(), expected, "{id}");
        }
    }
}
