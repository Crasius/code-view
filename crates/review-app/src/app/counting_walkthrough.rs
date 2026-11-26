//! The Files tab's diffs never wait for the line counts, offline against the mocked GitHub host
//! ([`super::github_mock`]) with a pull request of many changed files. Counting the lines
//! downloads the changed blobs in chunks, which the host holds back as a slow host would, until
//! the test opens the hold.
//!
//! While those downloads are held, the folder summary (the tab's default view) shows the code of
//! its first card and a file opened from the tree shows its diff; once released, the counts
//! arrive.
//! `cargo test -p review-app counting_walkthrough`

use std::sync::LazyLock;

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::action_e2e::{app, open_pull_request, wait_until};
use super::clicks::Clicks;
use super::github_mock::{ExtraFile, FILE, GitHubHost};
use cv_testkit::ScratchDir;

/// Changed files besides [`FILE`]: enough that their blobs make large downloads.
const EXTRA_FILES: usize = 40;
/// The spinner in the file tree's progress line while the lines are counted.
const COUNTING: &str = "Counting lines";

/// The extra files' paths; the mock takes paths that live as long as the program.
static PATHS: LazyLock<Vec<String>> =
    LazyLock::new(|| (0..EXTRA_FILES).map(|index| format!("src/part_{index:02}.rs")).collect());

fn extra_files() -> Vec<ExtraFile> {
    PATHS
        .iter()
        .enumerate()
        .map(|(index, path)| ExtraFile {
            path: path.as_str(),
            base: format!("pub fn part() -> u32 {{\n    {index}\n}}\n").into_bytes(),
            head: format!("pub fn part() -> u32 {{\n    {}\n}}\n", index + 100).into_bytes(),
        })
        .collect()
}

#[test]
fn counting_walkthrough_diffs_show_while_the_lines_are_counted() {
    let scratch = ScratchDir::new("counting");
    let mut host = GitHubHost::with_pull_request_files(scratch.path(), &extra_files());
    let held = host.hold_batch_downloads();
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    // Dropped before the app, so a failing test lets the held downloads go before it shuts down.
    let held = held;
    open_pull_request(&mut harness);
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files"))
            })
            .rect(),
    );

    wait_until(&mut harness, "the first card's code", |harness| {
        harness.query_by_label(&format!("Diff of {FILE}")).is_some()
    });
    assert_counting(&harness, "the folder summary showed its first card");

    let opened = "src/part_03.rs";
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, opened).rect());
    wait_until(&mut harness, "the opened file's diff", |harness| {
        harness.query_by_label(&format!("Diff of {opened}")).is_some()
    });
    assert_counting(&harness, "the file view showed the opened file");

    held.open();
    wait_until(&mut harness, "the counted lines", |harness| {
        harness.query_by_label(COUNTING).is_none()
            && harness.query_by_label_contains(" added, ").is_some()
    });
}

/// The held download is still on its way: what showed did not wait for it.
fn assert_counting(harness: &Harness<'_, ReviewApp>, what: &str) {
    assert!(harness.query_by_label(COUNTING).is_some(), "{what} only after the lines were counted");
}
