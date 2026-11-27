//! The header's merge split button, offline: an Azure DevOps connection answered by a scripted
//! provider whose target branch allows merge, squash and rebase (squash the default), not
//! semi-linear. Pull request 1 is mergeable, 2 has conflicts, 3 is blocked by policies.
//!
//! Checks the in-place confirm (click twice, Enter, Esc, the 4 s window), the dropdown (number
//! keys, picking keeps it open, message per strategy, Ctrl+Enter arms), what gets merged and
//! remembered, and that an inactive window loads no policy. Auto-complete on each platform is in
//! [`auto_complete`].
//! `cargo test -p review-app merge_button`

mod auto_complete;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{
    AutoComplete, Capabilities, Connection, MergeOptions, MergePolicy, MergeStatus, MergeStrategy,
    ProviderError, ProviderKind, PullRequestDetail, PullRequestQuery, PullRequestSummary, Result,
    ReviewVote, UserIdentity,
};

use super::ReviewApp;
use super::walkthrough::{
    app_with, has, hover_until, opened_urls, press, pump, saved_paths, saved_settings, set_focus,
    wait_until,
};
use cv_runtime::route::{PullRequestKey, Route};
use cv_testkit::provider::{Script, Scripted};
use cv_testkit::{Clicks, ScratchDir, samples};

const REPOSITORY: &str = "contoso/Web/site";
const POLICIES: &str = "https://dev.azure.com/contoso/Web/_settings/repositories?_a=policiesMid";
const OPTIONS: &str = "Merge options";

/// Every merge a test's scripted provider was asked for, and how often for a policy.
#[derive(Default)]
struct Calls {
    merges: Mutex<Vec<MergeOptions>>,
    policies: AtomicUsize,
    /// Auto-complete set (with its options) and cancelled, in order.
    auto_completes: Mutex<Vec<Option<MergeOptions>>>,
    /// The auto-complete the blocked pull request (3) has now.
    auto_complete: Mutex<Option<AutoComplete>>,
    /// The platform refuses to set auto-complete, as GitHub does where a repository forbids it.
    refuses_auto_complete: bool,
}

#[test]
fn merge_button_confirms_in_place_and_remembers_the_choice() {
    let scratch = ScratchDir::new("merge-button-confirm");
    let (mut harness, calls) = start(&scratch, "confirm");
    open(&mut harness, "confirm", 1);
    wait_until(&mut harness, "the preselected default", |harness| button(harness, "Squash merge"));
    assert!(
        has(&harness, "Merge type: Squash merge"),
        "the overview card names what the button runs"
    );

    // A click arms the button in place; the window runs out after 4 s.
    press(&mut harness, "Squash merge");
    assert!(button(&harness, "Click again to squash merge"), "armed in place");
    assert!(calls.merges.lock().unwrap().is_empty(), "one click merges nothing");
    pump(&mut harness, 4.5);
    assert!(button(&harness, "Squash merge"), "the confirm window ran out");

    // Esc disarms.
    press(&mut harness, "Squash merge");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(button(&harness, "Squash merge"), "Esc disarms");

    // The dropdown: number keys and clicks pick, it stays open, the message follows.
    press(&mut harness, OPTIONS);
    assert!(
        radio(&harness, "Squash merge", true) && radio(&harness, "Merge commit", false),
        "the preselection is checked"
    );
    assert!(
        harness.query_by_label("Semi-linear merge").is_none(),
        "the disallowed strategy is not offered"
    );
    assert!(
        has(&harness, "Semi-linear merge isn't allowed on main."),
        "what the branch forbids is said"
    );
    assert!(has(&harness, "Squash commit message"));
    harness.key_press(egui::Key::Num3);
    harness.run_steps(3);
    assert!(radio(&harness, "Rebase and fast-forward", true), "3 picks the third strategy");
    assert!(button(&harness, "Rebase and fast-forward"), "the main part follows");
    assert!(!has(&harness, "commit message"), "rebase writes no commit of its own");
    harness.click_at(harness.get_by_role_and_label(Role::MenuItemRadio, "Merge commit").rect());
    harness.run_steps(3);
    assert!(
        has(&harness, "Merge commit message") && radio(&harness, "Merge commit", true),
        "a click picks and keeps it open"
    );
    harness.click_at(harness.get_by_role_and_label(Role::Link, "Branch policies").rect());
    harness.step();
    assert_eq!(opened_urls(&harness), [POLICIES], "Branch policies opens the platform's page");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(harness.query_by_role(Role::MenuItemRadio).is_none(), "Esc closes the dropdown");
    assert!(button(&harness, "Merge commit"), "the pick stays for this pull request");

    // Ctrl+Enter in the dropdown arms; Enter completes.
    press(&mut harness, OPTIONS);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    harness.run_steps(3);
    assert!(button(&harness, "Click again to merge"), "Ctrl+Enter arms the button");
    harness.key_press(egui::Key::Enter);
    wait_until(&mut harness, "the merge", |_| !calls.merges.lock().unwrap().is_empty());
    let merged = calls.merges.lock().unwrap()[0].clone();
    assert_eq!(merged.strategy, MergeStrategy::Merge);
    assert_eq!(merged.commit_message.as_deref(), Some("Merged PR 1: Speed up login"));
    assert!(merged.delete_source_branch);
    let saved = saved_settings(&harness);
    assert_eq!(
        saved.merge.last_used("confirm", REPOSITORY),
        Some(MergeStrategy::Merge),
        "the merge is remembered"
    );

    // A second click within the window completes; the branch choice is remembered per repository.
    // The mock still answers open; a reload by hand takes its word over the merge on the page.
    harness.key_press(egui::Key::F5);
    wait_until(&mut harness, "the reloaded button", |harness| button(harness, "Merge commit"));
    press(&mut harness, OPTIONS);
    harness.click_at(harness.get_by_label("Delete feature/login after merging").rect());
    harness.run_steps(3);
    harness.key_press(egui::Key::Num1);
    harness.run_steps(3);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    press(&mut harness, "Merge commit");
    press(&mut harness, "Click again to merge");
    wait_until(&mut harness, "the second merge", |_| calls.merges.lock().unwrap().len() == 2);
    assert!(!calls.merges.lock().unwrap()[1].delete_source_branch, "the unticked box is sent");
    let saved = saved_settings(&harness);
    assert_eq!(saved.merge.delete_source_branch("confirm", REPOSITORY), Some(false));
}

#[test]
fn merge_button_is_disabled_with_conflicts_and_warns_when_forced_past_policies() {
    let scratch = ScratchDir::new("merge-button-states");
    let (mut harness, calls) = start(&scratch, "states");
    open(&mut harness, "states", 2);
    wait_until(&mut harness, "the conflicted pull request", |harness| {
        harness.query_by_role_and_label(Role::Button, "Squash merge").is_some()
    });
    let main = harness.get_by_role_and_label(Role::Button, "Squash merge");
    assert!(main.accesskit_node().is_disabled(), "conflicts disable the button");
    hover_until(&mut harness, Role::Button, "Squash merge", "the tooltip's reason", |harness| {
        has(harness, "This pull request has merge conflicts")
    });
    // The overview card has its own; the tooltip opens above the page, its action comes last.
    harness.click_at(
        harness
            .query_all_by_label("Open in Azure DevOps")
            .last()
            .expect("the tooltip's action")
            .rect(),
    );
    harness.step();
    assert_eq!(opened_urls(&harness), [web_url(2)], "the tooltip's action opens the pull request");
    harness.event(egui::Event::PointerGone);

    // Blocked, the button sets auto-complete; unticked in the dropdown, it completes at once.
    open(&mut harness, "states", 3);
    wait_until(&mut harness, "the blocked pull request", |harness| {
        button(harness, "Set auto-complete") && has(harness, "Blocked change")
    });
    press(&mut harness, OPTIONS);
    harness.click_at(harness.get_by_label("Complete automatically when policies pass").rect());
    harness.run_steps(3);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(button(&harness, "Squash merge"), "unticked, the button completes now");
    press(&mut harness, "Squash merge");
    assert!(
        has(&harness, "Policies or required reviews still block this merge."),
        "armed, the block is said"
    );
    assert!(
        harness.query_all_by_label("Open in Azure DevOps").count() >= 1,
        "with the way to the platform"
    );
    assert!(calls.merges.lock().unwrap().is_empty());
    assert!(calls.auto_completes.lock().unwrap().is_empty(), "nothing set either");
}

#[test]
fn merge_button_loads_no_policy_while_the_window_is_inactive() {
    let scratch = ScratchDir::new("merge-button-inactive");
    let (mut harness, calls) = start(&scratch, "inactive");
    wait_until(&mut harness, "the list", |harness| has(harness, "Speed up login"));
    set_focus(&mut harness, false);
    open(&mut harness, "inactive", 1);
    wait_until(&mut harness, "the header", |harness| {
        has(harness, "Speed up login") && harness.query_all_by_role(Role::Tab).next().is_some()
    });
    pump(&mut harness, 3.0);
    assert_eq!(calls.policies.load(Ordering::SeqCst), 0, "no policy request while inactive");
    assert!(button(&harness, "Complete"), "the button waits with its neutral label");
    set_focus(&mut harness, true);
    wait_until(&mut harness, "the policy once active", |harness| button(harness, "Squash merge"));
    assert_eq!(calls.policies.load(Ordering::SeqCst), 1, "one request once active");
}

/// The app on `scratch` with one Azure DevOps connection with id `name`, and what its provider is
/// asked.
fn start(scratch: &ScratchDir, name: &str) -> (Harness<'static, ReviewApp>, Arc<Calls>) {
    start_on(scratch, name, ProviderKind::AzureDevOps, Calls::default())
}

/// [`start`] with a connection of `kind` whose provider answers with `calls`.
fn start_on(
    scratch: &ScratchDir,
    name: &str,
    kind: ProviderKind,
    calls: Calls,
) -> (Harness<'static, ReviewApp>, Arc<Calls>) {
    let calls = Arc::new(calls);
    let script_calls = calls.clone();
    let connection = Connection {
        name: "Contoso".to_owned(),
        organizations: vec!["contoso".to_owned()],
        token: Some("scripted".to_owned()),
        ..samples::connection(name, kind, "https://dev.azure.com/contoso")
    };
    let builder = Harness::builder().with_size([1280.0, 860.0]).with_step_dt(1.0 / 30.0);
    let harness = app_with(builder, saved_paths(scratch, &[connection]), move |_, _| {
        Arc::new(Scripted(MergeButtonScript { calls: script_calls.clone() }))
    });
    (harness, calls)
}

fn open(harness: &mut Harness<'_, ReviewApp>, connection_id: &str, number: u64) {
    let key = PullRequestKey {
        connection_id: connection_id.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number,
    };
    harness.state_mut().navigator.go(Route::PullRequest(key));
    harness.step();
}

/// A button labelled exactly `label`.
fn button(harness: &Harness<'_, ReviewApp>, label: &str) -> bool {
    harness.query_by_role_and_label(Role::Button, label).is_some()
}

/// The strategy row `label` exists and is checked as `is_checked`.
fn radio(harness: &Harness<'_, ReviewApp>, label: &str, is_checked: bool) -> bool {
    harness.query_by_role_and_label(Role::MenuItemRadio, label).is_some_and(|node| {
        let toggled = node.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True);
        toggled == is_checked
    })
}

fn web_url(number: u64) -> String {
    format!("https://dev.azure.com/contoso/Web/_git/site/pullrequest/{number}")
}

struct MergeButtonScript {
    calls: Arc<Calls>,
}

fn person(id: &str, name: &str) -> UserIdentity {
    UserIdentity {
        id: id.to_owned(),
        display_name: name.to_owned(),
        login: Some(name.to_lowercase()),
        avatar_url: None,
    }
}

/// (title, source branch, merge status) of pull requests 1 to 3.
fn change(number: u64) -> (&'static str, &'static str, MergeStatus) {
    match number {
        1 => ("Speed up login", "feature/login", MergeStatus::Mergeable),
        2 => ("Conflicting change", "feature/conflict", MergeStatus::Conflicts),
        _ => ("Blocked change", "feature/blocked", MergeStatus::Blocked),
    }
}

fn summary(number: u64) -> PullRequestSummary {
    let (title, source, _) = change(number);
    PullRequestSummary {
        title: title.to_owned(),
        author: person("1", "Ada Lovelace"),
        source_branch: source.to_owned(),
        updated_at: Some(time::macros::datetime!(2026-10-18 15:00 UTC)),
        web_url: web_url(number),
        ..samples::summary(REPOSITORY, number)
    }
}

static CAPABILITIES: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    votes: vec![ReviewVote::Approved, ReviewVote::None],
    merge_strategies: MergeStrategy::ALL.to_vec(),
    has_label_catalog: false,
    running_logs: review_core::RunningLogs::Growing,
});

#[async_trait]
impl Script for MergeButtonScript {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(person("1", "Ada Lovelace"))
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        Ok((1..=3).map(summary).collect())
    }

    async fn pull_request(&self, _repo: &str, number: u64) -> Result<PullRequestDetail> {
        Ok(PullRequestDetail {
            summary: summary(number),
            description: "Completes with the branch's merge types.".to_owned(),
            merge_status: change(number).2,
            auto_complete: if number == 3 {
                self.calls.auto_complete.lock().unwrap().clone()
            } else {
                None
            },
            ..samples::detail(REPOSITORY, number)
        })
    }

    async fn merge_policy(&self, _repo: &str, _target_branch: &str) -> Result<MergePolicy> {
        self.calls.policies.fetch_add(1, Ordering::SeqCst);
        let allowed = vec![MergeStrategy::Merge, MergeStrategy::Squash, MergeStrategy::Rebase];
        Ok(MergePolicy::reported(allowed, Some(MergeStrategy::Squash))
            .with_settings_url(Some(POLICIES.to_owned())))
    }

    async fn merge(&self, _repo: &str, _number: u64, options: &MergeOptions) -> Result<()> {
        self.calls.merges.lock().unwrap().push(options.clone());
        Ok(())
    }

    async fn set_auto_complete(
        &self,
        _repo: &str,
        _number: u64,
        options: &MergeOptions,
    ) -> Result<()> {
        if self.calls.refuses_auto_complete {
            return Err(ProviderError::Unsupported(
                "Auto-merge is turned off for contoso/site.".to_owned(),
            ));
        }
        self.calls.auto_completes.lock().unwrap().push(Some(options.clone()));
        *self.calls.auto_complete.lock().unwrap() = Some(AutoComplete {
            set_by: Some(person("1", "Ada Lovelace")),
            strategy: Some(options.strategy),
        });
        Ok(())
    }

    async fn cancel_auto_complete(&self, _repo: &str, _number: u64) -> Result<()> {
        self.calls.auto_completes.lock().unwrap().push(None);
        *self.calls.auto_complete.lock().unwrap() = None;
        Ok(())
    }
}
