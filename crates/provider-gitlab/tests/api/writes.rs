//! Write paths against a mock GitLab REST API v4, matched on method, exact path (URL-encoded
//! project id), auth header and the body/query fields docs.gitlab.com requires.

use review_core::{
    DiffSide, FilePositions, LineRange, MergeOptions, MergeStrategy, NewThread, Provider,
    ProviderError, RangeDetail, ReviewVote, ThreadAnchor, ThreadStatus,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{self, REPO, TOKEN};

const MR: &str = "/api/v4/projects/group%2Fsub%2Fproject/merge_requests/7";

fn user(id: u64, username: &str) -> Value {
    json!({ "id": id, "username": username, "name": username })
}

fn merge_request(title: &str, reviewers: &Value) -> Value {
    json!({
        "id": 1, "iid": 7, "project_id": 42, "title": title, "description": "", "state": "opened",
        "author": user(1, "ann"), "source_branch": "feature", "target_branch": "main",
        "created_at": "2026-01-01T10:00:00Z", "web_url": "https://gitlab.example.com/group/sub/project/-/merge_requests/7",
        "reviewers": reviewers, "sha": "head1",
        "diff_refs": { "base_sha": "base1", "start_sha": "start1", "head_sha": "head1" }
    })
}

fn note(id: u64, body: &str) -> Value {
    json!({ "id": id, "body": body, "author": user(1, "ann"), "created_at": "2026-01-01T10:00:00Z",
            "system": false, "resolvable": true, "resolved": false })
}

async fn mount_merge_request(server: &MockServer, title: &str, reviewers: &Value) {
    Mock::given(method("GET"))
        .and(path(MR))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_json(merge_request(title, reviewers)))
        .mount(server)
        .await;
}

async fn expect_write(server: &MockServer, verb: &str, route: &str, body: Value, status: u16) {
    let mock = Mock::given(method(verb)).and(path(route)).and(header("PRIVATE-TOKEN", TOKEN));
    let mock = if body == json!({}) { mock } else { mock.and(body_partial_json(body)) };
    mock.respond_with(ResponseTemplate::new(status).set_body_json(json!({})))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn approves_with_comment_and_unapproves() {
    let server = MockServer::start().await;
    expect_write(&server, "POST", &format!("{MR}/approve"), json!({}), 201).await;
    expect_write(&server, "POST", &format!("{MR}/unapprove"), json!({}), 201).await;
    Mock::given(method("POST"))
        .and(path(format!("{MR}/notes")))
        .and(body_json(json!({ "body": "Looks good" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(note(5, "Looks good")))
        .expect(1)
        .mount(&server)
        .await;
    let provider = support::provider(&server, Some(TOKEN));
    provider.set_vote(REPO, 7, ReviewVote::Approved, Some(" Looks good ")).await.expect("approve");
    provider.set_vote(REPO, 7, ReviewVote::None, None).await.expect("unapprove");
    assert!(matches!(
        provider.set_vote(REPO, 7, ReviewVote::ChangesRequested, None).await,
        Err(ProviderError::Unsupported(_))
    ));
}

#[tokio::test]
async fn creates_general_discussion() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{MR}/discussions")))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .and(body_json(json!({ "body": "General remark" })))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({ "id": "d1", "notes": [note(10, "General remark")] })),
        )
        .expect(1)
        .mount(&server)
        .await;
    let thread = support::provider(&server, Some(TOKEN))
        .create_thread(REPO, 7, &NewThread { body: "General remark".to_owned(), anchor: None })
        .await
        .expect("general discussion");
    assert_eq!(thread.id, "d1");
    assert!(thread.anchor.is_none());
}

/// The position's SHAs come from the merge request's `diff_refs`; the unit tests of `discussions`
/// hold the line mapping of every side.
#[tokio::test]
async fn creates_line_discussion_with_the_merge_requests_diff_refs() {
    let server = MockServer::start().await;
    mount_merge_request(&server, "Title", &json!([])).await;
    let expected = json!({
        "body": "Inline",
        "position": {
            "position_type": "text", "base_sha": "base1", "start_sha": "start1", "head_sha": "head1",
            "new_path": "src/new.rs", "old_path": "src/old.rs", "new_line": 12,
        },
    });
    let response = json!({ "id": "d2", "notes": [{
        "id": 11, "body": "Inline", "author": user(1, "ann"), "created_at": "2026-01-01T10:00:00Z",
        "resolvable": true, "resolved": false,
        "position": { "head_sha": "head1", "old_path": "src/old.rs", "new_path": "src/new.rs",
                      "position_type": "text", "old_line": null, "new_line": 12 } }] });
    Mock::given(method("POST"))
        .and(path(format!("{MR}/discussions")))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .and(body_json(expected))
        .respond_with(ResponseTemplate::new(201).set_body_json(response))
        .expect(1)
        .mount(&server)
        .await;
    let anchor = ThreadAnchor {
        path: "src/new.rs".to_owned(),
        old_path: Some("src/old.rs".to_owned()),
        old_line: None,
        new_line: Some(12),
        range: None,
    };
    let thread = support::provider(&server, Some(TOKEN))
        .create_thread(
            REPO,
            7,
            &NewThread { body: "Inline".to_owned(), anchor: Some(anchor.clone()) },
        )
        .await
        .expect("line discussion");
    assert_eq!(thread.anchor, Some(anchor));
    assert!(!thread.is_outdated);
}

/// Multi-line note: `position[line_range]` with `line_code` (`<SHA-1 of the path>_<old>_<new>`) and
/// `type` per end, `position`'s own lines the last line (docs.gitlab.com, "Create a new thread in
/// the merge request diff").
#[tokio::test]
async fn creates_multi_line_discussion_with_line_range() {
    let server = MockServer::start().await;
    mount_merge_request(&server, "Title", &json!([])).await;
    // `printf src/new.rs | sha1sum`
    let hash = "0045bbd2766b7e4433e5b76b4802573be2c5621c";
    let expected = json!({
        "body": "Range",
        "position": {
            "position_type": "text", "base_sha": "base1", "start_sha": "start1", "head_sha": "head1",
            "new_path": "src/new.rs", "old_path": "src/old.rs", "new_line": 14,
            "line_range": {
                "start": { "line_code": format!("{hash}_8_8"), "type": "old", "old_line": 8 },
                "end": { "line_code": format!("{hash}_11_14"), "type": "new", "new_line": 14 },
            },
        },
    });
    let response = json!({ "id": "d3", "notes": [{
        "id": 13, "body": "Range", "author": user(1, "ann"), "created_at": "2026-01-01T10:00:00Z",
        "resolvable": true, "resolved": false,
        "position": { "head_sha": "head1", "old_path": "src/old.rs", "new_path": "src/new.rs", "position_type": "text",
                      "old_line": null, "new_line": 14,
                      "line_range": { "start": { "line_code": format!("{hash}_8_8"), "type": "old", "old_line": 8, "new_line": null },
                                      "end": { "line_code": format!("{hash}_11_14"), "type": "new", "old_line": null, "new_line": 14 } } } }] });
    Mock::given(method("POST"))
        .and(path(format!("{MR}/discussions")))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .and(body_json(expected))
        .respond_with(ResponseTemplate::new(201).set_body_json(response))
        .expect(1)
        .mount(&server)
        .await;
    // From the deleted base line 8 (removed before head line 8) down to the added head line 14.
    let anchor = ThreadAnchor {
        path: "src/new.rs".to_owned(),
        old_path: Some("src/old.rs".to_owned()),
        old_line: None,
        new_line: Some(14),
        range: Some(LineRange {
            start_old_line: Some(8),
            start_new_line: None,
            side: DiffSide::New,
            detail: Some(RangeDetail {
                start: FilePositions { old: 8, new: 8 },
                end: FilePositions { old: 11, new: 14 },
                start_offset: 1,
                end_offset: 20,
            }),
        }),
    };
    support::provider(&server, Some(TOKEN))
        .create_thread(REPO, 7, &NewThread { body: "Range".to_owned(), anchor: Some(anchor) })
        .await
        .expect("multi-line discussion");
}

#[tokio::test]
async fn replies_to_discussion() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{MR}/discussions/abc123/notes")))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .and(body_json(json!({ "body": "Thanks" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(note(12, "Thanks")))
        .expect(1)
        .mount(&server)
        .await;
    let comment = support::provider(&server, Some(TOKEN))
        .reply(REPO, 7, "abc123", "Thanks")
        .await
        .expect("reply");
    assert_eq!(comment.id, "12");
}

#[tokio::test]
async fn resolves_and_unresolves_discussion() {
    let server = MockServer::start().await;
    for resolved in ["true", "false"] {
        Mock::given(method("PUT"))
            .and(path(format!("{MR}/discussions/abc123")))
            .and(header("PRIVATE-TOKEN", TOKEN))
            .and(query_param("resolved", resolved))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "id": "abc123", "notes": [] })),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let provider = support::provider(&server, Some(TOKEN));
    provider.set_thread_status(REPO, 7, "abc123", ThreadStatus::Resolved).await.expect("resolve");
    provider.set_thread_status(REPO, 7, "abc123", ThreadStatus::Active).await.expect("unresolve");
}

#[tokio::test]
async fn replaces_labels() {
    let server = MockServer::start().await;
    expect_write(&server, "PUT", MR, json!({ "labels": "bug,type::feature" }), 200).await;
    expect_write(&server, "PUT", MR, json!({ "labels": "" }), 200).await;
    let provider = support::provider(&server, Some(TOKEN));
    provider
        .set_labels(REPO, 7, &["bug".to_owned(), "type::feature".to_owned()])
        .await
        .expect("set labels");
    provider.set_labels(REPO, 7, &[]).await.expect("clear labels");
}

#[tokio::test]
async fn adds_reviewer_by_username() {
    let server = MockServer::start().await;
    mount_merge_request(&server, "Title", &json!([user(2, "bob")])).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users"))
        .and(query_param("username", "cy"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([user(3, "cy")])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(MR))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .and(body_json(json!({ "reviewer_ids": [2, 3] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    support::provider(&server, Some(TOKEN))
        .add_reviewer(REPO, 7, "@cy")
        .await
        .expect("add reviewer");
}

#[tokio::test]
async fn removes_reviewer_by_id() {
    let server = MockServer::start().await;
    mount_merge_request(&server, "Title", &json!([user(2, "bob"), user(3, "cy")])).await;
    Mock::given(method("PUT"))
        .and(path(MR))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .and(body_json(json!({ "reviewer_ids": [2] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    support::provider(&server, Some(TOKEN))
        .remove_reviewer(REPO, 7, "3")
        .await
        .expect("remove reviewer");
}

#[tokio::test]
async fn merges_and_squashes() {
    let server = MockServer::start().await;
    let settings = json!({ "merge_method": "merge", "squash_option": "default_off" });
    support::mount_project(&server, support::project(settings)).await;
    let route = format!("{MR}/merge");
    expect_write(
        &server,
        "PUT",
        &route,
        json!({ "sha": "reviewed1", "squash": false, "should_remove_source_branch": true, "merge_commit_message": "Merge it" }),
        200,
    )
    .await;
    expect_write(
        &server,
        "PUT",
        &route,
        json!({ "sha": "reviewed1", "squash": true, "should_remove_source_branch": false, "squash_commit_message": "Squash it" }),
        200,
    )
    .await;
    let provider = support::provider(&server, Some(TOKEN));
    let merge = MergeOptions {
        strategy: MergeStrategy::Merge,
        delete_source_branch: true,
        commit_message: Some("Merge it".into()),
        head_sha: "reviewed1".into(),
    };
    provider.merge(REPO, 7, &merge).await.expect("merge");
    let squash = MergeOptions {
        strategy: MergeStrategy::Squash,
        delete_source_branch: false,
        commit_message: Some("Squash it".into()),
        head_sha: "reviewed1".into(),
    };
    provider.merge(REPO, 7, &squash).await.expect("squash");
}

/// The source branch moved past the reviewed `sha`: GitLab's 409 is the head-moved error.
#[tokio::test]
async fn merge_refused_for_a_moved_head_reports_new_commits() {
    let server = MockServer::start().await;
    let settings = json!({ "merge_method": "merge", "squash_option": "default_off" });
    support::mount_project(&server, support::project(settings)).await;
    Mock::given(method("PUT"))
        .and(path(format!("{MR}/merge")))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .and(body_partial_json(json!({ "sha": "reviewed1" })))
        .respond_with(
            ResponseTemplate::new(409)
                .set_body_json(json!({ "message": "SHA does not match HEAD of source branch" })),
        )
        .expect(1)
        .mount(&server)
        .await;
    let merge = MergeOptions {
        strategy: MergeStrategy::Merge,
        delete_source_branch: false,
        commit_message: None,
        head_sha: "reviewed1".into(),
    };
    let error = support::provider(&server, Some(TOKEN))
        .merge(REPO, 7, &merge)
        .await
        .expect_err("head moved");
    assert!(matches!(error, ProviderError::HeadMoved), "{error:?}");
}

/// Right after a push GitLab answers 405 while it checks whether the new head can merge: with the
/// head past the reviewed `sha` that is the head-moved error; at the reviewed head GitLab's own
/// refusal stays.
#[tokio::test]
async fn merge_refused_while_a_push_is_checked_reports_new_commits() {
    let server = MockServer::start().await;
    let settings = json!({ "merge_method": "merge", "squash_option": "default_off" });
    support::mount_project(&server, support::project(settings)).await;
    mount_merge_request(&server, "Title", &json!([])).await;
    Mock::given(method("PUT"))
        .and(path(format!("{MR}/merge")))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .respond_with(
            ResponseTemplate::new(405)
                .set_body_json(json!({ "message": "405 Method Not Allowed" })),
        )
        .expect(2)
        .mount(&server)
        .await;
    let mut merge = MergeOptions {
        strategy: MergeStrategy::Merge,
        delete_source_branch: false,
        commit_message: None,
        head_sha: "reviewed1".into(),
    };
    let provider = support::provider(&server, Some(TOKEN));
    let error = provider.merge(REPO, 7, &merge).await.expect_err("head moved");
    assert!(matches!(error, ProviderError::HeadMoved), "{error:?}");

    merge.head_sha = "head1".into();
    let error = provider.merge(REPO, 7, &merge).await.expect_err("not mergeable");
    assert_eq!(error.status(), Some(405));
}

#[tokio::test]
async fn closes_and_reopens() {
    let server = MockServer::start().await;
    expect_write(&server, "PUT", MR, json!({ "state_event": "close" }), 200).await;
    expect_write(&server, "PUT", MR, json!({ "state_event": "reopen" }), 200).await;
    let provider = support::provider(&server, Some(TOKEN));
    provider.close(REPO, 7).await.expect("close");
    provider.reopen(REPO, 7).await.expect("reopen");
}

#[tokio::test]
async fn sets_draft_by_title_prefix_and_skips_a_title_that_is_right() {
    let cases = [
        ("Fix parser", true, Some("Draft: Fix parser")),
        ("Draft: Fix parser", false, Some("Fix parser")),
        ("Draft: Fix parser", true, None),
    ];
    for (title, is_draft, written) in cases {
        let server = MockServer::start().await;
        mount_merge_request(&server, title, &json!([])).await;
        match written {
            Some(written) => {
                expect_write(&server, "PUT", MR, json!({ "title": written }), 200).await;
            }
            None => {
                Mock::given(method("PUT"))
                    .and(path(MR))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
                    .expect(0)
                    .mount(&server)
                    .await;
            }
        }
        support::provider(&server, Some(TOKEN))
            .set_draft(REPO, 7, is_draft)
            .await
            .expect("set draft");
    }
}

#[tokio::test]
async fn updates_title_and_description() {
    let server = MockServer::start().await;
    expect_write(
        &server,
        "PUT",
        MR,
        json!({ "title": "New title", "description": "New *body*" }),
        200,
    )
    .await;
    support::provider(&server, Some(TOKEN))
        .update_title_description(REPO, 7, "New title", "New *body*")
        .await
        .expect("update");
}

#[tokio::test]
async fn refuses_writes_without_token() {
    let server = MockServer::start().await;
    let provider = support::provider(&server, None);
    assert!(matches!(provider.close(REPO, 7).await, Err(ProviderError::Unsupported(_))));
    assert!(matches!(
        provider.set_vote(REPO, 7, ReviewVote::Approved, None).await,
        Err(ProviderError::Unsupported(_))
    ));
    assert!(
        server.received_requests().await.expect("recorded").is_empty(),
        "no request without a token"
    );
}
