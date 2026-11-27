//! Every writing feature of the app against the real hosts, in the repositories the owner released
//! for it ([`hosts::Released`]), signed in with the owner's saved connection. Each walkthrough opens
//! a pull request of its own between two new branches ([`hosts::TestPullRequest`]), drives the real
//! `ReviewApp` through its accessibility tree like a user (a line comment, a reply, an edit,
//! resolve and reopen, a general comment, the vote, a pipeline rerun where the repository has a
//! pipeline; on GitHub, which refuses the author's review, the proof that the vote is off and none
//! goes out), proves the merge guard with a commit pushed after loading, merges, and then leaves
//! nothing behind. Writes to real accounts, so ignored by default:
//!
//! `cargo test -p review-app live_writes -- --ignored --nocapture`
//!
//! The repositories come from `live-writes.json` in the owner's config folder
//! ([`hosts::TARGETS_FILE`]), never from the source; a run without it fails and names the file
//! and its shape.
//!
//! The app's own folders are a scratch folder; only the connection entry is read from the owner's
//! connections file, its token still protected by the system's store
//! ([`cv_testkit::allow_system_token_vault`]). Setup outside the app goes through the
//! owner's logged-in CLIs.

mod hosts;

use std::time::{Duration, Instant};

use egui::accesskit::Toggled;
use egui::{Rect, Role};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, Node};
use review_core::ProviderKind;
use review_store::{AppPaths, TokenVault};
use serde_json::{Value, json};

use self::hosts::{FILE, Released, State, TestPullRequest};
use super::ReviewApp;
use super::walkthrough::{
    self, files_tab, has, hover_until_add_comment, set_focus, settle_background, wheel,
};
use cv_runtime::route::{PullRequestKey, Route};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const NETWORK_TIMEOUT: Duration = Duration::from_secs(300);
const LINE_COMMENT: &str = "Live check: does the first line read well?";
const REPLY: &str = "Live check: a reply to the line comment.";
const EDITED: &str = " (edited)";
const GENERAL_COMMENT: &str = "Live check: a comment on the whole pull request.";
/// The app's reason for turning the votes off on the author's own GitHub pull request.
const AUTHOR_CANNOT_VOTE: &str = "GitHub does not let authors approve their own pull request";
/// The vote menu's entries on GitHub.
const VOTE_ENTRIES: [&str; 3] = ["Approve", "Approve with comment…", "Request changes…"];
/// The failure the walkthrough causes on purpose, as the start of a toast's first line: the
/// guarded merge.
const EXPECTED_FAILURES: [&str; 1] = ["New commits arrived"];

#[test]
#[ignore = "writes to the owner's Azure DevOps project; run explicitly"]
fn live_writes_on_azure_devops() {
    walkthrough(&Released::load(ProviderKind::AzureDevOps));
}

#[test]
#[ignore = "writes to the owner's GitHub repository; run explicitly"]
fn live_writes_on_github() {
    walkthrough(&Released::load(ProviderKind::GitHub));
}

#[test]
#[ignore = "writes to the owner's GitLab project; run explicitly"]
fn live_writes_on_gitlab() {
    walkthrough(&Released::load(ProviderKind::GitLab));
}

fn walkthrough(released: &Released) {
    let scratch = ScratchDir::new(&format!("live-writes-{}", released.connection_id));
    let paths = released_paths(&scratch, released);
    let pull_request = TestPullRequest::open(released, scratch.path());
    let _writes = WritesReport(released.connection_id.clone());
    if released.has_pipeline() {
        pull_request.wait_for_failed_pipeline();
    }
    let builder = Harness::builder()
        .with_size([1440.0, 900.0])
        .with_step_dt(1.0 / 60.0)
        // Real hosts' text may hold characters no font of the app draws.
        .allow_missing_glyphs();
    let mut harness = super::startup::start(builder, move |creation| {
        ReviewApp::new(creation, paths, cv_providers::create_provider)
    });
    harness.state_mut().navigator.go(Route::PullRequest(PullRequestKey {
        connection_id: released.connection_id.clone(),
        repository_key: released.repository_key.clone(),
        number: pull_request.number,
    }));
    wait_until(&mut harness, "the pull request", |harness| {
        harness.query_by_role_and_label(Role::Tab, "Overview").is_some()
    });

    if released.kind == ProviderKind::GitHub {
        votes_off_for_the_author(&mut harness, released);
    } else {
        vote(&mut harness);
    }
    comment_on_a_line(&mut harness);
    tab(&mut harness, "Overview");
    reply_edit_resolve_and_reopen(&mut harness);
    general_comment(&mut harness);
    if released.has_pipeline() {
        rerun_the_failed_pipeline(&mut harness);
        tab(&mut harness, "Overview");
    }
    merge_guard_then_merge(&mut harness, &pull_request);
    println!("{} #{} merged through the app", released.repository_key, pull_request.number);
}

/// Prints the writes the app sent, by method and path template, when the walkthrough ends (a
/// failed one too): the evidence of what reached the host.
struct WritesReport(String);

impl Drop for WritesReport {
    fn drop(&mut self) {
        let traffic = review_core::http::traffic::of(&self.0).unwrap_or_default();
        println!("writes sent through {}:", self.0);
        for endpoint in traffic.endpoints.iter().filter(|endpoint| endpoint.method != "GET") {
            println!("    {:>3}  {} {}", endpoint.count, endpoint.method, endpoint.path);
        }
    }
}

/// The scratch folders, holding only the released connection as the owner saved it: its token
/// stays protected by the system's store, which these folders use.
fn released_paths(scratch: &ScratchDir, released: &Released) -> AppPaths {
    cv_testkit::allow_system_token_vault();
    let owner = AppPaths::from_environment().connections_file();
    let text = std::fs::read_to_string(&owner).expect("the owner's connections file");
    let saved: Value = serde_json::from_str(&text).expect("connections JSON");
    let entry = saved["connections"]
        .as_array()
        .and_then(|connections| {
            connections.iter().find(|stored| stored["id"] == released.connection_id)
        })
        .unwrap_or_else(|| panic!("the owner saved no connection {}", released.connection_id))
        .clone();
    released.assert_connection(&entry);
    let paths = scratch.app_paths().with_token_vault(TokenVault::System);
    std::fs::create_dir_all(paths.config_dir()).expect("the config folder");
    let file = json!({ "connections": [entry] });
    std::fs::write(paths.connections_file(), file.to_string()).expect("the connections file");
    paths
}

/// Opens [`FILE`] in the Files tab and comments on one of its lines.
fn comment_on_a_line(harness: &mut Harness<'_, ReviewApp>) {
    files_tab(harness);
    wait_until(harness, "the file tree", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, FILE).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, FILE).rect());
    let diff_label = format!("Diff of {FILE}");
    wait_until(harness, "the diff", |harness| {
        harness.query_all_by_label_contains(&diff_label).next().is_some()
    });
    let diff = harness.query_all_by_label_contains(&diff_label).next().expect("the diff").rect();
    hover_until_add_comment(harness, diff, &[0.5, 0.75, 0.25]);
    harness.click_at(harness.get_by_label("Add comment").rect());
    wait_until(harness, "the line composer", |harness| {
        harness.query_by_label_contains("Comment on line").is_some()
    });
    type_into(harness, "Leave a comment", LINE_COMMENT);
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Comment").rect());
    wait_for_toast(harness, "Comment added.");
}

/// In the overview: replies to the line comment, edits the reply, resolves the thread and
/// reopens it.
fn reply_edit_resolve_and_reopen(harness: &mut Harness<'_, ReviewApp>) {
    wait_until(harness, "the line comment's thread", |harness| {
        harness.query_by_label(LINE_COMMENT).is_some()
    });
    let comment = reveal(harness, LINE_COMMENT);
    click(harness, nearest_below(harness, Role::Button, "Reply", comment));
    type_into(harness, "Reply", REPLY);
    submit(harness);
    wait_for_toast(harness, "Reply posted.");

    reveal(harness, REPLY);
    wait_until(harness, "the reply's menu", |harness| menu_above(harness, REPLY).is_some());
    let menu = menu_above(harness, REPLY).expect("the reply's menu");
    click(harness, menu);
    wait_until(harness, "the comment menu", |harness| {
        harness.query_by_role_and_label(Role::MenuItem, "Edit").is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::MenuItem, "Edit").rect());
    wait_until(harness, "the edit composer", |harness| {
        harness.query_by_role_and_label(Role::Button, "Save").is_some()
    });
    text_field(harness, REPLY).focus();
    harness.step();
    text_field(harness, REPLY).type_text(EDITED);
    harness.step();
    submit(harness);
    wait_for_toast(harness, "Comment updated.");
    let edited = format!("{REPLY}{EDITED}");
    wait_until(harness, "the edited reply", |harness| harness.query_by_label(&edited).is_some());

    let comment = reveal(harness, LINE_COMMENT);
    click(harness, nearest_below(harness, Role::Button, "Resolve", comment));
    wait_for_toast(harness, "Thread resolved.");
    wait_until(harness, "Reopen", |harness| harness.query_by_label("Reopen").is_some());
    harness.get_by_label("Reopen").scroll_to_me();
    let window = harness.ctx.content_rect();
    wait_until(harness, "Reopen in the window", |harness| {
        harness.query_by_label("Reopen").is_some_and(|node| window.contains(node.rect().center()))
    });
    harness.click_at(harness.get_by_label("Reopen").rect());
    wait_for_toast(harness, "Thread reopened.");
}

/// The overview's own composer posts a comment on the whole pull request.
fn general_comment(harness: &mut Harness<'_, ReviewApp>) {
    type_into(harness, "Leave a comment", GENERAL_COMMENT);
    submit(harness);
    wait_for_toast(harness, "Comment posted.");
}

/// The header's Approve casts the vote.
fn vote(harness: &mut Harness<'_, ReviewApp>) {
    wait_until(harness, "Approve", |harness| {
        harness.query_all_by_label("Approve").next().is_some()
    });
    let approve =
        harness.query_all_by_label("Approve").next().expect("the header's Approve").rect();
    click(harness, approve);
    let answer = wait_for_label(harness, "the vote's answer", |label| {
        label.contains("Vote submitted.") || label.contains("Vote failed:")
    });
    println!("{answer}");
    assert!(answer.contains("Vote submitted."), "{answer}");
}

/// GitHub refuses every review by the author, and the walkthrough's pull request is the owner's
/// own: once GitHub said who the user is, Approve and every vote menu entry are off with the
/// reason in the tooltip, clicks on them do nothing, and no review goes out.
fn votes_off_for_the_author(harness: &mut Harness<'_, ReviewApp>, released: &Released) {
    wait_until(harness, "Approve off for the author", |harness| {
        let approve: Vec<bool> = harness
            .query_all_by_role_and_label(Role::Button, "Approve")
            .map(|node| node.accesskit_node().is_disabled())
            .collect();
        !approve.is_empty() && approve.iter().all(|is_off| *is_off)
    });
    // A banner arriving meanwhile moves the header, so the pointer follows the button.
    walkthrough::wait_for(
        harness,
        "the reason in the tooltip",
        |harness| has(harness, AUTHOR_CANNOT_VOTE).then_some(()),
        |harness| {
            let approve = harness.get_by_role_and_label(Role::Button, "Approve").rect();
            harness.hover_at(approve.center());
        },
    );
    assert!(has(harness, "Comments and replies still work"), "the tooltip says what still works");
    let approve = harness.get_by_role_and_label(Role::Button, "Approve").rect();
    click(harness, approve);
    for label in VOTE_ENTRIES {
        // A click in the menu closes it, so each entry gets a menu of its own.
        let caret = harness.get_by_role_and_label(Role::Button, "More votes").rect();
        click(harness, caret);
        wait_until(harness, "the vote menu", |harness| {
            harness.query_by_role_and_label(Role::MenuItem, label).is_some()
        });
        let entry = harness.get_by_role_and_label(Role::MenuItem, label);
        assert!(entry.accesskit_node().is_disabled(), "{label} is off for the author");
        let entry = entry.rect();
        click(harness, entry);
        if harness.query_by_role_and_label(Role::MenuItem, label).is_some() {
            harness.key_press(egui::Key::Escape);
            harness.step();
        }
    }
    settle_background(harness);
    assert!(
        harness.query_by_role_and_label(Role::Button, "Request changes").is_none(),
        "no composer opens"
    );
    let traffic = review_core::http::traffic::of(&released.connection_id)
        .expect("the walkthrough's connection sent requests");
    let reviews: Vec<_> = traffic
        .endpoints
        .iter()
        .filter(|endpoint| endpoint.method == "POST" && endpoint.path.ends_with("/reviews"))
        .collect();
    assert!(reviews.is_empty(), "no review request goes out: {reviews:?}");
    println!("Approve and the vote menu are off for the author; no review was sent");
}

/// The Pipelines tab's "Rerun failed" reruns the failing job.
fn rerun_the_failed_pipeline(harness: &mut Harness<'_, ReviewApp>) {
    tab(harness, "Pipelines");
    let rerun = |harness: &Harness<'_, ReviewApp>| {
        harness
            .query_all_by_role(Role::Button)
            .find(|node| {
                node.accesskit_node().label().is_some_and(|label| label.starts_with("Rerun failed"))
            })
            .map(|node| node.rect())
    };
    wait_until(harness, "Rerun failed", |harness| rerun(harness).is_some());
    let button = rerun(harness).expect("Rerun failed");
    click(harness, button);
    wait_until(harness, "the rerun's answer", |harness| {
        has(harness, "Rerun started") || has(harness, "reruns started") || has(harness, "Not sent")
    });
    assert!(!has(harness, "Not sent"), "the rerun was refused");
}

/// A commit pushed after the page loaded makes the merge fail with the head-moved message and
/// leaves the pull request open; Reload shows the commit, and the merge then goes through.
///
/// The live probe would notice the push within seconds and reload the page, the merge button with
/// it; an inactive window holds the probe back, so the merge goes out against the reviewed head
/// the way it does when the commit lands between two probes.
fn merge_guard_then_merge(harness: &mut Harness<'_, ReviewApp>, pull_request: &TestPullRequest) {
    let commits_before = commits_tab(harness);
    show_header(harness);
    wait_until(harness, "the merge button", |harness| merge_button(harness).is_some());
    set_focus(harness, false);
    pull_request.push_commit();
    merge(harness);
    wait_until(harness, "the merge's answer", |harness| {
        has(harness, "New commits arrived") || has(harness, "Pull request merged.")
    });
    let state = pull_request.state();
    assert!(
        has(harness, "New commits arrived"),
        "the merge against the reviewed head was not refused; the platform reports {state:?}"
    );
    assert_eq!(state, State::Open, "the guarded merge merged nothing");
    assert_eq!(commits_tab(harness), commits_before, "the page still shows the reviewed head");
    set_focus(harness, true);
    let reload = harness.get_by_role_and_label(Role::Button, "Reload").rect();
    click(harness, reload);
    wait_until(harness, "the pushed commit", |harness| commits_tab(harness) != commits_before);
    wait_until(harness, "the merge button again", |harness| merge_button(harness).is_some());
    merge(harness);
    wait_for_toast(harness, "Pull request merged.");
    pull_request.wait_for_state(State::Merged);
}

/// Arms the merge button and confirms it.
fn merge(harness: &mut Harness<'_, ReviewApp>) {
    let button = merge_button(harness).expect("the merge button");
    click(harness, button);
    wait_until(harness, "the armed merge button", |harness| {
        harness.query_all_by_label_contains("Click again to").next().is_some()
    });
    let armed = harness.query_all_by_label_contains("Click again to").next().expect("armed").rect();
    click(harness, armed);
}

/// The merge split button's main part: the button just left of "Merge options".
fn merge_button(harness: &Harness<'_, ReviewApp>) -> Option<Rect> {
    let options = harness.query_by_label("Merge options")?.rect();
    harness.query_all_by_role(Role::Button).map(|node| node.rect()).find(|rect| {
        (rect.right() - options.left()).abs() < 4.0
            && (rect.center().y - options.center().y).abs() < 4.0
    })
}

/// The "Commits (N)" tab's label.
fn commits_tab(harness: &Harness<'_, ReviewApp>) -> String {
    harness
        .query_all_by_role(Role::Tab)
        .filter_map(|node| node.accesskit_node().label())
        .find(|label| label.starts_with("Commits"))
        .unwrap_or_default()
}

/// Scrolls the overview back to its top: scrolled down, the page header slides away (it comes back
/// only from the top), and its buttons take no clicks.
fn show_header(harness: &mut Harness<'_, ReviewApp>) {
    let window = harness.ctx.content_rect();
    harness.hover_at(egui::pos2(500.0, window.center().y));
    harness.step();
    for _ in 0..10 {
        harness.event(wheel(4000.0, egui::TouchPhase::Move, egui::MouseWheelSource::Wheel));
        harness.step();
    }
}

fn tab(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    harness.click_at(harness.get_by_role_and_label(Role::Tab, label).rect());
    wait_until(harness, &format!("the {label} tab"), |harness| {
        harness
            .query_by_role_and_label(Role::Tab, label)
            .is_some_and(|tab| tab.accesskit_node().toggled() == Some(Toggled::True))
    });
}

/// Scrolls the overview until the text `text` shows inside the window; returns where it is.
fn reveal(harness: &mut Harness<'_, ReviewApp>, text: &str) -> Rect {
    let window = harness.ctx.content_rect();
    for _ in 0..60 {
        let shown = harness.query_by_label(text).map(|node| node.rect());
        if let Some(body) =
            shown.filter(|body| window.contains_rect(body.expand2(egui::vec2(0.0, 80.0))))
        {
            return body;
        }
        harness.hover_at(egui::pos2(500.0, window.center().y));
        harness.step();
        let delta =
            if shown.is_some_and(|body| body.top() < window.top() + 80.0) { 120.0 } else { -120.0 };
        harness.event(wheel(delta, egui::TouchPhase::Move, egui::MouseWheelSource::Wheel));
        for _ in 0..3 {
            harness.step();
        }
    }
    panic!("{text:?} never came into view; screen:\n{}", screen(harness));
}

/// The "Comment actions" button heading the comment reading `text`: only one's own comments have
/// it.
fn menu_above(harness: &Harness<'_, ReviewApp>, text: &str) -> Option<Rect> {
    let body = harness.query_by_label(text)?.rect();
    harness
        .query_all_by_label("Comment actions")
        .map(|node| node.rect())
        .filter(|menu| menu.top() <= body.top() && body.top() - menu.top() < 60.0)
        .max_by(|a, b| a.top().total_cmp(&b.top()))
}

/// The `role` labelled `label` closest below the top of `anchor`.
fn nearest_below(harness: &Harness<'_, ReviewApp>, role: Role, label: &str, anchor: Rect) -> Rect {
    harness
        .query_all_by_role_and_label(role, label)
        .map(|node| node.rect())
        .filter(|rect| rect.top() >= anchor.top())
        .min_by(|a, b| a.top().total_cmp(&b.top()))
        .unwrap_or_else(|| panic!("no {label} below {anchor:?}; screen:\n{}", screen(harness)))
}

/// Clicks at the middle of `rect` like a pointer, then runs the frames of the press, the release
/// and the accessibility tree showing what it did, as [`walkthrough::press`] does.
fn click(harness: &mut Harness<'_, ReviewApp>, rect: Rect) {
    harness.click_at(rect);
    harness.run_steps(4);
}

/// The multiline text field holding `text`.
fn text_field<'h>(harness: &'h Harness<'_, ReviewApp>, text: &'static str) -> Node<'h> {
    harness
        .query_by(move |node| {
            node.role() == Role::MultilineTextInput
                && node.value().is_some_and(|value| value == text)
        })
        .unwrap_or_else(|| panic!("no text field holds {text:?}; screen:\n{}", screen(harness)))
}

/// Ctrl+Enter in the focused composer posts or saves it.
fn submit(harness: &mut Harness<'_, ReviewApp>) {
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    harness.step();
}

/// Types into the text field whose hint starts with `placeholder`, scrolled into view first, and
/// returns once the field holds the text (see [`walkthrough::type_into`]).
fn type_into(harness: &mut Harness<'_, ReviewApp>, placeholder: &'static str, text: &str) {
    wait_until(harness, placeholder, |harness| hinted(harness, placeholder).is_some());
    hinted(harness, placeholder).expect("the field").scroll_to_me();
    harness.step();
    hinted(harness, placeholder).expect("the field").focus();
    harness.step();
    let field = hinted(harness, placeholder).expect("the field");
    field.type_text(text);
    // Found by its id: a field with text may drop its hint.
    let id = field.accesskit_node().id();
    wait_until(harness, &format!("{placeholder} holding {text:?}"), |harness| {
        harness
            .query_by(|node| node.id() == id)
            .is_some_and(|field| field.value().is_some_and(|value| value.contains(text)))
    });
}

/// The text field whose hint starts with `placeholder`.
fn hinted<'h>(harness: &'h Harness<'_, ReviewApp>, placeholder: &'static str) -> Option<Node<'h>> {
    harness
        .query_by(move |node| node.placeholder().is_some_and(|hint| hint.starts_with(placeholder)))
}

fn wait_for_toast(harness: &mut Harness<'_, ReviewApp>, text: &str) {
    wait_until(harness, text, |harness| has(harness, text));
    println!("{text}");
}

/// Waits for a label `is_wanted` accepts and returns it; toasts come and go.
fn wait_for_label(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_wanted: impl Fn(&str) -> bool,
) -> String {
    let started = Instant::now();
    loop {
        harness.step();
        let found = texts(harness).find(|text| is_wanted(text));
        if let Some(found) = found {
            return found;
        }
        assert!(
            started.elapsed() < NETWORK_TIMEOUT,
            "timed out waiting for {what}; screen:\n{}",
            screen(harness)
        );
        cv_testkit::let_threads_run(Duration::from_millis(20));
    }
}

fn wait_until(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    let started = Instant::now();
    loop {
        harness.step();
        if is_done(harness) {
            return;
        }
        // A failed write will not succeed by waiting; only the expected ones may show.
        let failed = texts(harness)
            .filter(|label| !EXPECTED_FAILURES.iter().any(|expected| label.contains(expected)))
            .find(|label| label.contains(" failed: ") || label.starts_with("Not sent:"));
        if let Some(failure) = failed {
            panic!(
                "a write failed while waiting for {what}: {failure}; screen:\n{}",
                screen(harness)
            );
        }
        assert!(
            started.elapsed() < NETWORK_TIMEOUT,
            "timed out waiting for {what}; screen:\n{}",
            screen(harness)
        );
        cv_testkit::let_threads_run(Duration::from_millis(20));
    }
}

/// Every text on screen: labels, and the values text widgets carry theirs in.
fn texts<'h>(harness: &'h Harness<'_, ReviewApp>) -> impl Iterator<Item = String> + 'h {
    harness
        .query_all_by(|_| true)
        .filter_map(|node| node.accesskit_node().label().or_else(|| node.accesskit_node().value()))
}

/// Every text on screen, for a failure message.
fn screen(harness: &Harness<'_, ReviewApp>) -> String {
    format!("{:?}", texts(harness).collect::<Vec<_>>())
}
