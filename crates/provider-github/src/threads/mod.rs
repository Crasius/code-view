//! Loading every conversation of a pull request: review threads (GraphQL with a token, REST without),
//! general comments and review bodies. `writes` creates threads, replies and resolves; `edits`
//! edits and deletes comments.

pub(crate) mod edits;
pub(crate) mod writes;

use review_core::{ProviderError, Result, ReviewThread, ThreadComment};
use serde_json::json;

use crate::client::{GitHubClient, PER_PAGE, Paging};
use crate::dto::comment::{IssueCommentDto, ReviewCommentDto};
use crate::dto::review_thread::{
    NodeData, RepositoryData, ReviewThreadDto, ReviewThreadsField, ThreadCommentsField,
};
use crate::graphql::{self, ACTOR_FRAGMENT, Connection};
use crate::mapping::thread_id::ThreadId;
use crate::mapping::threads as mapping;
use crate::pull_requests::reviews;
use crate::repo_key::RepoKey;

/// 100 pages of 100 cover the largest pull requests GitHub renders.
const MAX_PAGES: usize = 100;

const THREADS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $name) { pullRequest(number: $number) {
    reviewThreads(first: 100, after: $cursor) { pageInfo { hasNextPage endCursor } nodes { ...thread } }
  } }
}";

const MORE_COMMENTS_QUERY: &str = "query($id: ID!, $cursor: String) {
  node(id: $id) { ... on PullRequestReviewThread {
    comments(first: 100, after: $cursor) { pageInfo { hasNextPage endCursor } nodes { ...comment } }
  } }
}";

pub(crate) const THREAD_FRAGMENT: &str = " fragment thread on PullRequestReviewThread {
  id isResolved isOutdated path line originalLine diffSide startLine originalStartLine startDiffSide
  comments(first: 100) { pageInfo { hasNextPage endCursor } nodes { ...comment } }
  origin: comments(first: 1) { nodes { originalCommit { oid } diffHunk url } }
}";

pub(crate) const COMMENT_FRAGMENT: &str = " fragment comment on PullRequestReviewComment { id databaseId body createdAt lastEditedAt author { ...actor } }";

pub(crate) async fn threads(
    client: &GitHubClient,
    key: &str,
    number: u64,
) -> Result<Vec<ReviewThread>> {
    let repo = RepoKey::parse(key)?;
    let review_threads = async {
        if client.has_token() {
            graphql_threads(client, &repo, number).await
        } else {
            rest_threads(client, &repo, number).await
        }
    };
    let (mut threads, issue_comments, reviews) = tokio::try_join!(
        review_threads,
        issue_comments(client, &repo, number),
        reviews::list(client, &repo, number)
    )?;
    threads.extend(issue_comments.into_iter().map(mapping::issue_comment_thread));
    threads.extend(reviews.into_iter().filter_map(mapping::review_body_thread));
    mapping::sort_oldest_first(&mut threads);
    Ok(threads)
}

async fn graphql_threads(
    client: &GitHubClient,
    repo: &RepoKey<'_>,
    number: u64,
) -> Result<Vec<ReviewThread>> {
    let document = [THREADS_QUERY, THREAD_FRAGMENT, COMMENT_FRAGMENT, ACTOR_FRAGMENT].concat();
    let mut threads = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let variables =
            json!({ "owner": repo.owner, "name": repo.name, "number": number, "cursor": cursor });
        let data: RepositoryData<ReviewThreadsField> =
            graphql::execute(client, "Loading review threads", &document, variables).await?;
        let (page, nodes) = review_threads_of(data)?.into_nodes();
        for dto in nodes {
            let (mut thread, comments_page) = mapping::review_thread(dto);
            if comments_page.has_next_page {
                thread.comments.extend(
                    remaining_comments(client, &thread.id, comments_page.end_cursor).await?,
                );
            }
            threads.push(thread);
        }
        cursor = page.end_cursor.filter(|_| page.has_next_page);
        if cursor.is_none() {
            break;
        }
    }
    Ok(threads)
}

/// Comments beyond the first 100 of one thread.
async fn remaining_comments(
    client: &GitHubClient,
    thread_id: &str,
    mut cursor: Option<String>,
) -> Result<Vec<ThreadComment>> {
    let document = [MORE_COMMENTS_QUERY, COMMENT_FRAGMENT, ACTOR_FRAGMENT].concat();
    let mut comments = Vec::new();
    for _ in 0..MAX_PAGES {
        let variables = json!({ "id": thread_id, "cursor": cursor });
        let data: NodeData<ThreadCommentsField> =
            graphql::execute(client, "Loading review comments", &document, variables).await?;
        let Some(field) = data.node else {
            break;
        };
        let (page, nodes) = field.comments.into_nodes();
        comments.extend(nodes.into_iter().map(mapping::graphql_comment));
        cursor = page.end_cursor.filter(|_| page.has_next_page);
        if cursor.is_none() {
            break;
        }
    }
    Ok(comments)
}

pub(super) fn parse_thread_id(thread_id: &str) -> Result<ThreadId<'_>> {
    ThreadId::parse(thread_id)
        .ok_or_else(|| ProviderError::Local(format!("'{thread_id}' is not a GitHub thread id.")))
}

/// Unwraps `repository.pullRequest.reviewThreads`; a missing level means the pull request is not visible.
pub(crate) fn review_threads_of(
    data: RepositoryData<ReviewThreadsField>,
) -> Result<Connection<ReviewThreadDto>> {
    data.repository
        .and_then(|repository| repository.pull_request)
        .map(|pull_request| pull_request.review_threads)
        .ok_or_else(|| ProviderError::Status {
            status: 404,
            path: "/graphql".to_owned(),
            body: "pull request not found".to_owned(),
        })
}

async fn rest_threads(
    client: &GitHubClient,
    repo: &RepoKey<'_>,
    number: u64,
) -> Result<Vec<ReviewThread>> {
    let path = format!("{}/comments", repo.pull_path(number));
    let query = [
        ("per_page", PER_PAGE.to_string()),
        ("sort", "created".to_owned()),
        ("direction", "asc".to_owned()),
    ];
    let comments = client
        .collect_pages(&path, &query, Paging::pages(MAX_PAGES), |page: Vec<ReviewCommentDto>| page)
        .await?;
    Ok(mapping::rest_threads(comments))
}

async fn issue_comments(
    client: &GitHubClient,
    repo: &RepoKey<'_>,
    number: u64,
) -> Result<Vec<IssueCommentDto>> {
    let path = format!("{}/comments", repo.issue_path(number));
    let query = [("per_page", PER_PAGE.to_string())];
    client
        .collect_pages(&path, &query, Paging::pages(MAX_PAGES), |page: Vec<IssueCommentDto>| page)
        .await
}
