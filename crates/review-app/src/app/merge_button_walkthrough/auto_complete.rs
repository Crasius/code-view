//! Auto-complete from the merge button on each platform, against the scripted provider of the
//! parent walkthrough: the blocked pull request (3) offers it in the platform's words, the confirm
//! sets it with the chosen strategy and the reviewed head, the overview names who set it and the
//! button cancels it; the mergeable one (1) keeps the plain merge. A refusal leads to the
//! platform's merge settings.
//! `cargo test -p review-app merge_button`

use std::sync::Arc;

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{MergeStrategy, ProviderKind};

use super::super::clicks::Clicks;
use super::{
    Calls, POLICIES, ReviewApp, button, has, open, opened_urls, press, start_on, wait_for,
};

/// (set, armed set, state line, cancel, armed cancel) as the platform words them.
struct Words {
    set: &'static str,
    confirm_set: &'static str,
    state: &'static str,
    cancel: &'static str,
    confirm_cancel: &'static str,
}

const AZURE: Words = Words {
    set: "Set auto-complete",
    confirm_set: "Click again to set auto-complete",
    state: "Auto-complete set by Ada Lovelace: Squash merge",
    cancel: "Cancel auto-complete",
    confirm_cancel: "Click again to cancel auto-complete",
};

const GITHUB: Words = Words {
    set: "Enable auto-merge",
    confirm_set: "Click again to enable auto-merge",
    state: "Auto-merge enabled by Ada Lovelace: Squash and merge",
    cancel: "Disable auto-merge",
    confirm_cancel: "Click again to disable auto-merge",
};

const GITLAB: Words = Words {
    set: "Set to auto-merge",
    confirm_set: "Click again to set to auto-merge",
    state: "Auto-merge turned on by Ada Lovelace: Squash and merge",
    cancel: "Cancel auto-merge",
    confirm_cancel: "Click again to cancel auto-merge",
};

#[test]
fn merge_button_sets_and_cancels_auto_complete_on_azure_devops() {
    round_trip("auto-ado", ProviderKind::AzureDevOps, &AZURE);
}

#[test]
fn merge_button_enables_and_disables_auto_merge_on_github() {
    let (mut harness, _) = round_trip("auto-github", ProviderKind::GitHub, &GITHUB);
    press(&mut harness, super::OPTIONS);
    assert!(
        harness.query_by_label("Delete feature/blocked after merging").is_none(),
        "GitHub's auto-merge takes no branch deletion"
    );
    assert!(has(&harness, "Merge automatically when requirements are met"));
}

#[test]
fn merge_button_sets_and_cancels_auto_merge_on_gitlab() {
    round_trip("auto-gitlab", ProviderKind::GitLab, &GITLAB);
}

/// A platform that refuses auto-merge (a GitHub repository without it) leads to its settings.
#[test]
fn a_refused_auto_merge_offers_the_merge_settings() {
    let refusing = Calls { refuses_auto_complete: true, ..Calls::default() };
    let (mut harness, _, calls) = start_on("auto-refused", ProviderKind::GitHub, refusing);
    open(&mut harness, "auto-refused", 3);
    wait_for(&mut harness, "auto-merge offered", |harness| button(harness, GITHUB.set));
    press(&mut harness, GITHUB.set);
    press(&mut harness, GITHUB.confirm_set);
    wait_for(&mut harness, "the refusal", |harness| {
        has(harness, "Enable auto-merge failed: Auto-merge is turned off for contoso/site.")
    });
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Repository settings").rect());
    harness.step();
    assert_eq!(opened_urls(&harness), [POLICIES], "the way to the merge settings");
    assert!(calls.auto_completes.lock().unwrap().is_empty());
}

/// On `kind`: the mergeable pull request keeps the plain merge; the blocked one sets auto-complete
/// with the preselected squash, shows who set it, and cancels it again.
fn round_trip(
    name: &str,
    kind: ProviderKind,
    words: &Words,
) -> (Harness<'static, ReviewApp>, Arc<Calls>) {
    let (mut harness, _, calls) = start_on(name, kind, Calls::default());
    open(&mut harness, name, 1);
    wait_for(&mut harness, "the mergeable pull request", |harness| {
        button(harness, squash_label(kind)) && has(harness, "Speed up login")
    });
    assert!(!button(&harness, words.set), "a completable pull request merges now");

    open(&mut harness, name, 3);
    wait_for(&mut harness, "auto-complete offered", |harness| {
        button(harness, words.set) && has(harness, "Blocked change")
    });
    press(&mut harness, words.set);
    assert!(button(&harness, words.confirm_set), "armed in place");
    assert!(calls.auto_completes.lock().unwrap().is_empty(), "one click sets nothing");
    press(&mut harness, words.confirm_set);
    wait_for(&mut harness, "auto-complete set", |_| {
        !calls.auto_completes.lock().unwrap().is_empty()
    });
    let set = calls.auto_completes.lock().unwrap()[0].clone().expect("a set, not a cancel");
    assert_eq!(set.strategy, MergeStrategy::Squash, "the preselected strategy");
    assert_eq!(set.head_sha, "a".repeat(40), "guarded by the reviewed head");
    assert!(calls.merges.lock().unwrap().is_empty(), "nothing merged now");

    wait_for(&mut harness, "the state and the cancel", |harness| {
        button(harness, words.cancel) && has(harness, words.state)
    });
    press(&mut harness, words.cancel);
    press(&mut harness, words.confirm_cancel);
    wait_for(&mut harness, "auto-complete cancelled", |harness| {
        calls.auto_completes.lock().unwrap().len() == 2 && button(harness, words.set)
    });
    assert_eq!(calls.auto_completes.lock().unwrap()[1], None, "the second call cancels");
    assert!(!has(&harness, words.state), "the state is gone");
    (harness, calls)
}

/// The squash button's label on `kind`.
fn squash_label(kind: ProviderKind) -> &'static str {
    if kind == ProviderKind::AzureDevOps { "Squash merge" } else { "Squash and merge" }
}
