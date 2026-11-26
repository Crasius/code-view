//! Write paths against a mock GitLab REST API v4, matched on method, exact path (URL-encoded
//! project id), auth header and the body/query fields docs.gitlab.com requires.

use provider_gitlab::GitLabProvider;
use review_core::{
    Connection, DiffSide, FilePositions, LineRange, MergeOptions, MergeStrategy, NewThread,
    Provider, ProviderError, ProviderKind, RangeDetail, ReviewVote, ThreadAnchor, ThreadStatus,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "group/sub/project";
const MR: &str = "/api/v4/projects/group%2Fsub%2Fproject/merge_requests/7";
const TOKEN: &str = "test-token";

fn provider(server: &MockServer) -> GitLabProvider {
    let connection = Connection {
        id: "mock".to_owned(),
        name: "mock".to_owned(),
        kind: ProviderKind::GitLab,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: Some(TOKEN.to_owned()),
    };
    GitLabProvider::new(connection, reqwest::Client::new())
}

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
    let provider = provider(&server);
    provider.set_vote(REPO, 7, ReviewVote::Approved, Some(" Looks good ")).await.expect("approve");
    provider.set_vote(REPO, 7, ReviewVote::None, None).await.expect("unapprove");
    assert!(provider.set_vote(REPO, 7, ReviewVote::ChangesRequested, None).await.is_err());
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
    let thread = provider(&server)
        .create_thread(REPO, 7, &NewThread { body: "General remark".to_owned(), anchor: None })
        .await
        .expect("general discussion");
    assert_eq!(thread.id, "d1");
    assert!(thread.anchor.is_none());
}

#[tokio::test]
async fn creates_line_discussions_with_documented_positions() {
    let server = MockServer::start().await;
    mount_merge_request(&server, "Title", &json!([])).await;
    let refs = json!({ "position_type": "text", "base_sha": "base1", "start_sha": "start1", "head_sha": "head1" });
    let cases = [
        // Added line: new_line only.
        (
            None,
            Some(12),
            json!({ "new_path": "src/new.rs", "old_path": "src/old.rs", "new_line": 12 }),
        ),
        // Deleted line: old_line only.
        (
            Some(8),
            None,
            json!({ "new_path": "src/new.rs", "old_path": "src/old.rs", "old_line": 8 }),
        ),
        // Unchanged line: both.
        (
            Some(3),
            Some(4),
            json!({ "new_path": "src/new.rs", "old_path": "src/old.rs", "old_line": 3, "new_line": 4 }),
        ),
    ];
    let provider = provider(&server);
    for (old_line, new_line, lines) in cases {
        let mut position = refs.clone();
        position
            .as_object_mut()
            .expect("object")
            .extend(lines.as_object().expect("object").clone());
        let position_matches = move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
            body.get("position") == Some(&position) && body.get("body") == Some(&json!("Inline"))
        };
        let response = json!({ "id": "d2", "notes": [{
            "id": 11, "body": "Inline", "author": user(1, "ann"), "created_at": "2026-01-01T10:00:00Z",
            "resolvable": true, "resolved": false,
            "position": { "head_sha": "head1", "old_path": "src/old.rs", "new_path": "src/new.rs",
                          "position_type": "text", "old_line": old_line, "new_line": new_line } }] });
        let mock = Mock::given(method("POST"))
            .and(path(format!("{MR}/discussions")))
            .and(header("PRIVATE-TOKEN", TOKEN))
            .and(position_matches)
            .respond_with(ResponseTemplate::new(201).set_body_json(response))
            .expect(1)
            .mount_as_scoped(&server)
            .await;
        let anchor = ThreadAnchor {
            path: "src/new.rs".to_owned(),
            old_path: Some("src/old.rs".to_owned()),
            old_line,
            new_line,
            range: None,
        };
        let thread = provider
            .create_thread(
                REPO,
                7,
                &NewThread { body: "Inline".to_owned(), anchor: Some(anchor.clone()) },
            )
            .await
            .expect("line discussion");
        assert_eq!(thread.anchor, Some(anchor));
        assert!(!thread.is_outdated);
        drop(mock);
    }
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
    let thread = provider(&server)
        .create_thread(REPO, 7, &NewThread { body: "Range".to_owned(), anchor: Some(anchor) })
        .await
        .expect("multi-line discussion");
    let anchor = thread.anchor.expect("anchor");
    assert_eq!(anchor.new_line, Some(14));
    assert_eq!(
        anchor.range,
        Some(LineRange {
            start_old_line: Some(8),
            start_new_line: None,
            side: DiffSide::New,
            detail: None
        })
    );
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
    let comment = provider(&server).reply(REPO, 7, "abc123", "Thanks").await.expect("reply");
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
    let provider = provider(&server);
    provider.set_thread_status(REPO, 7, "abc123", ThreadStatus::Resolved).await.expect("resolve");
    provider.set_thread_status(REPO, 7, "abc123", ThreadStatus::Active).await.expect("unresolve");
}

#[tokio::test]
async fn replaces_labels() {
    let server = MockServer::start().await;
    expect_write(&server, "PUT", MR, json!({ "labels": "bug,type::feature" }), 200).await;
    expect_write(&server, "PUT", MR, json!({ "labels": "" }), 200).await;
    let provider = provider(&server);
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
    provider(&server).add_reviewer(REPO, 7, "@cy").await.expect("add reviewer");
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
    provider(&server).remove_reviewer(REPO, 7, "3").await.expect("remove reviewer");
}

#[tokio::test]
async fn merges_and_squashes() {
    let server = MockServer::start().await;
    // The project's merge method is read first: a merge-commit project with optional squashing.
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/group%2Fsub%2Fproject"))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "path_with_namespace": REPO, "name": "project", "web_url": "https://gitlab.example.com/group/sub/project",
            "http_url_to_repo": "https://gitlab.example.com/group/sub/project.git", "merge_method": "merge", "squash_option": "default_off"
        })))
        .mount(&server)
        .await;
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
    let provider = provider(&server);
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
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/group%2Fsub%2Fproject"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "path_with_namespace": REPO, "name": "project", "web_url": "https://gitlab.example.com/group/sub/project",
            "http_url_to_repo": "https://gitlab.example.com/group/sub/project.git", "merge_method": "merge", "squash_option": "default_off"
        })))
        .mount(&server)
        .await;
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
    let error = provider(&server).merge(REPO, 7, &merge).await.expect_err("head moved");
    assert!(matches!(error, ProviderError::HeadMoved), "{error:?}");
}

/// Right after a push GitLab answers 405 while it checks whether the new head can merge: with the
/// head past the reviewed `sha` that is the head-moved error; at the reviewed head GitLab's own
/// refusal stays.
#[tokio::test]
async fn merge_refused_while_a_push_is_checked_reports_new_commits() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/group%2Fsub%2Fproject"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "path_with_namespace": REPO, "name": "project", "web_url": "https://gitlab.example.com/group/sub/project",
            "http_url_to_repo": "https://gitlab.example.com/group/sub/project.git", "merge_method": "merge", "squash_option": "default_off"
        })))
        .mount(&server)
        .await;
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
    let provider = provider(&server);
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
    let provider = provider(&server);
    provider.close(REPO, 7).await.expect("close");
    provider.reopen(REPO, 7).await.expect("reopen");
}

#[tokio::test]
async fn marks_draft_by_title_prefix() {
    let server = MockServer::start().await;
    mount_merge_request(&server, "Fix parser", &json!([])).await;
    expect_write(&server, "PUT", MR, json!({ "title": "Draft: Fix parser" }), 200).await;
    provider(&server).set_draft(REPO, 7, true).await.expect("set draft");
}

#[tokio::test]
async fn removes_draft_prefix() {
    let server = MockServer::start().await;
    mount_merge_request(&server, "Draft: Fix parser", &json!([])).await;
    expect_write(&server, "PUT", MR, json!({ "title": "Fix parser" }), 200).await;
    provider(&server).set_draft(REPO, 7, false).await.expect("unset draft");
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
    provider(&server)
        .update_title_description(REPO, 7, "New title", "New *body*")
        .await
        .expect("update");
}

#[tokio::test]
async fn refuses_writes_without_token() {
    let server = MockServer::start().await;
    let connection = Connection {
        id: "anon".to_owned(),
        name: "anon".to_owned(),
        kind: ProviderKind::GitLab,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: None,
    };
    let provider = GitLabProvider::new(connection, reqwest::Client::new());
    assert!(provider.close(REPO, 7).await.is_err());
    assert!(provider.set_vote(REPO, 7, ReviewVote::Approved, None).await.is_err());
    assert!(
        server.received_requests().await.unwrap_or_default().is_empty(),
        "no request without a token"
    );
}
