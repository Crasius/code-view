//! Merge, auto-merge, close, reopen, draft and title/description edits.

use reqwest::Method;
use review_core::{MergeOptions, MergeStrategy, ProviderError, Result};
use serde_json::{Value, json};

use crate::client::{ApiClient, merge_request_path};
use crate::merge_requests::fetch;
use crate::merge_requests::merge_policy::{ProjectMerge, project_merge};

const DRAFT_PREFIX: &str = "Draft: ";
/// Title prefixes GitLab treats as draft markers (case-insensitive).
const DRAFT_MARKERS: &[&str] = &["draft:", "[draft]", "(draft)", "draft -"];

/// Reads the project's merge method first: GitLab merges with it whatever is asked, so a strategy
/// it would carry out differently is refused before anything is sent. The reviewed head goes as
/// `sha`; GitLab answers 409 when the source branch moved past it, or right after a push 405 while
/// it still checks whether the new head can merge (seen live). A 405 while the merge request's
/// head is no longer the reviewed one is the head-moved error too.
pub(crate) async fn merge(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    options: &MergeOptions,
) -> Result<()> {
    client.require_token("Merging")?;
    let project = project_merge(client, repo).await?;
    put_merge(client, repo, iid, options, merge_body(options, project)?).await
}

/// "Set to auto-merge": the same merge call with `auto_merge` (GitLab 17.11 and later) and its
/// deprecated older name `merge_when_pipeline_succeeds`, both documented, so older hosts take it
/// too. GitLab then merges once the checks pass, with the same guard and options.
pub(crate) async fn set_auto_merge(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    options: &MergeOptions,
) -> Result<()> {
    client.require_token("Setting auto-merge")?;
    let project = project_merge(client, repo).await?;
    let mut body = merge_body(options, project)?;
    body["auto_merge"] = Value::Bool(true);
    body["merge_when_pipeline_succeeds"] = Value::Bool(true);
    put_merge(client, repo, iid, options, body).await
}

/// "Cancel auto-merge": `cancel_auto_merge` (GitLab 19.5 and later), else the deprecated
/// `cancel_merge_when_pipeline_succeeds` that older hosts know (404 for the new path there).
pub(crate) async fn cancel_auto_merge(client: &ApiClient, repo: &str, iid: u64) -> Result<()> {
    client.require_token("Cancelling auto-merge")?;
    let path = merge_request_path(repo, iid);
    let current = client.request(Method::POST, &format!("{path}/cancel_auto_merge"));
    match client.unit(current).await {
        Err(ProviderError::Status { status: 404, .. }) => {
            let legacy = format!("{path}/cancel_merge_when_pipeline_succeeds");
            client.unit(client.request(Method::POST, &legacy)).await
        }
        result => result,
    }
}

async fn put_merge(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    options: &MergeOptions,
    body: Value,
) -> Result<()> {
    let request = client
        .request(Method::PUT, &format!("{}/merge", merge_request_path(repo, iid)))
        .json(&body);
    match client.unit(request).await {
        Err(ProviderError::Status { status: 409, .. }) => Err(ProviderError::HeadMoved),
        Err(error @ ProviderError::Status { status: 405, .. }) => {
            let head = fetch(client, repo, iid).await.map(|merge_request| merge_request.sha);
            let has_moved = head.is_ok_and(|sha| sha.is_some_and(|sha| sha != options.head_sha));
            Err(if has_moved { ProviderError::HeadMoved } else { error })
        }
        result => result,
    }
}

/// `project` is `None` when the connection cannot read the merge settings; then only a merge
/// commit or a squash is sent, as the project's method stays unknown.
fn merge_body(options: &MergeOptions, project: Option<ProjectMerge>) -> Result<Value> {
    match project {
        Some(project) => project.check(options.strategy)?,
        None if matches!(options.strategy, MergeStrategy::Rebase | MergeStrategy::SemiLinear) => {
            return Err(ProviderError::Unsupported(
                "GitLab merges with the project's merge method, which this connection cannot read; pick a merge commit or squash.".to_owned(),
            ));
        }
        None => {}
    }
    let squash = options.strategy == MergeStrategy::Squash;
    let mut body = json!({
        "sha": options.head_sha,
        "squash": squash,
        "should_remove_source_branch": options.delete_source_branch,
    });
    let message = options.commit_message.as_deref().filter(|message| !message.trim().is_empty());
    if let Some(message) = message
        && options.strategy.takes_message()
    {
        let field = if squash { "squash_commit_message" } else { "merge_commit_message" };
        body[field] = Value::String(message.to_owned());
    }
    Ok(body)
}

pub(crate) async fn close(client: &ApiClient, repo: &str, iid: u64) -> Result<()> {
    client.require_token("Closing")?;
    update(client, repo, iid, json!({ "state_event": "close" })).await
}

pub(crate) async fn reopen(client: &ApiClient, repo: &str, iid: u64) -> Result<()> {
    client.require_token("Reopening")?;
    update(client, repo, iid, json!({ "state_event": "reopen" })).await
}

pub(crate) async fn set_draft(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    is_draft: bool,
) -> Result<()> {
    client.require_token("Changing the draft state")?;
    let merge_request = fetch(client, repo, iid).await?;
    let title = draft_title(&merge_request.title, is_draft);
    if title == merge_request.title {
        return Ok(());
    }
    update(client, repo, iid, json!({ "title": title })).await
}

pub(crate) async fn update_title_description(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    title: &str,
    description: &str,
) -> Result<()> {
    client.require_token("Editing the title and description")?;
    update(client, repo, iid, json!({ "title": title, "description": description })).await
}

async fn update(client: &ApiClient, repo: &str, iid: u64, body: Value) -> Result<()> {
    client.unit(client.request(Method::PUT, &merge_request_path(repo, iid)).json(&body)).await
}

/// GitLab derives the draft flag from the title: `Draft: ` added, or every known marker removed.
pub(crate) fn draft_title(title: &str, is_draft: bool) -> String {
    let bare = strip_draft_markers(title);
    if is_draft { format!("{DRAFT_PREFIX}{bare}") } else { bare.to_owned() }
}

fn strip_draft_markers(title: &str) -> &str {
    let mut rest = title.trim_start();
    loop {
        let unmarked = DRAFT_MARKERS.iter().find_map(|marker| {
            let (head, tail) = rest.split_at_checked(marker.len())?;
            head.eq_ignore_ascii_case(marker).then_some(tail)
        });
        match unmarked {
            Some(tail) => rest = tail.trim_start(),
            None => return rest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_and_removes_draft_prefix() {
        assert_eq!(draft_title("Fix parser", true), "Draft: Fix parser");
        assert_eq!(draft_title("Draft: Fix parser", true), "Draft: Fix parser");
        assert_eq!(draft_title("draft: Fix parser", false), "Fix parser");
        assert_eq!(draft_title("[Draft] (draft) Fix", false), "Fix");
        assert_eq!(draft_title("Drafting notes", false), "Drafting notes");
    }

    #[test]
    fn builds_merge_bodies() {
        let squash = MergeOptions {
            strategy: MergeStrategy::Squash,
            delete_source_branch: true,
            commit_message: Some("msg".into()),
            head_sha: "abc".into(),
        };
        assert_eq!(
            merge_body(&squash, None).expect("squash"),
            json!({ "sha": "abc", "squash": true, "should_remove_source_branch": true, "squash_commit_message": "msg" })
        );
        let merge = MergeOptions {
            strategy: MergeStrategy::Merge,
            delete_source_branch: false,
            commit_message: None,
            head_sha: "abc".into(),
        };
        assert_eq!(
            merge_body(&merge, None).expect("merge"),
            json!({ "sha": "abc", "squash": false, "should_remove_source_branch": false })
        );
        let rebase = MergeOptions {
            strategy: MergeStrategy::Rebase,
            delete_source_branch: false,
            commit_message: Some("ignored".into()),
            head_sha: "abc".into(),
        };
        assert!(
            matches!(merge_body(&rebase, None), Err(ProviderError::Unsupported(_))),
            "unknown project method: no fast-forward promised"
        );
    }
}
