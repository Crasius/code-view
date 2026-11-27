//! Change detection for an open merge request: the merge request itself (`sha`, `updated_at`,
//! `user_notes_count`, `blocking_discussions_resolved`), its approvals and its reviewers' states,
//! the same three small GETs the detail makes. The discussions, the expensive part of a reload,
//! are only read once something changed. The validator cache sends `If-None-Match` where GitLab
//! returns an `ETag`.

use review_core::{Fingerprint, PullRequestRevision, Result};

use crate::client::ApiClient;
use crate::dto::{ApprovalsDto, LabelRef, MergeRequestDto, ReviewerDto};
use crate::merge_requests::with_votes;

pub(crate) async fn revision(
    client: &ApiClient,
    repo: &str,
    iid: u64,
) -> Result<PullRequestRevision> {
    let (merge_request, reviewers, approvals) = with_votes(client, repo, iid).await?;
    Ok(of(&merge_request, &reviewers, &approvals))
}

fn of(
    merge_request: &MergeRequestDto,
    reviewers: &[ReviewerDto],
    approvals: &ApprovalsDto,
) -> PullRequestRevision {
    let mut fingerprint = Fingerprint::default();
    fingerprint
        .add(merge_request.updated_at)
        .add(&merge_request.title)
        .add(&merge_request.description)
        .add(&merge_request.state)
        .add(merge_request.draft || merge_request.work_in_progress)
        .add(merge_request.user_notes_count)
        .add(merge_request.blocking_discussions_resolved)
        .add(&merge_request.detailed_merge_status)
        .add(merge_request.diff_refs.as_ref().and_then(|refs| refs.base_sha.as_deref()));
    for label in &merge_request.labels {
        fingerprint.add(match label {
            LabelRef::Detail(label) => &label.name,
            LabelRef::Name(name) => name,
        });
    }
    for reviewer in reviewers {
        fingerprint.add(reviewer.user.id).add(&reviewer.state);
    }
    for approval in &approvals.approved_by {
        fingerprint.add(approval.user.id);
    }
    let head_sha = merge_request.head_sha().unwrap_or_default().to_owned();
    PullRequestRevision { head_sha, marker: fingerprint.finish() }
}
