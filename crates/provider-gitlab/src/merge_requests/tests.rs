use review_core::ReviewVote;

use super::*;

const MERGE_REQUEST: &str = r###"{
  "id": 901, "iid": 4131, "project_id": 42, "source_project_id": 42,
  "title": "Draft: docs: regenerate", "description": "## Summary", "state": "opened",
  "draft": true, "work_in_progress": true,
  "author": {"id": 7, "username": "ann", "name": "Ann", "avatar_url": "https://a/x.png"},
  "source_branch": "duo/fix", "target_branch": "main",
  "created_at": "2026-10-16T01:53:46.784Z", "updated_at": "2026-10-16T02:08:48.051Z",
  "merged_at": null, "closed_at": null,
  "web_url": "https://gitlab.example.com/acme/cli/-/merge_requests/4131",
  "labels": [{"id": 1, "name": "type::maintenance", "color": "#330066", "description": "Upkeep"}],
  "reviewers": [{"id": 8, "username": "bob", "name": "Bob"}],
  "references": {"short": "!4131", "full": "acme/cli!4131"},
  "sha": "7622f050",
  "diff_refs": {"base_sha": "af362f57", "head_sha": "7622f050", "start_sha": "af362f58"},
  "detailed_merge_status": "not_approved", "merge_status": "can_be_merged", "has_conflicts": false
}"###;

fn merge_request() -> MergeRequestDto {
    serde_json::from_str(MERGE_REQUEST).expect("fixture")
}

#[test]
fn maps_summary() {
    let summary = to_summary(merge_request(), "acme/cli".to_owned());
    assert_eq!(summary.repository_name, "cli");
    assert_eq!(summary.number, 4131);
    assert_eq!(summary.state, PullRequestState::Open);
    assert!(summary.is_draft);
    assert_eq!(summary.author.login.as_deref(), Some("ann"));
    assert_eq!(summary.author.display_name, "Ann");
    assert_eq!(summary.labels[0].color.as_deref(), Some("330066"));
    assert_eq!(summary.reviewers.len(), 1);
    assert_eq!(summary.reviewers[0].vote, ReviewVote::None);
}

#[test]
fn decodes_merge_request_states() {
    assert_eq!(pull_request_state("merged"), PullRequestState::Merged);
    assert_eq!(pull_request_state("closed"), PullRequestState::Closed);
    assert_eq!(pull_request_state("opened"), PullRequestState::Open);
    assert_eq!(pull_request_state("locked"), PullRequestState::Open);
}

#[test]
fn maps_detail_with_refs_and_votes() {
    let reviewers: Vec<ReviewerDto> =
        serde_json::from_str(r#"[{"user":{"id":8,"username":"bob"},"state":"approved"}]"#)
            .expect("fixture");
    let detail =
        to_detail(merge_request(), "acme/cli".to_owned(), &reviewers, &ApprovalsDto::default())
            .expect("detail");
    assert_eq!(detail.target_sha, "af362f57");
    assert_eq!(detail.head_sha, "7622f050");
    assert_eq!(detail.description, "## Summary");
    assert_eq!(detail.merge_status, MergeStatus::Blocked);
    assert_eq!(
        detail.git_fetch_refs,
        vec!["refs/merge-requests/4131/head".to_owned(), "refs/heads/main".to_owned()]
    );
    assert_eq!(detail.summary.reviewers[0].vote, ReviewVote::Approved);
    assert_eq!(detail.closed_at, None);
}

#[test]
fn rejects_detail_without_diff_refs() {
    let mut dto = merge_request();
    dto.diff_refs = None;
    dto.sha = None;
    assert!(matches!(
        to_detail(dto, "acme/cli".to_owned(), &[], &ApprovalsDto::default()),
        Err(ProviderError::Decode(_))
    ));
}

#[test]
fn derives_repository_key() {
    let web_root = "https://gitlab.example.com";
    let mut dto = merge_request();
    assert_eq!(repository_key(&dto, web_root), "acme/cli");
    dto.references = None;
    assert_eq!(repository_key(&dto, web_root), "acme/cli");
    dto.web_url = "https://elsewhere/x".to_owned();
    assert_eq!(repository_key(&dto, web_root), "42");
}

#[test]
fn maps_merge_statuses() {
    let cases = [
        (Some("mergeable"), None, MergeStatus::Mergeable),
        (Some("conflict"), None, MergeStatus::Conflicts),
        (Some("broken_status"), None, MergeStatus::Conflicts),
        (Some("not_approved"), None, MergeStatus::Blocked),
        (Some("discussions_not_resolved"), None, MergeStatus::Blocked),
        (Some("ci_must_pass"), None, MergeStatus::Blocked),
        (Some("draft_status"), None, MergeStatus::Blocked),
        (Some("need_rebase"), None, MergeStatus::Blocked),
        (Some("checking"), None, MergeStatus::Unknown),
        (Some("unchecked"), None, MergeStatus::Unknown),
        (None, Some("can_be_merged"), MergeStatus::Mergeable),
        (None, Some("cannot_be_merged"), MergeStatus::Conflicts),
        (None, None, MergeStatus::Unknown),
    ];
    for (detailed, legacy, expected) in cases {
        assert_eq!(merge_status(detailed, legacy, false), expected, "{detailed:?} {legacy:?}");
    }
    assert_eq!(merge_status(Some("mergeable"), None, true), MergeStatus::Conflicts);
}

#[test]
fn maps_states_and_plain_label_names() {
    let mut dto = merge_request();
    dto.state = "merged".to_owned();
    dto.draft = false;
    dto.work_in_progress = false;
    dto.labels = serde_json::from_str(r#"["bug"]"#).expect("fixture");
    let summary = to_summary(dto, "a/b".to_owned());
    assert_eq!(summary.state, PullRequestState::Merged);
    assert!(!summary.is_draft);
    assert_eq!(summary.labels[0].name, "bug");
}

#[test]
fn maps_who_set_auto_merge_and_its_squash() {
    let detail = |extra: serde_json::Value| {
        let mut value: serde_json::Value = serde_json::from_str(MERGE_REQUEST).expect("fixture");
        value.as_object_mut().expect("object").extend(extra.as_object().expect("object").clone());
        let dto: MergeRequestDto = serde_json::from_value(value).expect("fixture");
        to_detail(dto, "acme/cli".to_owned(), &[], &ApprovalsDto::default()).expect("detail")
    };
    let user = serde_json::json!({"id": 5, "username": "ada", "name": "Ada"});
    let squashed = detail(serde_json::json!({
        "merge_when_pipeline_succeeds": true, "merge_user": user, "squash_on_merge": true
    }))
    .auto_complete
    .expect("auto-merge is set");
    assert_eq!(squashed.strategy, Some(MergeStrategy::Squash));
    assert_eq!(squashed.set_by.and_then(|user| user.login).as_deref(), Some("ada"));
    let legacy =
        detail(serde_json::json!({ "merge_when_pipeline_succeeds": true, "merged_by": user }))
            .auto_complete
            .expect("auto-merge is set");
    assert_eq!(legacy.strategy, None, "the project's merge method decides");
    assert_eq!(legacy.set_by.map(|user| user.display_name).as_deref(), Some("Ada"));
    assert_eq!(detail(serde_json::json!({ "merge_user": user })).auto_complete, None);
}
