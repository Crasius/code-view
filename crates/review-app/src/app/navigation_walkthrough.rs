//! Walks the breadcrumb links, back and forward (Alt+Left/Right, the mouse's side buttons, the title
//! bar's arrows, across pages and pull request tabs, and macOS's Cmd+arrows) and the Linked card on
//! a real pull request page, offline:
//! a local git repository is the remote and a scripted provider describes the pull request and
//! answers the linked items. No network, no tokens. Runs with `cargo test`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use egui::{Event, Key, Modifiers, PointerButton, Role, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{
    Capabilities, Connection, GitRemote, ItemKind, ItemRef, LinkedItem, LinkedState, MergeStatus,
    ProviderKind, PullRequestDetail, PullRequestState, PullRequestSummary, Result, RunStatus,
};
use review_store::AppPaths;

use super::ReviewApp;
use super::scripted_platforms::{Origin, origin};
use super::walkthrough::{app_with, saved_paths, settle_background, wait_until};
use cv_base::platform;
use cv_runtime::route::{PullRequestKey, PullRequestTab, Route};
use cv_testkit::provider::{Script, Scripted};
use cv_testkit::{Clicks, ScratchDir, samples};

const CONNECTION_ID: &str = "local-navigation";
const CONNECTION_NAME: &str = "Acme GitHub";
const REPOSITORY: &str = "acme/widgets";
const NUMBER: u64 = 7;

const DESCRIPTION: &str = "Builds every widget with its final value right away.\n\n\
Follows #59 and needs acme/infra#3 first. Fixes #60.\n\n\
Tracked in [WEB-12](https://jira.example.com/browse/WEB-12); the follow-up is WEB-14.\n\n\
Upstream: https://github.com/acme/widgets/pull/61";

#[test]
fn navigation_breadcrumbs_back_and_linked_items() {
    let scratch = ScratchDir::new("navigation");
    let origin = origin(&scratch);
    let linked_calls = Arc::new(AtomicUsize::new(0));
    let mut harness = harness(saved_paths(&scratch, &[connection()]), origin, linked_calls.clone());

    // From outside (a link from another app): the trail is the pull request's structure.
    harness.state_mut().navigator.jump(Route::PullRequest(key(NUMBER)));
    wait_until(&mut harness, "the linked states", |harness| {
        harness.query_by_label_contains("Speed up login").is_some()
    });

    // The trail links every level above the page; the page itself is no link.
    for crumb in [review_core::APP_NAME, CONNECTION_NAME, REPOSITORY] {
        assert!(harness.query_by_role_and_label(Role::Link, crumb).is_some(), "{crumb} is a link");
    }
    assert!(harness.query_by_label("Pull request #7 (current page)").is_some());
    assert!(
        harness.query_by_label_contains("Back to My pull requests (").is_some(),
        "the back arrow names the list"
    );
    let forward = harness.get_by_label_contains("Forward (");
    assert!(forward.accesskit_node().is_disabled(), "nothing to go forward to yet");

    // One batch answered every reference; the tracker key without a link got the tracker's link.
    assert_eq!(linked_calls.load(Ordering::SeqCst), 1, "one linked_items call for the whole card");
    for row in [
        "#59 Speed up login (Merged)",
        "#60 Login is slow (Open)",
        "acme/infra#3 Provision cache (Draft)",
        "#61 Upstream widgets (Active)",
    ] {
        assert!(harness.query_by_label(row).is_some(), "row {row}");
    }
    for ticket in ["WEB-12", "WEB-14"] {
        assert!(harness.query_by_role_and_label(Role::Link, ticket).is_some(), "ticket {ticket}");
    }

    // A plain click on a pull request row opens it in the app.
    harness.click_at(harness.get_by_label("#59 Speed up login (Merged)").rect());
    wait_until(&mut harness, "pull request 59", |harness| {
        harness.query_by_label("Pull request #59 (current page)").is_some()
    });
    assert_eq!(harness.state().route, Route::PullRequest(key(59)));

    // Alt+Left goes back to the first pull request; its states come from the cache.
    harness.key_press_modifiers(Modifiers::ALT, egui::Key::ArrowLeft);
    wait_until(&mut harness, "pull request 7 again", |harness| {
        harness.query_by_label("Pull request #7 (current page)").is_some()
    });
    assert_eq!(harness.state().route, Route::PullRequest(key(NUMBER)));
    for _ in 0..120 {
        harness.step();
    }
    assert_eq!(
        linked_calls.load(Ordering::SeqCst),
        1,
        "revisiting asks nothing new and nothing polls"
    );

    // Every crumb leads to its level.
    harness.click_at(harness.get_by_role_and_label(Role::Link, REPOSITORY).rect());
    settle(&mut harness);
    assert_eq!(
        harness.state().route,
        Route::RepositoryPullRequests {
            connection_id: CONNECTION_ID.to_owned(),
            repository_key: REPOSITORY.to_owned()
        }
    );
    harness.click_at(harness.get_by_role_and_label(Role::Link, CONNECTION_NAME).rect());
    settle(&mut harness);
    assert_eq!(
        harness.state().route,
        Route::Connection { connection_id: CONNECTION_ID.to_owned() }
    );
    harness.click_at(harness.get_by_role_and_label(Role::Link, review_core::APP_NAME).rect());
    settle(&mut harness);
    assert_eq!(harness.state().route, Route::MyPullRequests);
    assert!(
        harness.query_by_role_and_label(Role::Link, review_core::APP_NAME).is_none(),
        "the root is the current page now"
    );

    back_and_forward_through_tabs(&mut harness);
}

/// From the list: a pull request's tabs are steps of the history like pages. The mouse's side
/// buttons, Alt+Left/Right and the bar's arrows walk them; a new step clears forward.
fn back_and_forward_through_tabs(harness: &mut Harness<'_, ReviewApp>) {
    let pull_request = Route::PullRequest(key(NUMBER));
    harness.state_mut().navigator.go(pull_request.clone());
    wait_until(harness, "the pull request again", |harness| tab_node(harness, "Files").is_some());
    settle_background(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Overview)));
    click_tab(harness, "Files");
    click_tab(harness, "Pipelines");
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Pipelines)));

    // A side button click is a press and a release: one step, over the page's content.
    side_button(harness, PointerButton::Extra1);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Files)));
    harness.key_press_modifiers(Modifiers::ALT, egui::Key::ArrowLeft);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Overview)));
    harness.key_press_modifiers(Modifiers::ALT, egui::Key::ArrowLeft);
    settle(harness);
    assert_eq!(place(harness), (Route::MyPullRequests, None));

    // Forward retraces the steps, tab by tab.
    side_button(harness, PointerButton::Extra2);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Overview)));
    harness.key_press_modifiers(Modifiers::ALT, egui::Key::ArrowRight);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Files)));
    assert!(
        harness.query_by_label_contains("Forward to Pull request #7, Pipelines tab (").is_some(),
        "the forward arrow names the tab it leads to"
    );

    // A new step clears forward: Alt+Right goes nowhere, the arrow rests disabled.
    click_tab(harness, "Commits");
    harness.key_press_modifiers(Modifiers::ALT, egui::Key::ArrowRight);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Commits)));
    let forward = harness.get_by_label_contains("Forward (");
    assert!(forward.accesskit_node().is_disabled(), "forward was cleared");

    // The bar's back arrow leads where its tooltip says.
    harness.click_at(harness.get_by_label_contains("Back to Pull request #7, Files tab (").rect());
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Files)));

    command_arrows_on_macos(harness, &pull_request);
}

/// On macOS Cmd+Right and Cmd+Left step like Chrome, except in a text field, where they move to
/// the line's ends. The arrows name the system's shortcut, as every other shortcut label does.
fn command_arrows_on_macos(harness: &mut Harness<'_, ReviewApp>, pull_request: &Route) {
    let expected =
        if cfg!(target_os = "macos") { ("⌘[", "⌘]") } else { ("Alt+Left", "Alt+Right") };
    assert_eq!((platform::BACK_SHORTCUT, platform::FORWARD_SHORTCUT), expected);
    let back_label = format!("Back to Pull request #7, Overview tab ({})", platform::BACK_SHORTCUT);
    assert!(harness.query_by_label(&back_label).is_some(), "the back arrow names {back_label}");
    let forward_label =
        format!("Forward to Pull request #7, Commits tab ({})", platform::FORWARD_SHORTCUT);
    assert!(harness.query_by_label(&forward_label).is_some(), "the forward arrow: {forward_label}");

    harness.ctx.set_os(egui::os::OperatingSystem::Mac);
    // What egui-winit reports for ⌘ on macOS: the Command key, which is also egui's `command`.
    let command = Modifiers::MAC_CMD | Modifiers::COMMAND;
    harness.key_press_modifiers(command, Key::ArrowRight);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Commits)));
    harness.key_press_modifiers(command, Key::ArrowLeft);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Files)));

    // The search box has the keys: Cmd+arrows and Option+arrows stay in it.
    harness.key_press_modifiers(command, Key::K);
    settle(harness);
    assert!(harness.ctx.text_edit_focused(), "the search box takes the keys");
    for modifiers in [command, Modifiers::ALT] {
        for arrow in [Key::ArrowLeft, Key::ArrowRight] {
            harness.key_press_modifiers(modifiers, arrow);
            settle(harness);
            assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Files)));
        }
    }
    harness.key_press(Key::Escape);
    settle(harness);
    assert!(!harness.ctx.text_edit_focused(), "Esc closed the search");
    harness.key_press_modifiers(command, Key::ArrowLeft);
    settle(harness);
    assert_eq!(place(harness), (pull_request.clone(), Some(PullRequestTab::Overview)));
}

/// The route and the pull request tab the app shows.
fn place(harness: &Harness<'_, ReviewApp>) -> (Route, Option<PullRequestTab>) {
    let app = harness.state();
    (app.route.clone(), app.current_tab())
}

fn tab_node<'a>(harness: &'a Harness<'_, ReviewApp>, name: &str) -> Option<egui_kittest::Node<'a>> {
    harness.query_all_by_role(Role::Tab).find(|node| {
        node.accesskit_node().label().is_some_and(|label| label.split(" (").next() == Some(name))
    })
}

fn click_tab(harness: &mut Harness<'_, ReviewApp>, name: &str) {
    harness.click_at(tab_node(harness, name).unwrap_or_else(|| panic!("the {name} tab")).rect());
    settle(harness);
}

/// Presses and releases a mouse side button in the middle of the page.
fn side_button(harness: &mut Harness<'_, ReviewApp>, button: PointerButton) {
    let pos = pos2(720.0, 500.0);
    harness.event(Event::PointerMoved(pos));
    for pressed in [true, false] {
        harness.event(Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE });
        harness.step();
    }
    settle(harness);
}

fn key(number: u64) -> PullRequestKey {
    PullRequestKey {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number,
    }
}

fn harness(
    paths: AppPaths,
    origin: Origin,
    linked_calls: Arc<AtomicUsize>,
) -> Harness<'static, ReviewApp> {
    let builder = Harness::builder().with_size([1440.0, 900.0]).with_step_dt(1.0 / 60.0);
    app_with(builder, paths, move |_, _| {
        Arc::new(Scripted(NavigationScript {
            origin: origin.clone(),
            linked_calls: linked_calls.clone(),
        }))
    })
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..20 {
        harness.step();
    }
}

fn connection() -> Connection {
    Connection {
        name: CONNECTION_NAME.to_owned(),
        ..samples::connection(
            CONNECTION_ID,
            ProviderKind::GitHub,
            ProviderKind::GitHub.default_base_url(),
        )
    }
}

/// What the scripted platform knows about the referenced numbers.
fn linked_item(reference: &ItemRef) -> Option<LinkedItem> {
    let (kind, title, state, label, checks) =
        match (reference.repository_key.as_str(), reference.number) {
            (REPOSITORY, 59) => (
                ItemKind::PullRequest,
                "Speed up login",
                LinkedState::Merged,
                "Merged",
                Some(RunStatus::Succeeded),
            ),
            (REPOSITORY, 60) => (ItemKind::Issue, "Login is slow", LinkedState::Open, "Open", None),
            (REPOSITORY, 61) => (
                ItemKind::PullRequest,
                "Upstream widgets",
                LinkedState::Open,
                "Active",
                Some(RunStatus::Failed),
            ),
            ("acme/infra", 3) => (
                ItemKind::PullRequest,
                "Provision cache",
                LinkedState::Draft,
                "Draft",
                Some(RunStatus::Running),
            ),
            _ => return None,
        };
    let path = if kind == ItemKind::PullRequest { "pull" } else { "issues" };
    Some(LinkedItem {
        reference: reference.clone(),
        kind,
        repository_key: reference.repository_key.clone(),
        title: title.to_owned(),
        state,
        state_label: label.to_owned(),
        checks,
        item_type: None,
        web_url: format!(
            "https://github.com/{}/{path}/{}",
            reference.repository_key, reference.number
        ),
    })
}

/// Describes the pull requests from the origin and counts the linked items calls.
struct NavigationScript {
    origin: Origin,
    linked_calls: Arc<AtomicUsize>,
}

static CAPABILITIES: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    running_logs: review_core::RunningLogs::Growing,
    ..cv_testkit::provider::CAPABILITIES.clone()
});

#[async_trait]
impl Script for NavigationScript {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn git_remote(&self, _repo: &str) -> Result<GitRemote> {
        Ok(GitRemote { clone_url: self.origin.clone_url.clone(), authorization_header: None })
    }

    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        let created_at = time::macros::datetime!(2026-10-01 09:30 UTC);
        let (title, description) = match number {
            NUMBER => ("Build widgets eagerly", DESCRIPTION),
            _ => ("Speed up login", "Caches the session lookup."),
        };
        Ok(PullRequestDetail {
            summary: PullRequestSummary {
                repository_key: repo.to_owned(),
                repository_name: "widgets".to_owned(),
                number,
                title: title.to_owned(),
                author: samples::author(),
                state: PullRequestState::Open,
                is_draft: false,
                source_branch: "feature".to_owned(),
                target_branch: "main".to_owned(),
                created_at,
                updated_at: None,
                web_url: format!("https://github.com/{repo}/pull/{number}"),
                labels: Vec::new(),
                reviewers: Vec::new(),
                checks: None,
                diff_stat: review_core::DiffStat::default(),
                head_sha: None,
                target_sha: None,
            },
            description: description.to_owned(),
            target_sha: self.origin.target.clone(),
            head_sha: self.origin.head.clone(),
            merge_status: MergeStatus::Mergeable,
            git_fetch_refs: vec!["refs/heads/feature".to_owned(), "refs/heads/main".to_owned()],
            closed_at: None,
            auto_complete: None,
        })
    }

    async fn linked_items(&self, references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
        self.linked_calls.fetch_add(1, Ordering::SeqCst);
        Ok(references.iter().filter_map(linked_item).collect())
    }
}
