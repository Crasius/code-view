//! The Files tab's review loop by keyboard, end to end: the real app against the mocked GitHub host
//! ([`super::github_mock`]). With "Hide viewed files" on, Alt+V on the open file keeps it listed,
//! so J still opens the next file. Alt+T opens the threads panel with the keys: a thread is
//! selected at once, N steps to the next, R opens its reply box with the keyboard in it, Shift+R
//! resolves it and Esc hands the keys back to the diff. Needs `git` on PATH, no network.

use egui::accesskit::Toggled;
use egui::{Key, Modifiers, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::ReviewApp;
use super::github_mock::{ExtraFile, FILE, GitHubHost, MockThread, open_pull_request};
use super::walkthrough::{
    app, files_tab, open_in_tree, shows_file_view, wait_for_diff, wait_until,
};
use cv_testkit::ScratchDir;

const FIRST: &str = "src/a.rs";
const SECOND: &str = "src/b.rs";
const FIRST_THREAD: &str = "Why one here?";
const SECOND_THREAD: &str = "Is two right?";

fn file(path: &'static str, value: u32) -> ExtraFile {
    ExtraFile {
        path,
        base: b"pub fn value() -> u32 {\n    0\n}\n".to_vec(),
        head: format!("pub fn value() -> u32 {{\n    {value}\n}}\n").into_bytes(),
    }
}

#[test]
fn bug_keys_mark_viewed_then_step_and_triage_threads() {
    let scratch = ScratchDir::new("bug-keys");
    let host =
        GitHubHost::with_pull_request_files(scratch.path(), &[file(FIRST, 1), file(SECOND, 2)]);
    host.add_threads(&[
        MockThread { id: "PRRT_first", path: FIRST, lines: 2..=2, text: FIRST_THREAD },
        MockThread { id: "PRRT_second", path: SECOND, lines: 2..=2, text: SECOND_THREAD },
    ]);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().services.settings.file_filters.hide_viewed = true;
    // J steps on here, not the option that opens the next unviewed file.
    harness.state_mut().services.settings.open_next_unviewed = false;
    open_pull_request(&mut harness);
    files_tab(&mut harness);

    // Viewed, the open file stays listed, so J steps from it; once the next opens, it leaves.
    open_in_tree(&mut harness, FIRST);
    wait_for_diff(&mut harness, FIRST);
    harness.key_press_modifiers(Modifiers::ALT, Key::V);
    harness.run_steps(3);
    assert!(
        harness.query_by_role_and_label(Role::TreeItem, FIRST).is_some(),
        "the open file stays in the tree once viewed"
    );
    harness.key_press(Key::J);
    wait_for_diff(&mut harness, SECOND);
    wait_until(&mut harness, "the viewed file to leave the tree", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, FIRST).is_none()
    });

    // Alt+T: the panel opens with the keys and the first thread to read selected and shown. The
    // filter goes off first, so the panel lists the threads in the order of all the files.
    harness.state_mut().services.settings.file_filters.hide_viewed = false;
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::ALT, Key::T);
    wait_until(&mut harness, "the first thread selected", |harness| {
        is_selected(harness, FIRST_THREAD)
    });
    wait_for_diff(&mut harness, FIRST);
    harness.key_press(Key::N);
    wait_until(&mut harness, "N to select the next thread", |harness| {
        is_selected(harness, SECOND_THREAD)
    });
    wait_for_diff(&mut harness, SECOND);

    // R: the selected thread's reply box takes the keyboard, without the letter in it.
    harness.key_press(Key::R);
    wait_until(&mut harness, "the reply box to take the keyboard", |harness| {
        harness.ctx.text_edit_focused()
    });
    assert!(
        harness.query_by(|node| node.value().as_deref() == Some("r")).is_none(),
        "the key is not typed into the reply"
    );
    harness.key_press(Key::Escape);
    wait_until(&mut harness, "the reply box to close", |harness| !harness.ctx.text_edit_focused());

    // Shift+R resolves the selected thread on the host.
    harness.key_press_modifiers(Modifiers::SHIFT, Key::R);
    wait_until(&mut harness, "the resolve request", |_| {
        host.find_graphql("resolveReviewThread")
            .is_some_and(|body| body["variables"]["id"] == "PRRT_second")
    });

    // Esc: the keys go back to the diff; N no longer steps through the threads.
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    harness.key_press(Key::N);
    harness.run_steps(6);
    assert!(is_selected(&harness, SECOND_THREAD), "the panel no longer takes N");
    assert!(!shows_file_view(&harness, FILE), "nor opens a thread");
}

/// The threads panel's row of the thread reading `text` is the selected one.
fn is_selected(harness: &Harness<'_, ReviewApp>, text: &str) -> bool {
    harness.query_all_by_role(Role::Button).any(|node| {
        node.accesskit_node().label().is_some_and(|label| label.ends_with(&format!(": {text}")))
            && node.accesskit_node().toggled() == Some(Toggled::True)
    })
}
