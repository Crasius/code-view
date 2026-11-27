//! A restarted app paints its last pages from the page cache's disk tier before the host answers,
//! against a GitLab mock ([`super::gitlab_mock::GitLabProject`]) that, after the restart, answers
//! nothing within the test.
//! `cargo test -p review-app --bin CodeView restart_walkthrough`

use egui_kittest::Harness;

use super::gitlab_mock::{GitLabProject, PROJECT};
use super::walkthrough::{app_with, has, saved_paths, wait_idle, wait_until, wait_until_unsettled};
use super::{ReviewApp, startup};
use cv_runtime::route::{PullRequestKey, Route};
use cv_testkit::ScratchDir;

const CONNECTION_ID: &str = "restart";

/// After a restart the list and a merge request's overview paint their last answers from the disk
/// tier at once, each saying it is updating while the host takes its time.
#[test]
fn saved_pages_show_at_once_after_a_restart() {
    let host = GitLabProject::start(3);
    let scratch = ScratchDir::new("restart-saved-pages");
    let paths = saved_paths(&scratch, &[host.connection(CONNECTION_ID)]);
    {
        let mut harness = app_with(builder(), paths.clone(), cv_providers::create_provider);
        wait_until_unsettled(&mut harness, "the merge request list", |harness| {
            has(harness, "Change 3")
        });
        harness.state_mut().navigator.go(Route::PullRequest(key(1)));
        wait_until_unsettled(&mut harness, "the overview", |harness| {
            has(harness, "Change 1") && has(harness, "Discussion")
        });
        wait_idle(&mut harness);
        let saved = paths.cold_cache_dir().join("page");
        // The list, the overview and its threads, written on the cold tier's own thread.
        wait_until_unsettled(&mut harness, "the pages on the disk tier", |_| {
            std::fs::read_dir(&saved).map_or(0, Iterator::count) >= 3
        });
    }
    // No request leaves before the restart, and none of the restarted app's is answered within
    // the test: whatever shows came from the disk.
    host.hold_answers();
    let mut harness = startup::first_frames(builder(), |creation| {
        ReviewApp::new(creation, paths, cv_providers::create_provider)
    });
    wait_until(&mut harness, "the saved list", |harness| {
        has(harness, "Change 3") && has(harness, "Updating…")
    });
    harness.state_mut().navigator.go(Route::PullRequest(key(1)));
    wait_until(&mut harness, "the saved overview", |harness| {
        has(harness, "Change 1") && has(harness, "Discussion") && has(harness, "Updating…")
    });
}

fn builder() -> egui_kittest::HarnessBuilder<ReviewApp> {
    Harness::builder().with_size([1440.0, 900.0])
}

fn key(number: u64) -> PullRequestKey {
    PullRequestKey {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: PROJECT.to_owned(),
        number,
    }
}
