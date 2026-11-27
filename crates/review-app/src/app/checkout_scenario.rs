//! The checkout walkthroughs' scenario: the host, the app on its pull request, a clean clone in
//! dev/widgets; and the popover's steps from the form to the done checkout.

use std::path::{Path, PathBuf};

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::AgentKind;

use super::ReviewApp;
use super::github_mock::{GitHubHost, NUMBER, REPOSITORY, open_pull_request};
use super::walkthrough::{app, has, press, wait_until};
use cv_services::agents::FakeCli;
use cv_testkit::Clicks;
use cv_testkit::{ScratchDir, git};

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

/// The last widget labelled `label`: the popover's button when the header has one of that name.
pub(super) fn click_last(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    wait_until(harness, label, |harness| harness.query_all_by_label(label).next().is_some());
    harness.click_at(harness.query_all_by_label(label).last().expect("the widget").rect());
    harness.run_steps(3);
}

/// Opens the popover and waits for the form, with the mirror ready for a checkout.
pub(super) fn open_form(harness: &mut Harness<'_, ReviewApp>) {
    press(harness, "Check out");
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
        harness.query_by_label("Checkout options").is_some() && has(harness, "Checked out")
    });
}
