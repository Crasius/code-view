//! A window in the background, against a GitLab mock ([`super::gitlab_mock::GitLabProject`]):
//! what waits for the window to be active asks for no frames meanwhile.
//! `cargo test -p review-app --bin CodeView background_window_walkthrough`

use std::time::Duration;

use super::gitlab_mock::{GitLabProject, PROJECT};
use super::walkthrough::{app, has, set_focus, wait_until, wait_until_unsettled};
use cv_runtime::route::{PullRequestKey, Route};
use cv_testkit::ScratchDir;

/// Frames the unfocused Overview runs while its skeletons are checked.
const UNFOCUSED_FRAMES: usize = 60;

/// A window in the background holds its skeletons still: the merge policy loads only while the
/// window is active, so the Overview's merge type stays a placeholder there, and its sweep asked
/// for a frame every millisecond, the full frame rate for as long as the window sat behind others.
#[test]
fn an_unfocused_overview_holds_its_skeletons_still() {
    let host = GitLabProject::start(1);
    let scratch = ScratchDir::new("background-window");
    let mut harness = app(&scratch, &[host.connection("background")]);
    wait_until_unsettled(&mut harness, "the merge request list", |harness| {
        has(harness, "Change 1")
    });
    set_focus(&mut harness, false);
    let key = PullRequestKey {
        connection_id: "background".to_owned(),
        repository_key: PROJECT.to_owned(),
        number: 1,
    };
    harness.state_mut().navigator.go(Route::PullRequest(key));
    wait_until_unsettled(&mut harness, "the overview", |harness| has(harness, "Discussion"));
    let mut causes = std::collections::BTreeSet::new();
    let mut time = harness.ctx.input(|input| input.time);
    for _ in 0..UNFOCUSED_FRAMES {
        time += 0.1;
        harness.input_mut().time = Some(time);
        harness.step();
        causes.extend(harness.ctx.repaint_causes().iter().map(|cause| cause.file.to_owned()));
        cv_testkit::let_threads_run(Duration::from_millis(3));
    }
    assert!(!has(&harness, "Merge type: "), "the merge type is still a placeholder");
    let sweeps: Vec<_> = causes
        .iter()
        .filter(|file| file.replace('\\', "/").ends_with("cv-widgets/src/skeleton.rs"))
        .collect();
    assert!(sweeps.is_empty(), "no skeleton sweeps in the background: {causes:?}");
    // The placeholder stood for the policy: the active window loads it.
    set_focus(&mut harness, true);
    wait_until(&mut harness, "the merge type", |harness| has(harness, "Merge type: "));
}
