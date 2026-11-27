//! Review actions end to end: the real app with the real GitHub provider against a mocked GitHub
//! Enterprise host ([`super::github_mock`]), driven headless through its accessibility tree and
//! pointer events like a user. Each test asserts the request that reached the host. Runs with
//! `cargo test`; needs `git` on PATH, no network.

use egui::{Role, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_store::FilesStart;
use serde_json::{Value, json};

use super::ReviewApp;
use super::github_mock::{
    self, FILE, GitHubHost, NUMBER, REPOSITORY, THREAD_ID, THREAD_TEXT, open_diff,
    open_pull_request,
};
use super::walkthrough::{
    app, press, press_button, secondary_click, settle_background, shows_file_view,
    shows_folder_view, type_into, wait_until, wait_until_found,
};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

#[test]
fn action_line_comment_via_hover_button() {
    let scratch = ScratchDir::new("action-hover-comment");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    let diff = open_diff(&mut harness);

    // The "+" only exists while a line is hovered; move down the right (head) pane until it shows.
    let x = diff.left() + diff.width() * 0.75;
    let mut y = diff.top() + 2.0;
    while harness.query_by_label("Add comment").is_none() {
        assert!(
            y < diff.top() + 120.0,
            "no comment button appeared while hovering the first lines"
        );
        harness.hover_at(pos2(x, y));
        harness.step();
        y += 3.0;
    }
    harness.click_at(harness.get_by_label("Add comment").rect());
    wait_until(&mut harness, "the comment composer", |harness| {
        harness.query_by_label("Comment on line 1").is_some()
    });
    type_into(&mut harness, "Leave a comment", "Please keep the old name as an alias.");
    press_button(&mut harness, "Comment");

    let request = wait_for_request(
        &mut harness,
        &host,
        "POST",
        &format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/comments"),
    );
    assert_eq!(
        request,
        json!({ "body": "Please keep the old name as an alias.", "commit_id": host.head_sha(), "path": FILE, "line": 1, "side": "RIGHT" })
    );
    wait_until(&mut harness, "the success toast", |harness| {
        harness.query_by_label_contains("Comment added.").is_some()
    });
}

#[test]
fn action_line_comment_via_context_menu() {
    let scratch = ScratchDir::new("action-menu-comment");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    let diff = open_diff(&mut harness);

    // First row of the left (base) pane: the deleted line 1.
    let at = pos2(diff.left() + diff.width() * 0.25, diff.top() + 4.0);
    secondary_click(&mut harness, at);
    // The menu opens at the pointer; the hovered line's own "Add comment" sits in the gutter.
    let is_in_menu = |node: &egui_kittest::Node<'_>| node.rect().left() >= at.x - 1.0;
    wait_until(&mut harness, "the context menu", |harness| {
        harness.query_all_by_label("Add comment").any(|node| is_in_menu(&node))
    });
    harness.click_at(
        harness
            .query_all_by_label("Add comment")
            .find(is_in_menu)
            .expect("the menu's Add comment")
            .rect(),
    );
    wait_until(&mut harness, "the comment composer", |harness| {
        harness.query_by_label("Comment on line 1").is_some()
    });
    type_into(&mut harness, "Leave a comment", "Why was greet renamed?");
    press_button(&mut harness, "Comment");

    let request = wait_for_request(
        &mut harness,
        &host,
        "POST",
        &format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/comments"),
    );
    assert_eq!(
        request,
        json!({ "body": "Why was greet renamed?", "commit_id": host.head_sha(), "path": FILE, "line": 1, "side": "LEFT" })
    );
}

#[test]
fn action_reply_to_a_thread_in_the_overview() {
    let scratch = ScratchDir::new("action-reply");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    wait_until(&mut harness, "the existing thread", |harness| {
        harness.query_by_label(THREAD_TEXT).is_some()
    });
    press(&mut harness, "Reply");
    type_into(&mut harness, "Reply", "Yes, other crates use it.");
    press_button(&mut harness, "Reply");

    let request = wait_until_found(&mut harness, "the reply mutation", |_| {
        host.find_graphql("addPullRequestReviewThreadReply")
    });
    assert_eq!(
        request["variables"],
        json!({ "thread": THREAD_ID, "body": "Yes, other crates use it." })
    );
    wait_until(&mut harness, "the success toast", |harness| {
        harness.query_by_label_contains("Reply posted.").is_some()
    });
}

#[test]
fn action_approve_from_the_header() {
    let scratch = ScratchDir::new("action-approve");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    // The header's Approve comes first; the merge status card offers the same vote further down.
    wait_until(&mut harness, "Approve", |harness| {
        harness.query_all_by_label("Approve").next().is_some()
    });
    harness.click_at(
        harness.query_all_by_label("Approve").next().expect("the header's Approve").rect(),
    );
    harness.step();

    let request = wait_for_request(
        &mut harness,
        &host,
        "POST",
        &format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/reviews"),
    );
    assert_eq!(request, json!({ "event": "APPROVE" }));
}

/// After an approval the way often leads back to the list: it refreshes in the background at
/// once, so the rows are current before the user gets there.
#[test]
fn action_an_approval_refreshes_the_list_ahead() {
    let scratch = ScratchDir::new("action-approve-ahead");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.sign_in_as_me();
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    wait_until(&mut harness, "Approve", |harness| {
        harness.query_all_by_label("Approve").next().is_some()
    });
    harness.click_at(
        harness.query_all_by_label("Approve").next().expect("the header's Approve").rect(),
    );
    harness.step();

    let reviews = format!("/api/v3/repos/{REPOSITORY}/pulls/{NUMBER}/reviews");
    wait_until_found(&mut harness, "the list's refresh after the approval", |_| {
        let requests = host.requests();
        let vote = requests.iter().position(|request| {
            request.method.as_str() == "POST" && request.url.path() == reviews
        })?;
        requests[vote..].iter().any(is_listing).then_some(())
    });
    assert!(
        harness.query_by_role_and_label(Role::Tab, "Overview").is_some(),
        "the page stays where it was"
    );
}

/// The overview's listing: GitHub's search.
fn is_listing(request: &wiremock::Request) -> bool {
    request.url.path() == "/api/graphql"
        && request.body_json::<Value>().is_ok_and(|body| {
            body["query"].as_str().is_some_and(|query| query.contains("search(type: ISSUE"))
        })
}

#[test]
fn action_add_a_label_from_the_catalog() {
    let scratch = ScratchDir::new("action-labels");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    wait_until(&mut harness, "the current label", |harness| {
        harness.query_by_label("bug").is_some()
    });
    press(&mut harness, "Edit labels");
    wait_until(&mut harness, "the label catalog", |harness| {
        harness.query_by_label("enhancement").is_some()
    });
    press(&mut harness, "enhancement");
    // Picks collect in a draft; Apply sends them in one request.
    press(&mut harness, "Apply");

    let request = wait_for_request(
        &mut harness,
        &host,
        "PUT",
        &format!("/api/v3/repos/{REPOSITORY}/issues/{NUMBER}/labels"),
    );
    assert_eq!(request, json!({ "labels": ["bug", "enhancement"] }));
    assert!(
        host.find("GET", &format!("/api/v3/repos/{REPOSITORY}/labels")).is_some(),
        "the picker loaded the catalog"
    );
}

#[test]
fn action_my_pull_requests_lists_every_host() {
    let scratch = ScratchDir::new("action-hosts");
    let acme =
        GitHubHost::start("acme", &[github_mock::search_row(11, "Tidy the build", "acme/widgets")]);
    let globex = GitHubHost::start(
        "globex",
        &[github_mock::search_row(22, "Bump the parser", "globex/engine")],
    );
    let mut harness = app(
        &scratch,
        &[acme.connection("acme", "Acme"), globex.connection("globex", "Globex Enterprise")],
    );

    wait_until(&mut harness, "rows of both hosts", |harness| {
        harness.query_by_label("11 Tidy the build").is_some()
            && harness.query_by_label("22 Bump the parser").is_some()
    });
    for (host, owner) in [(&acme, "acme"), (&globex, "globex")] {
        let search = host.find_graphql("search(type: ISSUE").expect("a search request");
        let query = search["variables"]["q"].as_str().unwrap_or_default();
        assert!(
            query.contains(&format!("org:{owner}")),
            "{owner} searched with its own qualifier: {query}"
        );
        // The default "All" chip lists everything open in the organisation, not only the user's own.
        assert!(!query.contains("@me"), "{query}");
    }
}

/// The Files tab opens on the summary of every changed file; a card's View opens the file view.
/// The breadcrumb ends at the pull request, whose crumb leads back to the summary of all files.
#[test]
fn action_folder_summary_opens_the_file_view_and_the_trail_leads_back() {
    let scratch = ScratchDir::new("action-folder-summary");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    files_tab(&mut harness);
    let view = format!("View {FILE}");
    wait_until(&mut harness, "the summary card", |harness| {
        harness.query_by_label("1 changed file").is_some()
            && harness.query_by_label(&view).is_some()
    });
    // What was on its way when the files showed lands first: the card's code, and anything that
    // could open a file (a place to resume, the first file).
    settle_background(&mut harness);
    assert!(
        shows_folder_view(&harness)
            && harness.query_by_label(&view).is_some()
            && !shows_file_view(&harness, FILE),
        "no file opens by itself"
    );

    harness.click_at(harness.get_by_label(&view).rect());
    wait_until(&mut harness, "the file view", |harness| shows_file_view(harness, FILE));
    assert!(
        harness.query_by_label("lib.rs (current page)").is_none()
            && harness.query_by_role_and_label(Role::Link, "src").is_none(),
        "the trail names neither the folders nor the file"
    );

    harness.click_at(
        harness.get_by_role_and_label(Role::Link, &format!("Pull request #{NUMBER}")).rect(),
    );
    wait_until(&mut harness, "the summary of all files", |harness| {
        harness.query_by_label("All files (current folder)").is_some()
    });
}

/// Changed besides [`FILE`]: in a folder the tree lists first.
const FIRST_FILE: &str = "app/first.rs";

fn with_first_file(scratch: &ScratchDir) -> GitHubHost {
    let first = github_mock::ExtraFile {
        path: FIRST_FILE,
        base: b"pub fn first() {}\n".to_vec(),
        head: b"pub fn first() -> u32 {\n    1\n}\n".to_vec(),
    };
    GitHubHost::with_pull_request_files(scratch.path(), &[first])
}

fn files_tab(harness: &mut Harness<'_, ReviewApp>) {
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files"))
            })
            .rect(),
    );
}

/// With "First file" chosen, the Files tab opens on the first file of the tree straight away: the
/// folder view never shows on the way. The trail's pull request crumb still leads to the folder
/// view of all files, and a click on a folder in the tree to that folder's.
#[test]
fn action_files_tab_opens_on_the_first_file_when_chosen() {
    let scratch = ScratchDir::new("action-first-file");
    let host = with_first_file(&scratch);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().services.settings.files_start = FilesStart::FirstFile;
    open_pull_request(&mut harness);
    files_tab(&mut harness);
    wait_until(&mut harness, "the first file's diff", |harness| {
        assert!(!shows_folder_view(harness), "the folder view showed before the first file");
        shows_file_view(harness, FIRST_FILE)
    });
    assert!(
        harness.query_by_label("first.rs (current page)").is_none(),
        "the trail ends at the pull request"
    );

    harness.click_at(
        harness.get_by_role_and_label(Role::Link, &format!("Pull request #{NUMBER}")).rect(),
    );
    wait_until(&mut harness, "the folder view of all files", |harness| {
        harness.query_by_label("All files (current folder)").is_some()
            && harness.query_by_label("2 changed files").is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, "src").rect());
    wait_until(&mut harness, "the folder view of src", |harness| {
        harness.query_by_label("src (current folder)").is_some()
            && harness.query_by_label("1 changed file").is_some()
    });
}

/// A comment's link opens its own file, also when the Files tab would open on the first file.
#[test]
fn action_a_comment_link_wins_over_the_first_file() {
    let scratch = ScratchDir::new("action-first-file-link");
    let host = with_first_file(&scratch);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().services.settings.files_start = FilesStart::FirstFile;
    open_pull_request(&mut harness);
    let link = format!("{FILE}:");
    wait_until(&mut harness, "the comment's link", |harness| {
        harness.query_all_by_label_contains(&link).next().is_some()
    });
    harness.click_at(harness.query_all_by_label_contains(&link).next().expect("the link").rect());
    wait_until(&mut harness, "the commented file", |harness| {
        assert!(
            !shows_file_view(harness, FIRST_FILE),
            "the first file opened instead of the comment's"
        );
        shows_file_view(harness, FILE)
    });
}

/// Waits for the request and returns its JSON body.
fn wait_for_request(
    harness: &mut Harness<'_, ReviewApp>,
    host: &GitHubHost,
    verb: &str,
    request_path: &str,
) -> Value {
    let request = wait_until_found(harness, request_path, |_| host.find(verb, request_path));
    request.body_json().expect("a JSON body")
}
