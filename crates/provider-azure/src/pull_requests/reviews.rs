//! Votes and reviewer changes. A vote is the reviewer entry of the signed-in user; adding a
//! reviewer resolves the typed user through the identities API first, except the signed-in user
//! (known from `connectionData`), whose identity id is used as it is.

use reqwest::Method;
use review_core::{NewThread, Result, ReviewVote, UserIdentity};
use serde_json::json;

use crate::client::{Client, RepoKey};
use crate::dto::identity::vote_to_ado;
use crate::identity::{CurrentUsers, find_identity};
use crate::threads;

fn reviewer_url(client: &Client, repo: RepoKey<'_>, number: u64, reviewer_id: &str) -> String {
    client.pull_request_api(
        repo,
        number,
        &format!("reviewers/{}", urlencoding::encode(reviewer_id)),
    )
}

/// Azure DevOps has no review body: a non-empty comment becomes a general thread after the vote.
pub(crate) async fn set_vote(
    client: &Client,
    users: &CurrentUsers,
    repo: &str,
    number: u64,
    vote: ReviewVote,
    comment: Option<&str>,
) -> Result<()> {
    client.require_token("Voting")?;
    let key = RepoKey::parse(repo)?;
    let me = users.get(client, key.organization).await?;
    let body = json!({ "vote": vote_to_ado(vote) });
    client.send_unit(Method::PUT, &reviewer_url(client, key, number, &me.id), Some(&body)).await?;
    if let Some(comment) = comment.map(str::trim).filter(|comment| !comment.is_empty()) {
        let thread = NewThread { body: comment.to_owned(), anchor: None };
        threads::create(client, repo, number, &thread).await?;
    }
    Ok(())
}

/// Vote 0 ("no vote") on a new reviewer entry: the reviewer is asked, nothing is decided yet.
pub(crate) async fn add_reviewer(
    client: &Client,
    users: &CurrentUsers,
    repo: &str,
    number: u64,
    user: &str,
) -> Result<()> {
    client.require_token("Adding a reviewer")?;
    let key = RepoKey::parse(repo)?;
    let me = users.cached(key.organization).filter(|me| names_identity(me, user));
    let id = match me {
        Some(me) => me.id,
        None => find_identity(client, key.organization, user).await?.id,
    };
    let body = json!({ "vote": 0 });
    client.send_unit(Method::PUT, &reviewer_url(client, key, number, &id), Some(&body)).await
}

/// `input` is the identity's id or its account name (any case), as the app passes the signed-in user.
fn names_identity(identity: &UserIdentity, input: &str) -> bool {
    let input = input.trim();
    identity.id == input
        || identity.login.as_deref().is_some_and(|login| login.eq_ignore_ascii_case(input))
}

pub(crate) async fn remove_reviewer(
    client: &Client,
    repo: &str,
    number: u64,
    reviewer_id: &str,
) -> Result<()> {
    client.require_token("Removing a reviewer")?;
    let key = RepoKey::parse(repo)?;
    client
        .send_unit::<()>(Method::DELETE, &reviewer_url(client, key, number, reviewer_id), None)
        .await
}
