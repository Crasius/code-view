//! Merge request discussions: listing, new threads with diff positions, replies, resolving, and
//! editing and deleting notes.

use reqwest::Method;
use review_core::provider::CommentActions;
use review_core::{
    DiffSide, FilePositions, LineRange, NewThread, ProviderError, Result, ReviewThread,
    ThreadAnchor, ThreadComment, ThreadOrigin, ThreadStatus,
};
use serde_json::{Value, json};

use crate::client::{ApiClient, merge_request_path};
use crate::dto::{DiffRefsDto, DiscussionDto, NoteDto, PositionDto};
use crate::merge_requests::fetch;

const MAX_DISCUSSIONS: usize = 10_000;

pub(crate) async fn threads(client: &ApiClient, repo: &str, iid: u64) -> Result<Vec<ReviewThread>> {
    let path = discussions_path(repo, iid);
    let (merge_request, discussions) = tokio::try_join!(
        fetch(client, repo, iid),
        client.get_pages::<DiscussionDto>(&path, &[], MAX_DISCUSSIONS)
    )?;
    let head_sha = merge_request.head_sha();
    Ok(discussions.into_iter().filter_map(|discussion| to_thread(discussion, head_sha)).collect())
}

pub(crate) async fn create_thread(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    thread: &NewThread,
) -> Result<ReviewThread> {
    client.require_token("Commenting")?;
    let mut body = json!({ "body": thread.body });
    let mut head_sha = None;
    if let Some(anchor) = &thread.anchor {
        let merge_request = fetch(client, repo, iid).await?;
        let diff_refs = merge_request.diff_refs.ok_or_else(|| {
            ProviderError::Decode(format!("!{iid} has no diff refs yet; reload in a moment."))
        })?;
        body["position"] = text_position(anchor, &diff_refs)?;
        head_sha = diff_refs.head_sha;
    }
    let request = client.request(Method::POST, &discussions_path(repo, iid)).json(&body);
    let discussion: DiscussionDto = client.json(request).await?;
    to_thread(discussion, head_sha.as_deref()).ok_or_else(|| {
        ProviderError::Decode("GitLab returned a discussion without notes.".to_owned())
    })
}

pub(crate) async fn reply(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    thread_id: &str,
    body: &str,
) -> Result<ThreadComment> {
    client.require_token("Replying")?;
    let path = format!("{}/notes", discussion_path(repo, iid, thread_id));
    let note: NoteDto =
        client.json(client.request(Method::POST, &path).json(&json!({ "body": body }))).await?;
    Ok(to_comment(&note))
}

pub(crate) async fn set_status(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    thread_id: &str,
    status: ThreadStatus,
) -> Result<()> {
    client.require_token("Resolving threads")?;
    let resolved = matches!(status, ThreadStatus::Resolved).to_string();
    let request = client
        .request(Method::PUT, &discussion_path(repo, iid, thread_id))
        .query(&[("resolved", resolved)]);
    client.unit(request).await
}

/// System notes are GitLab's own and cannot be edited or deleted; every other note can.
pub(crate) fn comment_actions(thread: &ReviewThread) -> CommentActions {
    if thread.is_system { CommentActions::NONE } else { CommentActions::EDIT_AND_DELETE }
}

/// The anchor of the discussion's first note on the merge request's page.
pub(crate) fn page_suffix(thread: &ReviewThread) -> Option<String> {
    thread.comments.first().map(|note| format!("#note_{}", note.id))
}

pub(crate) async fn edit_note(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    thread_id: &str,
    note_id: &str,
    body: &str,
) -> Result<ThreadComment> {
    client.require_token("Editing comments")?;
    // The endpoint takes either `body` or `resolved`, never both.
    let request = client
        .request(Method::PUT, &note_path(repo, iid, thread_id, note_id))
        .json(&json!({ "body": body }));
    let note: NoteDto = client.json(request).await?;
    Ok(to_comment(&note))
}

pub(crate) async fn delete_note(
    client: &ApiClient,
    repo: &str,
    iid: u64,
    thread_id: &str,
    note_id: &str,
) -> Result<()> {
    client.require_token("Deleting comments")?;
    client.unit(client.request(Method::DELETE, &note_path(repo, iid, thread_id, note_id))).await
}

fn note_path(repo: &str, iid: u64, thread_id: &str, note_id: &str) -> String {
    format!("{}/notes/{}", discussion_path(repo, iid, thread_id), urlencoding::encode(note_id))
}

fn discussions_path(repo: &str, iid: u64) -> String {
    format!("{}/discussions", merge_request_path(repo, iid))
}

fn discussion_path(repo: &str, iid: u64, thread_id: &str) -> String {
    format!("{}/{}", discussions_path(repo, iid), urlencoding::encode(thread_id))
}

/// `position` JSON for a text diff note. Line numbers go exactly as the anchor says:
/// added → `new_line`, deleted → `old_line`, unchanged context → both.
pub(crate) fn text_position(anchor: &ThreadAnchor, diff_refs: &DiffRefsDto) -> Result<Value> {
    let (Some(base_sha), Some(start_sha), Some(head_sha)) =
        (&diff_refs.base_sha, &diff_refs.start_sha, &diff_refs.head_sha)
    else {
        return Err(ProviderError::Decode(
            "The merge request's diff refs are incomplete.".to_owned(),
        ));
    };
    if anchor.old_line.is_none() && anchor.new_line.is_none() {
        return Err(ProviderError::Local(
            "An inline comment needs an old or a new line number.".to_owned(),
        ));
    }
    let mut position = json!({
        "position_type": "text",
        "base_sha": base_sha,
        "start_sha": start_sha,
        "head_sha": head_sha,
        "new_path": anchor.path,
        "old_path": anchor.old_path.as_deref().unwrap_or(&anchor.path),
    });
    if let Some(line) = anchor.new_line {
        position["new_line"] = json!(line);
    }
    if let Some(line) = anchor.old_line {
        position["old_line"] = json!(line);
    }
    if let Some(range) = &anchor.range {
        position["line_range"] = line_range(anchor, range)?;
    }
    Ok(position)
}

/// `position[line_range]` of a multi-line note; `position`'s own lines are its last line.
/// Each end names its line by a line code, `<SHA-1 of the file path>_<old>_<new>` with the
/// line's position in both files, and by `type`: `new` for an added line, `old` otherwise.
fn line_range(anchor: &ThreadAnchor, range: &LineRange) -> Result<Value> {
    let detail = range.detail.ok_or_else(|| {
        ProviderError::Local(
            "A multi-line GitLab comment needs the line positions from the diff.".to_owned(),
        )
    })?;
    let path_hash = sha1_smol::Sha1::from(anchor.path.as_bytes()).digest().to_string();
    let point = |old_line: Option<u32>, new_line: Option<u32>, positions: FilePositions| {
        let is_added = old_line.is_none() && new_line.is_some();
        let mut point = json!({
            "line_code": format!("{path_hash}_{}_{}", positions.old, positions.new),
            "type": if is_added { "new" } else { "old" },
        });
        if let Some(line) = old_line {
            point["old_line"] = json!(line);
        }
        if let Some(line) = new_line {
            point["new_line"] = json!(line);
        }
        point
    };
    Ok(json!({
        "start": point(range.start_old_line, range.start_new_line, detail.start),
        "end": point(anchor.old_line, anchor.new_line, detail.end),
    }))
}

/// A note's `line_range` as a [`LineRange`] when it spans more than one line. GitLab has no
/// panes, so the range counts in the base only when its last line is a deleted one.
fn range_from(position: &PositionDto) -> Option<LineRange> {
    let range = position.line_range.as_ref()?;
    let (start, end) = (&range.start, &range.end);
    if (start.old_line, start.new_line) == (end.old_line, end.new_line) {
        return None;
    }
    let ends_on_deleted = end.kind.as_deref() == Some("old") && end.new_line.is_none();
    Some(LineRange {
        start_old_line: start.old_line,
        start_new_line: start.new_line,
        side: if ends_on_deleted { DiffSide::Old } else { DiffSide::New },
        detail: None,
    })
}

/// Maps one discussion; `None` when it has no notes.
///
/// Outdated detection: GitLab re-maps a diff note's `position` onto every new push while the
/// commented line survives; a position still pointing at an older `head_sha` therefore means the
/// line changed. Right after a push the re-mapping runs asynchronously, so a thread can look
/// outdated for a few seconds.
pub(crate) fn to_thread(
    discussion: DiscussionDto,
    mr_head_sha: Option<&str>,
) -> Option<ReviewThread> {
    let first = discussion.notes.first()?;
    let is_system = first.system;
    let position = discussion.notes.iter().find_map(|note| note.position.as_ref());
    let anchor = position
        .filter(|position| position.position_type.as_deref().unwrap_or("text") == "text")
        .and_then(|position| {
            let new_path = position.new_path.clone().or_else(|| position.old_path.clone())?;
            let old_path = position.old_path.clone().filter(|old_path| *old_path != new_path);
            Some(ThreadAnchor {
                path: new_path,
                old_path,
                old_line: position.old_line,
                new_line: position.new_line,
                range: range_from(position),
            })
        });
    let is_outdated =
        match (position.and_then(|position| position.head_sha.as_deref()), mr_head_sha) {
            (Some(note_head), Some(mr_head)) => note_head != mr_head,
            _ => false,
        };
    // An outdated note's position stays on the last diff its line was in, which is the version to show.
    let origin =
        position.zip(anchor.clone()).filter(|_| is_outdated).and_then(|(position, anchor)| {
            Some(ThreadOrigin {
                base_sha: position.base_sha.clone(),
                head_sha: position.head_sha.clone()?,
                anchor,
                diff_hunk: None,
                web_url: None,
            })
        });
    let resolvable: Vec<&NoteDto> =
        discussion.notes.iter().filter(|note| note.resolvable).collect();
    let is_resolved = !resolvable.is_empty() && resolvable.iter().all(|note| note.resolved);
    Some(ReviewThread {
        id: discussion.id,
        status: if is_resolved { ThreadStatus::Resolved } else { ThreadStatus::Active },
        anchor,
        is_outdated,
        origin,
        is_system,
        can_reply: !is_system,
        can_resolve: !is_system && !resolvable.is_empty(),
        comments: discussion.notes.iter().map(to_comment).collect(),
    })
}

pub(crate) fn to_comment(note: &NoteDto) -> ThreadComment {
    ThreadComment {
        id: note.id.to_string(),
        author: note.author.to_identity(),
        body: note.body.clone(),
        created_at: note.created_at,
        updated_at: note.updated_at.filter(|updated_at| *updated_at != note.created_at),
    }
}

#[cfg(test)]
#[path = "discussions_tests.rs"]
mod tests;
