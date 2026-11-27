//! Where a pull request opens, end to end: the real app against the mocked GitHub host with a real
//! git mirror. Settings' "Pull requests open on: Files" opens a pull request on its first file's
//! diff; after that it opens where the user left it (tab and file), also after a restart; a saved
//! file that is no longer changed falls back to the first file without an error.

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_store::{
    AppPaths, AppSettings, LastPlace, PullRequestStart, PullRequestTab, ViewedFiles,
};

use super::ReviewApp;
use super::github_mock::{ExtraFile, GitHubHost, open_pull_request};
use super::walkthrough::{
    app, files_tab, press_role, settle_background, shows_file_view, shows_folder_view, wait_until,
};
use cv_runtime::route::Route;
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

/// First in the tree's order, before the mock's own `src/lib.rs`.
const FIRST: &str = "a/first.rs";
/// Last in the tree's order.
const LAST: &str = "z/last.rs";

fn files() -> Vec<ExtraFile> {
    let file = |path| ExtraFile {
        path,
        base: b"pub fn one() -> u32 {\n    1\n}\n".to_vec(),
        head: b"pub fn one() -> u32 {\n    2\n}\n".to_vec(),
    };
    vec![file(FIRST), file(LAST)]
}

fn paths(scratch: &ScratchDir) -> AppPaths {
    scratch.app_paths()
}

fn tab(harness: &Harness<'_, ReviewApp>) -> Option<PullRequestTab> {
    harness.state().current_tab()
}

/// Opens the pull request and waits for `path`'s diff, the folder view never showing on the way.
fn open_on_file(harness: &mut Harness<'_, ReviewApp>, path: &str) {
    open_pull_request(harness);
    wait_until(harness, path, |harness| {
        assert!(!shows_folder_view(harness), "the folder view showed");
        shows_file_view(harness, path)
    });
    assert_eq!(tab(harness), Some(PullRequestTab::Files));
}

/// Picks "Files" in Settings → Appearance → "Pull requests open on", as a user does.
fn choose_files_in_settings(harness: &mut Harness<'_, ReviewApp>) {
    harness.state_mut().navigator.go(Route::Settings);
    wait_until(harness, "Settings", |harness| harness.state().route == Route::Settings);
    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Appearance").rect());
    wait_until(harness, "the row", |harness| {
        harness.query_by_label("Pull requests open on").is_some()
    });
    // The row sits below the fold of Appearance.
    press_role(harness, Role::RadioButton, "Files");
    wait_until(harness, "the choice", |harness| {
        harness.state().services.settings.pull_request_start == PullRequestStart::Files
    });
    harness.state_mut().navigator.go(Route::MyPullRequests);
}

#[test]
fn open_resume_files_option_and_the_place_left_survive_a_restart() {
    let scratch = ScratchDir::new("open-resume");
    let host = GitHubHost::with_pull_request_files(scratch.path(), &files());
    let connection = host.connection("gh", "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    assert_eq!(
        harness.state().services.settings.pull_request_start,
        PullRequestStart::Overview,
        "the Overview stays the default"
    );
    choose_files_in_settings(&mut harness);
    let saved = AppSettings::load(&paths(&scratch).settings_file()).expect("settings file");
    assert_eq!(saved.pull_request_start, PullRequestStart::Files, "the choice is saved");

    open_on_file(&mut harness, FIRST);
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, LAST).rect());
    wait_until(&mut harness, "the last file", |harness| shows_file_view(harness, LAST));
    drop(harness);

    // A restart opens the file left open, not the first one.
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    assert_eq!(harness.state().services.settings.pull_request_start, PullRequestStart::Files);
    open_on_file(&mut harness, LAST);
    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Overview").rect());
    wait_until(&mut harness, "the Overview", |harness| {
        tab(harness) == Some(PullRequestTab::Overview)
    });
    drop(harness);

    // Left on the Overview: it opens there, and the Files tab still has the file left open.
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    open_pull_request(&mut harness);
    assert_eq!(tab(&harness), Some(PullRequestTab::Overview), "the tab left on");
    files_tab(&mut harness);
    wait_until(&mut harness, "the file left open", |harness| shows_file_view(harness, LAST));
    assert!(!shows_file_view(&harness, FIRST));
}

/// A saved file the pull request no longer changes opens the first file, without an error or the
/// "no longer changed" hint: nothing the user did took it away.
#[test]
fn open_resume_a_vanished_file_falls_back_to_the_first_file() {
    let scratch = ScratchDir::new("open-resume-vanished");
    let host = GitHubHost::with_pull_request_files(scratch.path(), &files());
    let connection = host.connection("gh", "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    open_pull_request(&mut harness);
    assert_eq!(tab(&harness), Some(PullRequestTab::Overview), "the default without a place");
    let Route::PullRequest(key) = harness.state().route.clone() else {
        panic!("the pull request is open");
    };
    harness.state_mut().services.settings.pull_request_start = PullRequestStart::Files;
    harness.state_mut().services.save_settings();
    // The first app's background work (the mirror it opens for the page, a git process that
    // outlives the app) ends before the second app opens the same mirror: two at once fail on
    // git's config lock ("could not lock config file ... File exists").
    settle_background(&mut harness);
    drop(harness);

    let paths = paths(&scratch);
    let mut viewed =
        ViewedFiles::load(&paths, &key.connection_id, &key.storage_key()).expect("the marks' file");
    let gone = LastPlace { tab: PullRequestTab::Files, file: Some("gone/removed.rs".to_owned()) };
    viewed.set_last_place(gone).expect("saved");

    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    open_on_file(&mut harness, FIRST);
    assert!(harness.query_by_label_contains("Could not").is_none(), "no error");
    assert!(harness.query_by_label_contains("is no longer changed").is_none(), "no hint");
    let saved =
        ViewedFiles::load(&paths, &key.connection_id, &key.storage_key()).expect("the marks' file");
    assert_eq!(
        saved.last_place(),
        Some(&LastPlace { tab: PullRequestTab::Files, file: Some(FIRST.to_owned()) }),
        "the place shown now is saved"
    );
}
