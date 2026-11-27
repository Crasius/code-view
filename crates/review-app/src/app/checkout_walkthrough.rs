//! "Check out" end to end: the real app against the mocked GitHub host ([`super::github_mock`]),
//! with a clone of its repository in a scratch "dev" folder that Settings searches. Checks the
//! popover's choices and its button text, the progress and the title bar's chip, the ready split
//! button with its menu and the header chip, each card's way on (no clone, a clone with changes,
//! the branch checked out already, a fork, a failure, local commits), Settings › Local checkouts,
//! and that a new head of the pull request moves the checkout silently, or says why it did not.
//! Needs `git` on PATH, no network; terminals and agents open in the fake.
//! `cargo test -p review-app checkout_walkthrough`

use std::path::{Path, PathBuf};

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::AgentKind;
use review_store::AppSettings;

use super::ReviewApp;
use super::github_mock::{GitHubHost, NUMBER, REPOSITORY, open_pull_request};
use super::walkthrough::{app, press, wait_until};
use cv_runtime::route::Route;
use cv_services::agents::FakeCli;
use cv_testkit::Clicks;
use cv_testkit::{ScratchDir, git};

const POPOVER_TITLE: &str = "Check out #7 locally";

/// The host, the app on its pull request, and a clean clone of the repository in `dev/widgets`.
/// Bind every field: an unbound one drops at once, and with it the folder or the host.
pub(super) struct Setup {
    pub scratch: ScratchDir,
    pub host: GitHubHost,
    pub harness: Harness<'static, ReviewApp>,
    pub clone: PathBuf,
}

pub(super) fn start(name: &str) -> Setup {
    let scratch = ScratchDir::new(&format!("checkout-{name}"));
    let host = GitHubHost::with_pull_request(scratch.path());
    let connection = host.connection("gh", "Acme");
    let dev = scratch.path().join("dev");
    std::fs::create_dir_all(&dev).expect("dev folder");
    let bare = scratch.path().join("hosted").join(format!("{REPOSITORY}.git"));
    git(&dev, &["clone", "-q", &bare.to_string_lossy(), "widgets"]);
    let clone = dev.join("widgets");
    // The clone knows the host by the URL the app has for the repository, as a real one would.
    let url = format!("{}/{REPOSITORY}.git", connection.base_url);
    git(&clone, &["remote", "set-url", "origin", &url]);
    git(&clone, &["config", "core.autocrlf", "false"]);
    // The host's default branch is the pull request's; a clone starts on main, as usual.
    git(&clone, &["switch", "-q", "main"]);
    let mut harness = app(&scratch, &[connection]);
    // Claude Code is installed, Codex is not; set before the first frame asks.
    let cli = FakeCli::Installed { models: Vec::new(), is_signed_in: true };
    harness.state().services.agents.fake.set(AgentKind::Claude, cli);
    harness.state_mut().services.settings.checkouts.search_folders = Some(vec![dev]);
    open_pull_request(&mut harness);
    Setup { scratch, host, harness, clone }
}

pub(super) fn worktree_of(clone: &Path) -> PathBuf {
    let parent = clone.parent().expect("clone folder");
    parent.join("widgets-wt").join(format!("pr-{NUMBER}-feature"))
}

pub(super) fn shows(harness: &Harness<'_, ReviewApp>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

/// A menu entry's second line or marker, which it carries as its value.
fn has_value(harness: &Harness<'_, ReviewApp>, text: &str) -> bool {
    harness
        .query_all_by(|node| node.value().is_some_and(|value| value.contains(text)))
        .next()
        .is_some()
}

/// Presses `label` and lets what it opened draw: a popup sizes itself unseen in its first frame.
pub(super) fn tap(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    press(harness, label);
    harness.run_steps(3);
}

/// The last widget labelled `label`: the popover's button when the header has one of that name.
pub(super) fn click_last(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    wait_until(harness, label, |harness| harness.query_all_by_label(label).next().is_some());
    harness.click_at(harness.query_all_by_label(label).last().expect("the widget").rect());
    harness.run_steps(3);
}

/// Opens the popover and waits for the form, with the mirror ready for a checkout.
pub(super) fn open_form(harness: &mut Harness<'_, ReviewApp>) {
    tap(harness, "Check out");
    wait_until(harness, "the choices", |harness| {
        harness.query_by_role_and_label(Role::RadioButton, "New worktree").is_some()
            && harness
                .query_all_by_label("Check out")
                .last()
                .is_some_and(|button| !button.accesskit_node().is_disabled())
    });
}

pub(super) fn wait_ready(harness: &mut Harness<'_, ReviewApp>) {
    wait_until(harness, "the done checkout", |harness| {
        harness.query_by_label("Checkout options").is_some() && shows(harness, "Checked out")
    });
}

pub(super) fn head_of(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"])
}

#[test]
fn checkout_walkthrough_offers_runs_and_opens_claude_in_a_new_worktree() {
    let Setup { scratch: _scratch, host, mut harness, clone } = start("main");
    open_form(&mut harness);

    assert!(shows(&harness, POPOVER_TITLE), "the popover names the pull request");
    assert!(shows(&harness, "The commits are already in Code View's mirror"));
    let worktree = worktree_of(&clone);
    assert!(shows(&harness, &worktree.display().to_string()), "the new worktree's folder is shown");
    assert!(shows(&harness, "recommended"), "the worktree is the recommended place");
    assert!(shows(&harness, "on main, clean"), "the clone's state is shown");
    let codex = harness.get_by_role_and_label(Role::RadioButton, "Codex");
    assert!(codex.accesskit_node().is_disabled(), "an agent that is not installed is greyed");
    assert!(shows(&harness, "Codex not installed."));
    assert!(harness.query_by_label("Set up in Settings › Assistant").is_some());

    harness.click_at(harness.get_by_role_and_label(Role::RadioButton, "Claude Code").rect());
    harness.run_steps(3);
    assert!(
        harness.query_by_label("Check out and open Claude Code").is_some(),
        "the button says what opens after"
    );
    harness.click_at(harness.get_by_label("Remember for this repository").rect());
    harness.run_steps(3);
    harness.click_at(harness.get_by_label("Check out and open Claude Code").rect());
    harness.step();
    harness.step();

    // While it runs: the header button counts the steps, the title bar names the running one.
    assert!(shows(&harness, "Checking out…"), "the header button shows the progress");
    assert!(shows(&harness, "Checkout #7 · "), "the title bar's chip names the checkout");
    assert!(shows(&harness, "Fetch #7 from the mirror"), "the steps are listed");
    assert!(shows(&harness, "You can close this. It keeps running and the title bar shows it."));

    wait_ready(&mut harness);
    assert_eq!(head_of(&worktree), host.head_sha(), "the worktree has the pull request's head");
    assert_eq!(git(&worktree, &["branch", "--show-current"]), "feature");
    assert_eq!(head_of(&clone), git(&clone, &["rev-parse", "main"]), "the clone stays on main");
    assert!(!shows(&harness, "Checkout #7 · "), "the chip leaves the title bar when done");
    let terminals = harness.state().services.agents.fake.take_terminals();
    assert_eq!(terminals.len(), 1, "one agent opened: {terminals:?}");
    assert_eq!(terminals[0].program, "claude");
    assert_eq!(terminals[0].dir, worktree, "the agent runs in the worktree");
    assert!(
        harness.query_by_label("Open Claude Code").is_some(),
        "the split button opens it again"
    );
    let settings =
        AppSettings::load(&harness.state().services.paths.settings_file()).expect("saved settings");
    assert_eq!(settings.checkouts.remembered.len(), 1, "the choice is remembered");

    // The menu: where it is, the tools, the folder and the path, removal, the log.
    tap(&mut harness, "Checkout options");
    for entry in ["Checked out in", "Terminal", "Claude Code", "Open folder", "Copy path"] {
        assert!(shows(&harness, entry), "the menu has {entry}");
    }
    assert!(has_value(&harness, "last used"), "the last used tool is marked");
    assert!(!shows(&harness, "Update checkout"), "nothing to update while it has the head");
    tap(&mut harness, "Terminal");
    let terminals = harness.state().services.agents.fake.take_terminals();
    assert_eq!(terminals.len(), 1);
    assert!(terminals[0].program.starts_with("shell"), "the terminal opens: {terminals:?}");

    // Remove worktree asks first, then removes the folder and the record.
    tap(&mut harness, "Checkout options");
    tap(&mut harness, "Remove worktree…");
    wait_until(&mut harness, "the removal card", |harness| shows(harness, "Remove this worktree?"));
    click_last(&mut harness, "Remove worktree");
    wait_until(&mut harness, "the removed worktree", |_| !worktree.exists());
    wait_until(&mut harness, "the button again", |harness| {
        harness.query_by_label("Checkout options").is_none()
    });
    assert_eq!(head_of(&clone), git(&clone, &["rev-parse", "main"]), "the clone is untouched");
}

#[test]
fn checkout_walkthrough_follows_a_new_head_silently_and_says_why_when_it_cannot() {
    let Setup { scratch: _scratch, mut host, mut harness, clone } = start("follow");
    open_form(&mut harness);
    click_last(&mut harness, "Check out");
    wait_ready(&mut harness);
    let worktree = worktree_of(&clone);

    // A push moves the clean worktree along: no toast, no popover, the chip stays plain.
    let pushed = host.push_lines_on_top("/// The greeting every caller shows.\n");
    // The record names the head once the update is done; asking git in every frame would spend
    // the frames (and the live probe's simulated time) on starting processes.
    wait_until(&mut harness, "the followed head", |harness| {
        let records = harness.state().services.checkouts.records().all();
        records.iter().any(|record| record.number == NUMBER && record.head == pushed)
    });
    assert_eq!(head_of(&worktree), pushed, "the worktree follows the new head");
    harness.run_steps(5);
    assert_eq!(harness.state().services.toasts.texts(), Vec::<&str>::new(), "silent");
    assert!(!shows(&harness, "behind"), "the chip has nothing behind");
    assert!(!shows(&harness, POPOVER_TITLE), "no popover opened");

    // A local commit blocks the next update: the chip says how far behind, the menu why.
    std::fs::write(worktree.join("NOTES.md"), "local\n").expect("local file");
    git(&worktree, &["add", "NOTES.md"]);
    git(&worktree, &["commit", "-q", "-m", "Take notes"]);
    let local = head_of(&worktree);
    host.push_lines_on_top("/// Says hello.\n");
    wait_until(&mut harness, "the behind chip", |harness| {
        shows(harness, "Checked out · 1 commit behind")
    });
    assert_eq!(head_of(&worktree), local, "nothing of the local work moved");
    assert_eq!(harness.state().services.toasts.texts(), Vec::<&str>::new(), "still silent");
    tap(&mut harness, "Checkout options");
    assert!(shows(&harness, "Update checkout"), "the menu offers the update by hand");
    assert!(has_value(&harness, "1 behind · "), "with the reason");
    tap(&mut harness, "Update checkout");
    wait_until(&mut harness, "the local commits card", |harness| {
        shows(harness, "Update would drop 1 local commit")
    });
    assert!(harness.query_by_label("Keep as is").is_some());
    tap(&mut harness, "Show in terminal");
    let terminals = harness.state().services.agents.fake.take_terminals();
    assert_eq!(terminals.len(), 1, "the terminal opens on the checkout: {terminals:?}");
    assert_eq!(terminals[0].dir, worktree);
    assert_eq!(head_of(&worktree), local, "showing changes nothing");
}

#[test]
fn checkout_walkthrough_cards_lead_on_from_each_dead_end() {
    let Setup { scratch: _scratch, host: _host, mut harness, clone } = start("cards");

    // A clone with changes: Your clone turns into the card, a worktree is the way on.
    let file = clone.join("src").join("lib.rs");
    let text = std::fs::read_to_string(&file).expect("tracked file");
    std::fs::write(&file, format!("// edited\n{text}")).expect("edited file");
    open_form(&mut harness);
    harness.click_at(harness.get_by_role_and_label(Role::RadioButton, "Your clone").rect());
    harness.run_steps(3);
    assert!(shows(&harness, "YOUR CLONE HAS CHANGES"));
    assert!(shows(&harness, "widgets has 1 uncommitted file"));
    tap(&mut harness, "Open the clone");
    assert_eq!(cv_runtime::toasts::opened_folder(&harness.ctx), Some(clone.clone()));
    tap(&mut harness, "Use a new worktree");
    assert!(
        harness.query_by_role_and_label(Role::RadioButton, "New worktree").is_some(),
        "back at the choices"
    );
    std::fs::write(&file, text).expect("restored file");

    // A taken folder fails the checkout; Retry runs it again once the folder is free.
    let worktree = worktree_of(&clone);
    std::fs::create_dir_all(&worktree).expect("taken folder");
    std::fs::write(worktree.join("other.txt"), "not ours\n").expect("other file");
    click_last(&mut harness, "Check out");
    wait_until(&mut harness, "the failure card", |harness| shows(harness, "FAILED"));
    assert!(shows(&harness, "Checkout failed"), "the header button says it failed");
    assert!(harness.query_by_label("Copy log").is_some());
    assert!(worktree.join("other.txt").exists(), "a folder the checkout did not make stays");
    std::fs::remove_dir_all(&worktree).expect("freed folder");
    tap(&mut harness, "Retry");
    wait_ready(&mut harness);
    assert!(worktree.join("src").join("lib.rs").exists());
}

#[test]
fn checkout_walkthrough_adopts_a_branch_checked_out_already() {
    let Setup { scratch: _scratch, host: _host, mut harness, clone } = start("adopt");
    git(&clone, &["switch", "-q", "feature"]);
    open_form_card(&mut harness, "ALREADY CHECKED OUT");
    assert!(harness.query_by_label("Update it").is_some());
    tap(&mut harness, "Open it");
    wait_ready(&mut harness);
    tap(&mut harness, "Checkout options");
    assert!(shows(&harness, "Opened from your clone"));
    assert!(shows(&harness, &clone.display().to_string()), "the clone is the checkout");
    assert!(!shows(&harness, "Remove worktree…"), "a clone is never offered for removal");
}

#[test]
fn checkout_walkthrough_asks_for_a_fork_and_names_its_branch() {
    let Setup { scratch: _scratch, host: _host, mut harness, clone } = start("fork");
    // The branch is not on the clone's remote: the pull request comes from a fork.
    git(&clone, &["update-ref", "-d", "refs/remotes/origin/feature"]);
    open_form(&mut harness);
    click_last(&mut harness, "Check out");
    wait_until(&mut harness, "the fork card", |harness| shows(harness, "PULL REQUEST FROM A FORK"));
    assert!(harness.query_by_label("Cancel").is_some());
    click_last(&mut harness, "Check out");
    wait_ready(&mut harness);
    let worktree = worktree_of(&clone);
    assert_eq!(git(&worktree, &["branch", "--show-current"]), "pr-7-feature");
    let config = git(&worktree, &["config", "--list", "--local"]);
    assert!(!config.contains("branch.pr-7-feature.merge"), "a fork's branch has no upstream");
}

#[test]
fn checkout_walkthrough_clones_from_the_mirror_without_a_clone() {
    let Setup { scratch, host, mut harness, clone: _clone } = start("no-clone");
    harness.state_mut().services.settings.checkouts.search_folders = Some(Vec::new());
    open_form_card(&mut harness, "NO LOCAL CLONE FOUND");
    assert!(harness.query_by_label("Pick existing clone…").is_some());
    let target = scratch.path().join("fresh");
    std::fs::create_dir_all(&target).expect("target folder");
    harness.state().services.checkouts.answer_folder_dialog(Some(target.clone()));
    wait_until(&mut harness, "the mirror", |harness| {
        harness
            .query_by_label("Clone into a folder…")
            .is_some_and(|button| !button.accesskit_node().is_disabled())
    });
    tap(&mut harness, "Clone into a folder…");
    wait_ready(&mut harness);
    let fresh = target.join("widgets");
    assert_eq!(head_of(&fresh), host.head_sha());
    let pinned = &harness.state().services.settings.checkouts.pinned_clones;
    assert_eq!(pinned.values().collect::<Vec<_>>(), [&fresh], "the new clone is pinned");
    let origin = git(&fresh, &["remote", "get-url", "origin"]);
    assert!(
        origin.ends_with("/acme/widgets.git") && !origin.contains('@'),
        "the remote has no credentials: {origin}"
    );
}

/// Opens the popover and waits for a card instead of the form.
fn open_form_card(harness: &mut Harness<'_, ReviewApp>, card: &str) {
    tap(harness, "Check out");
    wait_until(harness, card, |harness| shows(harness, card));
}

#[test]
fn checkout_walkthrough_settings_section_lists_clones_folders_and_the_pattern() {
    let Setup { scratch, host: _host, mut harness, clone: _clone } = start("settings");
    harness.state_mut().navigator.go(Route::Settings);
    wait_until(&mut harness, "Settings", |harness| harness.state().route == Route::Settings);
    harness.click_at(
        harness.query_all_by_label("Local checkouts").next().expect("the section").rect(),
    );
    wait_until(&mut harness, "the section", |harness| shows(harness, "Folder for new worktrees"));
    assert!(shows(&harness, "Where Check out puts a pull request, and what opens after it."));
    assert!(shows(&harness, "Found by matching the remote URL in the folders below."));
    let dev = scratch.path().join("dev");
    assert!(shows(&harness, &dev.display().to_string()), "the search folder is listed");
    assert!(shows(&harness, "Example: "), "the pattern shows where it puts #12");
    assert!(shows(&harness, "pr-12-login-cache"));
    for option in ["Ask each time", "Claude Code", "Codex"] {
        assert!(shows(&harness, option), "Open by default offers {option}");
    }

    let added = scratch.path().join("more");
    std::fs::create_dir_all(&added).expect("added folder");
    harness.state().services.checkouts.answer_folder_dialog(Some(added.clone()));
    tap(&mut harness, "Add folder…");
    wait_until(&mut harness, "the added folder", |harness| {
        shows(harness, &added.display().to_string())
    });
    let saved =
        AppSettings::load(&harness.state().services.paths.settings_file()).expect("saved settings");
    assert_eq!(saved.checkouts.search_folders, Some(vec![dev, added]));

    tap(&mut harness, "Set up in Settings › Assistant");
    wait_until(&mut harness, "the Assistant section", |harness| {
        shows(harness, "Show the assistant")
    });
}
