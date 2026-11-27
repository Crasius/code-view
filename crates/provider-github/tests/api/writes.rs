//! Write paths against a mock GitHub Enterprise host. Every matcher follows the documented
//! REST v3 (2022-11-28) and GraphQL contracts; `.expect(n)` is verified when the server drops.

use provider_github::GitHubProvider;
use review_core::{
    Connection, DiffSide, LineRange, MergeOptions, MergeStrategy, NewThread, Provider,
    ProviderError, ProviderKind, ReviewVote, ThreadAnchor, ThreadStatus,
};
use serde_json::{Value, json};
use wiremock::matchers::{
    body_json, body_partial_json, body_string_contains, header, method, path,
};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

const REPO: &str = "contoso/app";
const NUMBER: u64 = 7;
const HEAD_SHA: &str = "head111";
/// The head the user reviewed, older than the fixture's: the merge must guard with this one.
const REVIEWED_SHA: &str = "reviewed000";
const PULL_NODE_ID: &str = "PR_kwDOAbc";

fn pull_fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/pull_request.json"))
        .expect("pull request fixture")
}

async fn setup() -> (MockServer, GitHubProvider) {
    let server = MockServer::start().await;
    let connection = Connection {
        id: "mock".to_owned(),
        name: "Mock GitHub Enterprise".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: server.uri(),
        organizations: Vec::new(),
        token: Some("test-token".to_owned()),
    };
    let provider = GitHubProvider::new(connection, reqwest::Client::new());
    (server, provider)
}

/// REST call below `/api/v3/repos/contoso/app` with GitHub's required headers.
fn rest(verb: &str, repo_path: &str) -> MockBuilder {
    Mock::given(method(verb))
        .and(path(format!("/api/v3/repos/{REPO}{repo_path}")))
        .and(header("authorization", "Bearer test-token"))
        .and(header("accept", "application/vnd.github+json"))
        .and(header("x-github-api-version", "2022-11-28"))
}

fn graphql() -> MockBuilder {
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(header("authorization", "Bearer test-token"))
}

fn ok_json(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

async fn mount_pull(server: &MockServer, pull: Value) {
    rest("GET", &format!("/pulls/{NUMBER}")).respond_with(ok_json(pull)).mount(server).await;
}

/// The one page of newest review threads a thread lookup after a new comment reads.
fn newest_threads(nodes: &Value) -> ResponseTemplate {
    ok_json(json!({ "data": { "repository": { "pullRequest": { "reviewThreads": {
        "pageInfo": { "hasPreviousPage": false }, "nodes": nodes
    } } } } }))
}

fn review_comment(id: u64, line: u32, side: &str) -> Value {
    json!({
        "id": id, "path": "src/main.rs", "line": line, "side": side, "subject_type": "line",
        "user": { "id": 1, "login": "mona" }, "body": "note", "created_at": "2024-06-01T10:00:00Z"
    })
}

fn graphql_comment(database_id: u64) -> Value {
    json!({
        "id": format!("PRRC_{database_id}"), "databaseId": database_id, "body": "note",
        "createdAt": "2024-06-01T10:00:00Z", "author": { "login": "mona", "databaseId": 1 }
    })
}

#[tokio::test]
async fn set_vote_posts_review_events() {
    let (server, provider) = setup().await;
    rest("POST", &format!("/pulls/{NUMBER}/reviews"))
        .and(body_json(json!({ "event": "APPROVE" })))
        .respond_with(ok_json(json!({ "id": 1 })))
        .expect(1)
        .mount(&server)
        .await;
    rest("POST", &format!("/pulls/{NUMBER}/reviews"))
        .and(body_json(json!({ "event": "REQUEST_CHANGES", "body": "Please fix" })))
        .respond_with(ok_json(json!({ "id": 2 })))
        .expect(1)
        .mount(&server)
        .await;

    provider.set_vote(REPO, NUMBER, ReviewVote::Approved, None).await.expect("approve");
    provider
        .set_vote(REPO, NUMBER, ReviewVote::ChangesRequested, Some("Please fix"))
        .await
        .expect("request changes");
}

/// GitHub says why it refused a review only in `errors`; the failure names that reason instead of
/// "Unprocessable Entity".
#[tokio::test]
async fn a_refused_vote_names_githubs_reason() {
    let (server, provider) = setup().await;
    rest("POST", &format!("/pulls/{NUMBER}/reviews"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Unprocessable Entity",
            "errors": ["Review Can not approve your own pull request"],
            "status": "422"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let error = provider.set_vote(REPO, NUMBER, ReviewVote::Approved, None).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "GitHub did not take the review: Review Can not approve your own pull request."
    );
}

#[tokio::test]
async fn unsupported_votes_send_nothing() {
    let (server, provider) = setup().await;
    let reset = provider.set_vote(REPO, NUMBER, ReviewVote::None, None).await;
    assert!(matches!(reset, Err(ProviderError::Unsupported(_))), "{reset:?}");
    let without_body = provider.set_vote(REPO, NUMBER, ReviewVote::ChangesRequested, None).await;
    assert!(matches!(without_body, Err(ProviderError::Local(_))), "{without_body:?}");
    assert!(server.received_requests().await.expect("recording").is_empty());
}

#[tokio::test]
async fn create_thread_general_comment_uses_issue_comments() {
    let (server, provider) = setup().await;
    rest("POST", &format!("/issues/{NUMBER}/comments"))
        .and(body_json(json!({ "body": "Looks good" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": 900, "user": { "id": 1, "login": "mona" }, "body": "Looks good",
            "created_at": "2024-06-01T10:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let thread = provider
        .create_thread(REPO, NUMBER, &NewThread { body: "Looks good".to_owned(), anchor: None })
        .await
        .expect("general comment");
    assert_eq!(thread.id, "issue-comment:900");
}

#[tokio::test]
async fn create_thread_on_new_line_comments_right_and_resolves_graphql_thread() {
    let (server, provider) = setup().await;
    mount_pull(&server, pull_fixture()).await;
    rest("POST", &format!("/pulls/{NUMBER}/comments"))
        .and(body_json(json!({
            "body": "note", "commit_id": HEAD_SHA, "path": "src/main.rs", "line": 12, "side": "RIGHT"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(review_comment(555, 12, "RIGHT")))
        .expect(1)
        .mount(&server)
        .await;
    graphql()
        .and(body_string_contains("reviewThreads(last: 20"))
        .and(body_partial_json(
            json!({ "variables": { "owner": "contoso", "name": "app", "number": NUMBER } }),
        ))
        .respond_with(newest_threads(&json!([{
            "id": "PRRT_new", "isResolved": false, "isOutdated": false, "path": "src/main.rs",
            "line": 12, "originalLine": 12, "diffSide": "RIGHT",
            "comments": { "nodes": [graphql_comment(555)] }
        }])))
        .expect(1)
        .mount(&server)
        .await;

    let anchor = ThreadAnchor {
        path: "src/main.rs".to_owned(),
        old_path: None,
        old_line: None,
        new_line: Some(12),
        range: None,
    };
    let thread = provider
        .create_thread(REPO, NUMBER, &NewThread { body: "note".to_owned(), anchor: Some(anchor) })
        .await
        .expect("line comment");
    assert_eq!(thread.id, "PRRT_new");
    assert!(thread.can_reply && thread.can_resolve);
}

#[tokio::test]
async fn create_thread_on_old_line_comments_left_and_falls_back_to_rest_id() {
    let (server, provider) = setup().await;
    mount_pull(&server, pull_fixture()).await;
    rest("POST", &format!("/pulls/{NUMBER}/comments"))
        .and(body_json(json!({
            "body": "note", "commit_id": HEAD_SHA, "path": "src/main.rs", "line": 4, "side": "LEFT"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(review_comment(556, 4, "LEFT")))
        .expect(1)
        .mount(&server)
        .await;
    graphql().respond_with(newest_threads(&json!([]))).expect(1).mount(&server).await;

    let anchor = ThreadAnchor {
        path: "src/main.rs".to_owned(),
        old_path: None,
        old_line: Some(4),
        new_line: None,
        range: None,
    };
    let thread = provider
        .create_thread(REPO, NUMBER, &NewThread { body: "note".to_owned(), anchor: Some(anchor) })
        .await
        .expect("old-side comment");
    assert_eq!(thread.id, "review-comment:556");
    assert!(thread.can_reply);
    let anchor = thread.anchor.expect("anchor");
    assert_eq!((anchor.old_line, anchor.new_line), (Some(4), None));
}

/// Multi-line comment: `start_line`/`start_side` for the first line, `line`/`side` for the last
/// (docs.github.com, "Create a review comment for a pull request").
#[tokio::test]
async fn create_thread_on_a_line_range_sends_start_and_end() {
    let (server, provider) = setup().await;
    mount_pull(&server, pull_fixture()).await;
    rest("POST", &format!("/pulls/{NUMBER}/comments"))
        .and(body_json(json!({
            "body": "range", "commit_id": HEAD_SHA, "path": "src/main.rs",
            "start_line": 10, "start_side": "RIGHT", "line": 14, "side": "RIGHT"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": 557, "path": "src/main.rs", "line": 14, "side": "RIGHT", "start_line": 10,
            "start_side": "RIGHT", "subject_type": "line", "user": { "id": 1, "login": "mona" },
            "body": "range", "created_at": "2024-06-01T10:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;
    graphql()
        .and(body_string_contains("startLine originalStartLine startDiffSide"))
        .respond_with(newest_threads(&json!([{
            "id": "PRRT_range", "isResolved": false, "isOutdated": false, "path": "src/main.rs",
            "line": 14, "originalLine": 14, "diffSide": "RIGHT",
            "startLine": 10, "originalStartLine": 10, "startDiffSide": "RIGHT",
            "comments": { "nodes": [graphql_comment(557)] }
        }])))
        .expect(1)
        .mount(&server)
        .await;

    let anchor = ThreadAnchor {
        path: "src/main.rs".to_owned(),
        old_path: None,
        old_line: None,
        new_line: Some(14),
        range: Some(LineRange {
            // A context line: GitHub counts it on the side the range was picked in.
            start_old_line: Some(9),
            start_new_line: Some(10),
            side: DiffSide::New,
            detail: None,
        }),
    };
    let thread = provider
        .create_thread(REPO, NUMBER, &NewThread { body: "range".to_owned(), anchor: Some(anchor) })
        .await
        .expect("range comment");
    assert_eq!(thread.id, "PRRT_range");
    let range = thread.anchor.and_then(|anchor| anchor.range).expect("range");
    assert_eq!((range.start_new_line, range.side), (Some(10), DiffSide::New));
}

#[tokio::test]
async fn reply_uses_graphql_for_threads_and_rest_for_review_comments() {
    let (server, provider) = setup().await;
    graphql()
        .and(body_string_contains("addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $thread, body: $body })"))
        .and(body_partial_json(json!({ "variables": { "thread": "PRRT_x", "body": "Thanks" } })))
        .respond_with(ok_json(json!({ "data": { "addPullRequestReviewThreadReply": {
            "comment": graphql_comment(777)
        } } })))
        .expect(1)
        .mount(&server)
        .await;
    rest("POST", &format!("/pulls/{NUMBER}/comments/555/replies"))
        .and(body_json(json!({ "body": "Thanks" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(review_comment(778, 12, "RIGHT")))
        .expect(1)
        .mount(&server)
        .await;

    let comment = provider.reply(REPO, NUMBER, "PRRT_x", "Thanks").await.expect("graphql reply");
    assert_eq!(comment.id, "777");
    let comment =
        provider.reply(REPO, NUMBER, "review-comment:555", "Thanks").await.expect("rest reply");
    assert_eq!(comment.id, "778");
    let issue_reply = provider.reply(REPO, NUMBER, "issue-comment:900", "Thanks").await;
    assert!(matches!(issue_reply, Err(ProviderError::Unsupported(_))), "{issue_reply:?}");
}

#[tokio::test]
async fn set_thread_status_resolves_and_unresolves() {
    let (server, provider) = setup().await;
    graphql()
        .and(body_string_contains("{ resolveReviewThread(input: { threadId: $id })"))
        .and(body_partial_json(json!({ "variables": { "id": "PRRT_x" } })))
        .respond_with(ok_json(
            json!({ "data": { "resolveReviewThread": { "thread": { "id": "PRRT_x" } } } }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    graphql()
        .and(body_string_contains("unresolveReviewThread(input: { threadId: $id })"))
        .and(body_partial_json(json!({ "variables": { "id": "PRRT_x" } })))
        .respond_with(ok_json(
            json!({ "data": { "unresolveReviewThread": { "thread": { "id": "PRRT_x" } } } }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    provider
        .set_thread_status(REPO, NUMBER, "PRRT_x", ThreadStatus::Resolved)
        .await
        .expect("resolve");
    provider
        .set_thread_status(REPO, NUMBER, "PRRT_x", ThreadStatus::Active)
        .await
        .expect("unresolve");
}

#[tokio::test]
async fn set_labels_replaces_issue_labels() {
    let (server, provider) = setup().await;
    rest("PUT", &format!("/issues/{NUMBER}/labels"))
        .and(body_json(json!({ "labels": ["bug", "ui"] })))
        .respond_with(ok_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;

    let labels = ["bug".to_owned(), "ui".to_owned()];
    provider.set_labels(REPO, NUMBER, &labels).await.expect("labels");
}

#[tokio::test]
async fn add_reviewer_requests_users_and_teams() {
    let (server, provider) = setup().await;
    rest("POST", &format!("/pulls/{NUMBER}/requested_reviewers"))
        .and(body_json(json!({ "reviewers": ["octocat"] })))
        .respond_with(ResponseTemplate::new(201).set_body_json(pull_fixture()))
        .expect(1)
        .mount(&server)
        .await;
    rest("POST", &format!("/pulls/{NUMBER}/requested_reviewers"))
        .and(body_json(json!({ "team_reviewers": ["core"] })))
        .respond_with(ResponseTemplate::new(201).set_body_json(pull_fixture()))
        .expect(1)
        .mount(&server)
        .await;

    provider.add_reviewer(REPO, NUMBER, "@octocat").await.expect("user");
    provider.add_reviewer(REPO, NUMBER, "contoso/core").await.expect("team");
}

#[tokio::test]
async fn a_refused_review_request_names_the_rules_and_keeps_githubs_words() {
    let (server, provider) = setup().await;
    rest("POST", &format!("/pulls/{NUMBER}/requested_reviewers"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Reviews may only be requested from collaborators."
        })))
        .expect(1)
        .mount(&server)
        .await;

    let error = provider.add_reviewer(REPO, NUMBER, "octocat").await.unwrap_err();
    let ProviderError::Local(message) = error else { panic!("a local message: {error:?}") };
    assert!(
        message.contains("octocat") && message.contains("collaborators on contoso/app"),
        "{message}"
    );
    assert!(message.ends_with("Reviews may only be requested from collaborators."), "{message}");
}

#[tokio::test]
async fn remove_reviewer_deletes_pending_requests() {
    let (server, provider) = setup().await;
    rest("GET", &format!("/pulls/{NUMBER}/requested_reviewers"))
        .respond_with(ok_json(json!({
            "users": [{ "id": 3, "login": "cy" }],
            "teams": [{ "id": 77, "name": "Maintainers", "slug": "maintainers" }]
        })))
        .expect(2)
        .mount(&server)
        .await;
    rest("DELETE", &format!("/pulls/{NUMBER}/requested_reviewers"))
        .and(body_json(json!({ "reviewers": ["cy"] })))
        .respond_with(ok_json(pull_fixture()))
        .expect(1)
        .mount(&server)
        .await;
    rest("DELETE", &format!("/pulls/{NUMBER}/requested_reviewers"))
        .and(body_json(json!({ "reviewers": [], "team_reviewers": ["maintainers"] })))
        .respond_with(ok_json(pull_fixture()))
        .expect(1)
        .mount(&server)
        .await;

    provider.remove_reviewer(REPO, NUMBER, "3").await.expect("user");
    provider.remove_reviewer(REPO, NUMBER, "team:77").await.expect("team");
}

#[tokio::test]
async fn merge_with_merge_commit_and_branch_deletion() {
    let (server, provider) = setup().await;
    mount_pull(&server, pull_fixture()).await;
    rest("PUT", &format!("/pulls/{NUMBER}/merge"))
        .and(body_json(
            json!({ "merge_method": "merge", "sha": REVIEWED_SHA, "commit_title": "Merge it" }),
        ))
        .respond_with(ok_json(json!({ "sha": "merged", "merged": true, "message": "Pull Request successfully merged" })))
        .expect(1)
        .mount(&server)
        .await;
    rest("DELETE", "/git/refs/heads/feature/x")
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let options = MergeOptions {
        strategy: MergeStrategy::Merge,
        delete_source_branch: true,
        commit_message: Some("Merge it".to_owned()),
        head_sha: REVIEWED_SHA.to_owned(),
    };
    provider.merge(REPO, NUMBER, &options).await.expect("merge");
}

#[tokio::test]
async fn merge_refuses_semi_linear_without_a_request() {
    let (server, provider) = setup().await;
    let options = MergeOptions {
        strategy: MergeStrategy::SemiLinear,
        delete_source_branch: true,
        commit_message: None,
        head_sha: REVIEWED_SHA.to_owned(),
    };
    let result = provider.merge(REPO, NUMBER, &options).await;
    assert!(matches!(result, Err(ProviderError::Unsupported(_))), "{result:?}");
    assert!(server.received_requests().await.expect("recording").is_empty());
}

/// A push after the review moved the head: GitHub answers 409 to the reviewed `sha`, which is
/// the head-moved error, and the branch stays.
#[tokio::test]
async fn merge_refused_for_a_moved_head_reports_new_commits() {
    let (server, provider) = setup().await;
    mount_pull(&server, pull_fixture()).await;
    rest("PUT", &format!("/pulls/{NUMBER}/merge"))
        .and(body_json(json!({ "merge_method": "squash", "sha": REVIEWED_SHA })))
        .respond_with(ResponseTemplate::new(409).set_body_json(
            json!({ "message": "Head branch was modified. Review and try the merge again." }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    rest("DELETE", "/git/refs/heads/feature/x")
        .respond_with(ResponseTemplate::new(204))
        .expect(0)
        .mount(&server)
        .await;
    let options = MergeOptions {
        strategy: MergeStrategy::Squash,
        delete_source_branch: true,
        commit_message: None,
        head_sha: REVIEWED_SHA.to_owned(),
    };
    let error = provider.merge(REPO, NUMBER, &options).await.expect_err("head moved");
    assert!(matches!(error, ProviderError::HeadMoved), "{error:?}");
}

/// Right after a push GitHub answers 405 "not mergeable" while it computes the merge of the new
/// head: with the head past the reviewed `sha` that is the head-moved error; at the reviewed head
/// GitHub's own refusal stays.
#[tokio::test]
async fn merge_refused_as_not_mergeable_after_a_push_reports_new_commits() {
    let (server, provider) = setup().await;
    rest("GET", &format!("/pulls/{NUMBER}"))
        .respond_with(ok_json(pull_fixture()))
        .expect(2)
        .mount(&server)
        .await;
    rest("PUT", &format!("/pulls/{NUMBER}/merge"))
        .respond_with(
            ResponseTemplate::new(405)
                .set_body_json(json!({ "message": "Pull Request is not mergeable" })),
        )
        .expect(2)
        .mount(&server)
        .await;
    let mut options = MergeOptions {
        strategy: MergeStrategy::Squash,
        delete_source_branch: false,
        commit_message: None,
        head_sha: REVIEWED_SHA.to_owned(),
    };
    let error = provider.merge(REPO, NUMBER, &options).await.expect_err("head moved");
    assert!(matches!(error, ProviderError::HeadMoved), "{error:?}");

    HEAD_SHA.clone_into(&mut options.head_sha);
    let error = provider.merge(REPO, NUMBER, &options).await.expect_err("not mergeable");
    assert_eq!(error.status(), Some(405));
}

/// "Automatically delete head branches" removed the branch with the merge: the merge went through,
/// so the delete that finds no branch is no failure. A branch still there that cannot be deleted is.
#[tokio::test]
async fn merge_succeeds_when_github_already_deleted_the_branch() {
    let (server, provider) = setup().await;
    mount_pull(&server, pull_fixture()).await;
    rest("PUT", &format!("/pulls/{NUMBER}/merge"))
        .respond_with(ok_json(json!({ "sha": "merged", "merged": true })))
        .expect(2)
        .mount(&server)
        .await;
    rest("DELETE", "/git/refs/heads/feature/x")
        .respond_with(
            ResponseTemplate::new(422)
                .set_body_json(json!({ "message": "Reference does not exist" })),
        )
        .expect(2)
        .mount(&server)
        .await;
    let options = MergeOptions {
        strategy: MergeStrategy::Merge,
        delete_source_branch: true,
        commit_message: None,
        head_sha: HEAD_SHA.to_owned(),
    };
    rest("GET", "/git/ref/heads/feature/x")
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    provider.merge(REPO, NUMBER, &options).await.expect("merged; the branch is gone already");

    rest("GET", "/git/ref/heads/feature/x")
        .respond_with(ok_json(
            json!({ "ref": "refs/heads/feature/x", "object": { "sha": HEAD_SHA } }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let error = provider.merge(REPO, NUMBER, &options).await.expect_err("the branch stayed");
    assert_eq!(error.status(), Some(422));
}

#[tokio::test]
async fn close_and_reopen_patch_state() {
    let (server, provider) = setup().await;
    rest("PATCH", &format!("/pulls/{NUMBER}"))
        .and(body_json(json!({ "state": "closed" })))
        .respond_with(ok_json(pull_fixture()))
        .expect(1)
        .mount(&server)
        .await;
    rest("PATCH", &format!("/pulls/{NUMBER}"))
        .and(body_json(json!({ "state": "open" })))
        .respond_with(ok_json(pull_fixture()))
        .expect(1)
        .mount(&server)
        .await;

    provider.close(REPO, NUMBER).await.expect("close");
    provider.reopen(REPO, NUMBER).await.expect("reopen");
}

#[tokio::test]
async fn update_title_description_patches_title_and_body() {
    let (server, provider) = setup().await;
    rest("PATCH", &format!("/pulls/{NUMBER}"))
        .and(body_json(json!({ "title": "New title", "body": "New body" })))
        .respond_with(ok_json(pull_fixture()))
        .expect(1)
        .mount(&server)
        .await;

    provider.update_title_description(REPO, NUMBER, "New title", "New body").await.expect("edit");
}

#[tokio::test]
async fn set_draft_false_marks_ready_for_review() {
    let (server, provider) = setup().await;
    mount_pull(&server, pull_fixture()).await;
    graphql()
        .and(body_string_contains("markPullRequestReadyForReview(input: { pullRequestId: $id })"))
        .and(body_partial_json(json!({ "variables": { "id": PULL_NODE_ID } })))
        .respond_with(ok_json(json!({ "data": { "markPullRequestReadyForReview": { "pullRequest": { "id": PULL_NODE_ID } } } })))
        .expect(1)
        .mount(&server)
        .await;

    provider.set_draft(REPO, NUMBER, false).await.expect("ready for review");
}

#[tokio::test]
async fn set_draft_true_converts_to_draft() {
    let (server, provider) = setup().await;
    let mut pull = pull_fixture();
    pull["draft"] = Value::Bool(false);
    mount_pull(&server, pull).await;
    graphql()
        .and(body_string_contains("convertPullRequestToDraft(input: { pullRequestId: $id })"))
        .and(body_partial_json(json!({ "variables": { "id": PULL_NODE_ID } })))
        .respond_with(ok_json(json!({ "data": { "convertPullRequestToDraft": { "pullRequest": { "id": PULL_NODE_ID } } } })))
        .expect(1)
        .mount(&server)
        .await;

    provider.set_draft(REPO, NUMBER, true).await.expect("convert to draft");
}

#[tokio::test]
async fn set_draft_to_current_state_sends_no_mutation() {
    let (server, provider) = setup().await;
    mount_pull(&server, pull_fixture()).await;
    graphql().respond_with(ok_json(json!({ "data": {} }))).expect(0).mount(&server).await;

    provider.set_draft(REPO, NUMBER, true).await.expect("already draft");
}

#[tokio::test]
async fn graphql_errors_surface_as_status_errors() {
    let (server, provider) = setup().await;
    graphql()
        .respond_with(ok_json(json!({ "data": null, "errors": [{ "type": "FORBIDDEN", "message": "Resource not accessible" }] })))
        .expect(1)
        .mount(&server)
        .await;

    let result = provider.set_thread_status(REPO, NUMBER, "PRRT_x", ThreadStatus::Resolved).await;
    assert!(matches!(result, Err(ProviderError::Status { status: 403, .. })), "{result:?}");
}
