//! Every write path against a mocked Azure DevOps REST API (7.1): method, path, api-version,
//! Basic auth and body as documented on learn.microsoft.com/rest/api/azure/devops/git.
//! The base URL is the mock server root with the collection in the path, the Azure DevOps Server layout.

use review_core::{
    DiffSide, FilePositions, LineRange, MergeOptions, MergeStrategy, NewThread, Provider,
    ProviderError, RangeDetail, RerunAction, RerunKind, ReviewVote, ThreadAnchor, ThreadStatus,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, body_partial_json, header, method, path, query_param};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

use crate::support::{
    AUTHORIZATION, COLLECTION, PR, REPO, TOKEN, api, mount_source_tip, ok, provider, pull_request,
};

const ME: &str = "me-id";

fn pr_path(suffix: &str) -> String {
    format!("/{COLLECTION}/My%20Project/_apis/git/repositories/web/pullrequests/{PR}{suffix}")
}

/// Asserts that a top-level JSON body field is absent.
struct WithoutField(&'static str);

impl Match for WithoutField {
    fn matches(&self, request: &Request) -> bool {
        serde_json::from_slice::<Value>(&request.body).is_ok_and(|body| body.get(self.0).is_none())
    }
}

async fn mount_current_user(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(format!("/{COLLECTION}/_apis/connectionData")))
        .and(query_param("api-version", "7.1-preview"))
        .and(header("authorization", AUTHORIZATION))
        .respond_with(ok(json!({ "authenticatedUser": { "id": ME, "providerDisplayName": "Me" } })))
        .expect(1)
        .mount(server)
        .await;
}

fn thread_response(id: u64, content: &str, thread_context: &Value) -> Value {
    json!({
        "id": id, "status": "active", "publishedDate": "2026-10-18T10:00:00.123Z",
        "threadContext": thread_context,
        "comments": [{ "id": 1, "parentCommentId": 0, "content": content, "commentType": "text",
                       "publishedDate": "2026-10-18T10:00:00.123Z", "author": { "id": ME, "displayName": "Me" } }]
    })
}

fn new_comment(content: &str) -> Value {
    json!({ "comments": [{ "parentCommentId": 0, "content": content, "commentType": 1 }], "status": "active" })
}

#[tokio::test]
async fn set_vote_puts_each_vote_value_on_own_reviewer_entry() {
    let server = MockServer::start().await;
    mount_current_user(&server).await;
    let votes = [
        (ReviewVote::Approved, 10),
        (ReviewVote::ApprovedWithSuggestions, 5),
        (ReviewVote::None, 0),
        (ReviewVote::WaitingForAuthor, -5),
        (ReviewVote::ChangesRequested, -10),
    ];
    for (_, value) in votes {
        api("PUT", &pr_path(&format!("/reviewers/{ME}")))
            .and(body_partial_json(json!({ "vote": value })))
            .respond_with(ok(json!({ "id": ME, "vote": value })))
            .expect(1)
            .mount(&server)
            .await;
    }
    let provider = provider(&server, &[COLLECTION], Some(TOKEN));
    for (vote, _) in votes {
        provider.set_vote(REPO, PR, vote, None).await.unwrap();
    }
}

#[tokio::test]
async fn set_vote_with_comment_adds_general_thread() {
    let server = MockServer::start().await;
    mount_current_user(&server).await;
    api("PUT", &pr_path(&format!("/reviewers/{ME}")))
        .and(body_partial_json(json!({ "vote": 10 })))
        .respond_with(ok(json!({ "id": ME, "vote": 10 })))
        .expect(1)
        .mount(&server)
        .await;
    api("POST", &pr_path("/threads"))
        .and(body_partial_json(new_comment("Looks good")))
        .and(WithoutField("threadContext"))
        .respond_with(ok(thread_response(3, "Looks good", &Value::Null)))
        .expect(1)
        .mount(&server)
        .await;
    provider(&server, &[COLLECTION], Some(TOKEN))
        .set_vote(REPO, PR, ReviewVote::Approved, Some(" Looks good "))
        .await
        .unwrap();
}

#[tokio::test]
async fn create_thread_on_pull_request() {
    let server = MockServer::start().await;
    api("POST", &pr_path("/threads"))
        .and(body_partial_json(new_comment("General")))
        .and(WithoutField("threadContext"))
        .and(WithoutField("pullRequestThreadContext"))
        .respond_with(ok(thread_response(4, "General", &Value::Null)))
        .expect(1)
        .mount(&server)
        .await;
    let thread = NewThread { body: "General".to_owned(), anchor: None };
    let created = provider(&server, &[COLLECTION], Some(TOKEN))
        .create_thread(REPO, PR, &thread)
        .await
        .unwrap();
    assert_eq!(created.id, "4");
    assert!(created.anchor.is_none());
    assert_eq!(created.comments[0].body, "General");
}

async fn mount_iterations(server: &MockServer) {
    api("GET", &pr_path("/iterations"))
        .and(query_param("includeCommits", "false"))
        .respond_with(ok(json!({ "count": 2, "value": [{ "id": 1 }, { "id": 3 }, { "id": 2 }] })))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn create_thread_on_new_line_attaches_to_latest_iteration_change() {
    let server = MockServer::start().await;
    mount_iterations(&server).await;
    api("GET", &pr_path("/iterations/3/changes"))
        .and(query_param("$top", "2000"))
        .and(query_param("$skip", "0"))
        .respond_with(ok(json!({
            "changeEntries": [
                { "changeTrackingId": 4, "changeId": 1, "item": { "path": "/README.md" }, "changeType": "edit" },
                { "changeTrackingId": 5, "changeId": 2, "item": { "path": "/src/lib.rs" }, "changeType": "edit" }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let context = json!({
        "filePath": "/src/lib.rs",
        "rightFileStart": { "line": 12, "offset": 1 },
        "rightFileEnd": { "line": 12, "offset": 1 }
    });
    let mut expected = new_comment("New side");
    expected["threadContext"] = context.clone();
    expected["pullRequestThreadContext"] = json!({ "changeTrackingId": 5, "iterationContext": { "firstComparingIteration": 3, "secondComparingIteration": 3 } });
    api("POST", &pr_path("/threads"))
        .and(body_partial_json(expected))
        .respond_with(ok(thread_response(5, "New side", &context)))
        .expect(1)
        .mount(&server)
        .await;
    let anchor = ThreadAnchor {
        path: "src/lib.rs".to_owned(),
        old_path: None,
        old_line: Some(10),
        new_line: Some(12),
        range: None,
    };
    let thread = NewThread { body: "New side".to_owned(), anchor: Some(anchor) };
    let created = provider(&server, &[COLLECTION], Some(TOKEN))
        .create_thread(REPO, PR, &thread)
        .await
        .unwrap();
    let anchor = created.anchor.unwrap();
    assert_eq!(
        (anchor.path.as_str(), anchor.new_line, anchor.old_line),
        ("src/lib.rs", Some(12), None)
    );
}

#[tokio::test]
async fn create_thread_on_deleted_line_uses_left_file_and_pages_changes() {
    let server = MockServer::start().await;
    mount_iterations(&server).await;
    api("GET", &pr_path("/iterations/3/changes"))
        .and(query_param("$skip", "0"))
        .respond_with(ok(json!({
            "changeEntries": [{ "changeTrackingId": 1, "item": { "path": "/other.rs" } }],
            "nextSkip": 2000, "nextTop": 2000
        })))
        .expect(1)
        .mount(&server)
        .await;
    api("GET", &pr_path("/iterations/3/changes"))
        .and(query_param("$skip", "2000"))
        .respond_with(ok(json!({
            "changeEntries": [{ "changeTrackingId": 9, "item": { "path": "/old.txt" }, "changeType": "edit" }],
            "nextSkip": 0, "nextTop": 0
        })))
        .expect(1)
        .mount(&server)
        .await;
    let context = json!({
        "filePath": "/old.txt",
        "leftFileStart": { "line": 4, "offset": 1 },
        "leftFileEnd": { "line": 4, "offset": 1 }
    });
    let mut expected = new_comment("Old side");
    expected["threadContext"] = context.clone();
    expected["pullRequestThreadContext"] = json!({ "changeTrackingId": 9, "iterationContext": { "firstComparingIteration": 3, "secondComparingIteration": 3 } });
    api("POST", &pr_path("/threads"))
        .and(body_partial_json(expected))
        .respond_with(ok(thread_response(6, "Old side", &context)))
        .expect(1)
        .mount(&server)
        .await;
    let anchor = ThreadAnchor {
        path: "old.txt".to_owned(),
        old_path: None,
        old_line: Some(4),
        new_line: None,
        range: None,
    };
    let thread = NewThread { body: "Old side".to_owned(), anchor: Some(anchor) };
    let created = provider(&server, &[COLLECTION], Some(TOKEN))
        .create_thread(REPO, PR, &thread)
        .await
        .unwrap();
    let anchor = created.anchor.unwrap();
    assert_eq!((anchor.old_line, anchor.new_line), (Some(4), None));
}

/// Multi-line thread: `rightFileStart`/`rightFileEnd` span the first line's first selected
/// character to the last line's end (learn.microsoft.com, "Pull Request Threads - Create").
#[tokio::test]
async fn create_thread_on_a_line_range_spans_right_file_start_to_end() {
    let server = MockServer::start().await;
    mount_iterations(&server).await;
    api("GET", &pr_path("/iterations/3/changes"))
        .respond_with(ok(json!({
            "changeEntries": [{ "changeTrackingId": 5, "changeId": 2, "item": { "path": "/src/lib.rs" }, "changeType": "edit" }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let context = json!({
        "filePath": "/src/lib.rs",
        "rightFileStart": { "line": 10, "offset": 1 },
        "rightFileEnd": { "line": 14, "offset": 23 }
    });
    let mut expected = new_comment("Range");
    expected["threadContext"] = context.clone();
    expected["pullRequestThreadContext"] = json!({ "changeTrackingId": 5, "iterationContext": { "firstComparingIteration": 3, "secondComparingIteration": 3 } });
    api("POST", &pr_path("/threads"))
        .and(body_json(expected))
        .respond_with(ok(thread_response(7, "Range", &context)))
        .expect(1)
        .mount(&server)
        .await;
    let anchor = ThreadAnchor {
        path: "src/lib.rs".to_owned(),
        old_path: None,
        old_line: None,
        new_line: Some(14),
        range: Some(LineRange {
            start_old_line: Some(9),
            start_new_line: Some(10),
            side: DiffSide::New,
            detail: Some(RangeDetail {
                start: FilePositions { old: 9, new: 10 },
                end: FilePositions { old: 13, new: 14 },
                start_offset: 1,
                end_offset: 23,
            }),
        }),
    };
    let thread = NewThread { body: "Range".to_owned(), anchor: Some(anchor) };
    let created = provider(&server, &[COLLECTION], Some(TOKEN))
        .create_thread(REPO, PR, &thread)
        .await
        .unwrap();
    let anchor = created.anchor.unwrap();
    assert_eq!(anchor.new_line, Some(14));
    assert_eq!(
        anchor.range,
        Some(LineRange {
            start_old_line: None,
            start_new_line: Some(10),
            side: DiffSide::New,
            detail: None
        })
    );
}

#[tokio::test]
async fn reply_posts_comment_under_first_comment() {
    let server = MockServer::start().await;
    api("GET", &pr_path("/threads/9"))
        .respond_with(ok(thread_response(9, "Question", &Value::Null)))
        .expect(1)
        .mount(&server)
        .await;
    api("POST", &pr_path("/threads/9/comments"))
        .and(body_partial_json(
            json!({ "parentCommentId": 1, "content": "Answer", "commentType": 1 }),
        ))
        .respond_with(ok(json!({
            "id": 2, "parentCommentId": 1, "content": "Answer", "commentType": "text",
            "publishedDate": "2026-10-18T11:00:00Z", "author": { "id": ME, "displayName": "Me" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let comment =
        provider(&server, &[COLLECTION], Some(TOKEN)).reply(REPO, PR, "9", "Answer").await.unwrap();
    assert_eq!((comment.id.as_str(), comment.body.as_str()), ("2", "Answer"));
}

#[tokio::test]
async fn set_thread_status_patches_fixed_and_active() {
    let server = MockServer::start().await;
    for status in ["fixed", "active"] {
        api("PATCH", &pr_path("/threads/9"))
            .and(body_partial_json(json!({ "status": status })))
            .respond_with(ok(thread_response(9, "Question", &Value::Null)))
            .expect(1)
            .mount(&server)
            .await;
    }
    let provider = provider(&server, &[COLLECTION], Some(TOKEN));
    provider.set_thread_status(REPO, PR, "9", ThreadStatus::Resolved).await.unwrap();
    provider.set_thread_status(REPO, PR, "9", ThreadStatus::Active).await.unwrap();
}

#[tokio::test]
async fn set_labels_adds_missing_and_deletes_unwanted() {
    let server = MockServer::start().await;
    api("GET", &pr_path("/labels"))
        .respond_with(ok(json!({ "count": 2, "value": [
            { "id": "t1", "name": "bug", "active": true },
            { "id": "t2", "name": "needs work", "active": true }
        ] })))
        .expect(1)
        .mount(&server)
        .await;
    api("POST", &pr_path("/labels"))
        .and(body_partial_json(json!({ "name": "ui" })))
        .respond_with(ok(json!({ "id": "t3", "name": "ui", "active": true })))
        .expect(1)
        .mount(&server)
        .await;
    api("DELETE", &pr_path("/labels/needs%20work"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let labels = ["Bug".to_owned(), "ui".to_owned()];
    provider(&server, &[COLLECTION], Some(TOKEN)).set_labels(REPO, PR, &labels).await.unwrap();
}

#[tokio::test]
async fn add_reviewer_resolves_identity_then_puts_zero_vote() {
    let server = MockServer::start().await;
    api("GET", &format!("/{COLLECTION}/_apis/identities"))
        .and(query_param("searchFilter", "General"))
        .and(query_param("filterValue", "jo@example.com"))
        .and(query_param("queryMembership", "None"))
        .respond_with(ok(json!({ "count": 1, "value": [{
            "id": "jo-id", "providerDisplayName": "Jo", "isActive": true,
            "properties": { "Mail": { "$type": "System.String", "$value": "jo@example.com" } }
        }] })))
        .expect(1)
        .mount(&server)
        .await;
    api("PUT", &pr_path("/reviewers/jo-id"))
        .and(body_partial_json(json!({ "vote": 0 })))
        .respond_with(ok(json!({ "id": "jo-id", "vote": 0 })))
        .expect(1)
        .mount(&server)
        .await;
    provider(&server, &[COLLECTION], Some(TOKEN))
        .add_reviewer(REPO, PR, " jo@example.com ")
        .await
        .unwrap();
}

#[tokio::test]
async fn adding_the_signed_in_user_puts_their_own_identity_without_a_search() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/{COLLECTION}/_apis/connectionData")))
        .and(query_param("api-version", "7.1-preview"))
        .and(header("authorization", AUTHORIZATION))
        .respond_with(ok(json!({ "authenticatedUser": {
            "id": ME, "providerDisplayName": "Me",
            "properties": { "Account": { "$type": "System.String", "$value": "me@example.com" } }
        } })))
        .expect(1)
        .mount(&server)
        .await;
    api("GET", &format!("/{COLLECTION}/_apis/identities"))
        .respond_with(ok(json!({})))
        .expect(0)
        .mount(&server)
        .await;
    api("PUT", &pr_path(&format!("/reviewers/{ME}")))
        .and(body_json(json!({ "vote": 0 })))
        .respond_with(ok(json!({ "id": ME, "vote": 0 })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, &[COLLECTION], Some(TOKEN));
    let me = provider.current_user().await.unwrap();
    assert_eq!(me.login.as_deref(), Some("me@example.com"));
    provider.add_reviewer(REPO, PR, "ME@example.com").await.unwrap();
}

#[tokio::test]
async fn remove_reviewer_deletes_reviewer_entry() {
    let server = MockServer::start().await;
    api("DELETE", &pr_path("/reviewers/jo-id"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    provider(&server, &[COLLECTION], Some(TOKEN)).remove_reviewer(REPO, PR, "jo-id").await.unwrap();
}

/// The completion carries the reviewed source commit, not one fetched before it.
#[tokio::test]
async fn merge_completes_with_each_strategy_against_the_reviewed_source_commit() {
    let server = MockServer::start().await;
    api("GET", &pr_path(""))
        .respond_with(ok(pull_request(&json!({}))))
        .expect(4)
        .mount(&server)
        .await;
    mount_source_tip(&server, "reviewed-sha", Some(4)).await;
    let strategies = [
        (MergeStrategy::Merge, "noFastForward"),
        (MergeStrategy::Squash, "squash"),
        (MergeStrategy::Rebase, "rebase"),
        (MergeStrategy::SemiLinear, "rebaseMerge"),
    ];
    for (_, ado) in strategies {
        api("PATCH", &pr_path(""))
            .and(body_partial_json(json!({
                "status": "completed",
                "lastMergeSourceCommit": { "commitId": "reviewed-sha" },
                "completionOptions": { "mergeStrategy": ado, "deleteSourceBranch": true, "mergeCommitMessage": "Merged" }
            })))
            .respond_with(ok(pull_request(&json!({}))))
            .expect(1)
            .mount(&server)
            .await;
    }
    let provider = provider(&server, &[COLLECTION], Some(TOKEN));
    for (strategy, _) in strategies {
        let options = MergeOptions {
            strategy,
            delete_source_branch: true,
            commit_message: Some("Merged".to_owned()),
            head_sha: "reviewed-sha".to_owned(),
        };
        provider.merge(REPO, PR, &options).await.unwrap();
    }
}

/// Azure DevOps accepts a completion against the commit before a push until it computed the merge
/// again (the pull request still names that commit): a source branch past the reviewed commit is
/// the head-moved error, and no completion goes out.
#[tokio::test]
async fn merge_refuses_a_source_branch_pushed_past_the_reviewed_commit_before_completing() {
    let server = MockServer::start().await;
    let lagging = pull_request(&json!({ "lastMergeSourceCommit": { "commitId": "reviewed-sha" } }));
    api("GET", &pr_path("")).respond_with(ok(lagging)).expect(1).mount(&server).await;
    mount_source_tip(&server, "pushed-sha", Some(1)).await;
    api("PATCH", &pr_path(""))
        .respond_with(ok(pull_request(&json!({}))))
        .expect(0)
        .mount(&server)
        .await;
    let provider = provider(&server, &[COLLECTION], Some(TOKEN));
    let options = MergeOptions {
        strategy: MergeStrategy::Squash,
        delete_source_branch: false,
        commit_message: None,
        head_sha: "reviewed-sha".to_owned(),
    };
    let error = provider.merge(REPO, PR, &options).await.expect_err("the branch moved");
    assert!(matches!(error, ProviderError::HeadMoved), "{error:?}");
}

/// A completion refused while the source commit moved past the reviewed one (pushed after the
/// branch was checked) is the head-moved error; refused at the reviewed commit, the platform's own
/// answer stays.
#[tokio::test]
async fn merge_refused_for_a_moved_source_reports_new_commits() {
    let server = MockServer::start().await;
    api("GET", &pr_path(""))
        .respond_with(ok(pull_request(&json!({}))))
        .expect(4)
        .mount(&server)
        .await;
    mount_source_tip(&server, "reviewed-sha", Some(1)).await;
    api("PATCH", &pr_path(""))
        .respond_with(
            ResponseTemplate::new(409)
                .set_body_json(json!({ "message": "The pull request was updated." })),
        )
        .expect(2)
        .mount(&server)
        .await;
    let provider = provider(&server, &[COLLECTION], Some(TOKEN));
    let mut options = MergeOptions {
        strategy: MergeStrategy::Squash,
        delete_source_branch: false,
        commit_message: None,
        head_sha: "reviewed-sha".to_owned(),
    };
    let error = provider.merge(REPO, PR, &options).await.expect_err("source moved to head-sha");
    assert!(matches!(error, ProviderError::HeadMoved), "{error:?}");

    mount_source_tip(&server, "head-sha", Some(1)).await;
    "head-sha".clone_into(&mut options.head_sha);
    let error = provider.merge(REPO, PR, &options).await.expect_err("refused at the same head");
    assert_eq!(error.status(), Some(409));
}

#[tokio::test]
async fn lifecycle_and_metadata_updates_patch_the_pull_request() {
    let server = MockServer::start().await;
    let bodies = [
        json!({ "status": "abandoned" }),
        json!({ "status": "active" }),
        json!({ "isDraft": true }),
        json!({ "isDraft": false }),
        json!({ "title": "New title", "description": "New body" }),
    ];
    for body in bodies {
        api("PATCH", &pr_path(""))
            .and(body_partial_json(body))
            .respond_with(ok(pull_request(&json!({}))))
            .expect(1)
            .mount(&server)
            .await;
    }
    let provider = provider(&server, &[COLLECTION], Some(TOKEN));
    provider.close(REPO, PR).await.unwrap();
    provider.reopen(REPO, PR).await.unwrap();
    provider.set_draft(REPO, PR, true).await.unwrap();
    provider.set_draft(REPO, PR, false).await.unwrap();
    provider.update_title_description(REPO, PR, "New title", "New body").await.unwrap();
}

/// Stages - Update: `PATCH build/builds/{buildId}/stages/{stageRefName}`, body exactly
/// `{ state, forceRetryAllJobs }`.
#[tokio::test]
async fn stage_retry_patches_the_stage_with_state_retry() {
    let server = MockServer::start().await;
    let stage_path =
        format!("/{COLLECTION}/My%20Project/_apis/build/builds/42/stages/Deploy_integration");
    for force in [true, false] {
        api("PATCH", &stage_path)
            .and(body_json(json!({ "state": "retry", "forceRetryAllJobs": force })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }
    let provider = provider(&server, &[COLLECTION], Some(TOKEN));
    let rerun_all = RerunAction::new(RerunKind::StageAll, "42:Deploy_integration");
    provider.rerun(REPO, PR, "build:42", &rerun_all).await.unwrap();
    let retry_failed = RerunAction::new(RerunKind::StageFailed, "42:Deploy_integration");
    provider.rerun(REPO, PR, "build:42", &retry_failed).await.unwrap();
    let job = RerunAction::new(RerunKind::Job, "42:Deploy_integration");
    assert!(matches!(
        provider.rerun(REPO, PR, "build:42", &job).await,
        Err(ProviderError::Unsupported(_))
    ));
}

#[tokio::test]
async fn writes_without_token_send_nothing() {
    let server = MockServer::start().await;
    let provider = provider(&server, &[COLLECTION], None);
    let options = MergeOptions {
        strategy: MergeStrategy::Squash,
        delete_source_branch: false,
        commit_message: None,
        head_sha: "head-sha".to_owned(),
    };
    let thread = NewThread { body: "x".to_owned(), anchor: None };
    let results = [
        provider.set_vote(REPO, PR, ReviewVote::Approved, None).await.err(),
        provider.create_thread(REPO, PR, &thread).await.err(),
        provider.reply(REPO, PR, "9", "x").await.err(),
        provider.set_thread_status(REPO, PR, "9", ThreadStatus::Resolved).await.err(),
        provider.set_labels(REPO, PR, &[]).await.err(),
        provider.add_reviewer(REPO, PR, "jo").await.err(),
        provider.remove_reviewer(REPO, PR, "jo-id").await.err(),
        provider.merge(REPO, PR, &options).await.err(),
        provider.set_auto_complete(REPO, PR, &options).await.err(),
        provider.cancel_auto_complete(REPO, PR).await.err(),
        provider.edit_comment(REPO, PR, "9", "1", "x").await.err(),
        provider.delete_comment(REPO, PR, "9", "1").await.err(),
        provider.close(REPO, PR).await.err(),
        provider.reopen(REPO, PR).await.err(),
        provider.set_draft(REPO, PR, true).await.err(),
        provider.update_title_description(REPO, PR, "t", "d").await.err(),
        provider
            .rerun(REPO, PR, "build:42", &RerunAction::new(RerunKind::StageAll, "42:Build"))
            .await
            .err(),
    ];
    for result in results {
        assert!(matches!(result, Some(ProviderError::Unsupported(_))), "{result:?}");
    }
    assert!(server.received_requests().await.unwrap_or_default().is_empty());
}
