//! A finished pipeline's job logs ([`Scenario::LogReuse`]): the scripted provider counts every
//! download. A finished job's log is downloaded once, then comes from disk, when switching back to
//! it and after a restart; a download cut off mid-body leaves no whole log, so the next open
//! downloads it again.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_store::AppPaths;

use super::super::walkthrough::{has, wait_until};
use super::{App, CUT_OFF, Calls, ReviewApp, Scenario};
use cv_testkit::Clicks;

const ID: &str = "log-reuse";
const RUN: &str = "Pipeline #50";
const SIZE: [f32; 2] = [1500.0, 900.0];

#[test]
fn log_reuse_finished_logs_open_from_disk_and_a_cut_download_is_fetched_again() {
    let App { mut harness, calls, scratch } = App::start(ID, Scenario::LogReuse, SIZE);
    let paths = scratch.app_paths();
    open_run(&mut harness);

    // J opens the run's first job; its log is downloaded once.
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "build");
    assert_eq!(downloads(&calls, "7:1"), 1, "the first open downloads the log");

    // Another job and back: the finished job's log comes from disk.
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "lint");
    harness.key_press(egui::Key::K);
    wait_for_log(&mut harness, "build");
    assert_eq!(downloads(&calls, "7:1"), 1, "switching back downloads nothing");
    assert_eq!(downloads(&calls, "7:2"), 1);
    assert_eq!(whole_logs(&paths), 2, "each finished job's log rests whole");

    // The host drops the third job's log mid-body: an error, and no whole file.
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "lint");
    harness.key_press(egui::Key::J);
    wait_until(&mut harness, "the cut-off download", |harness| {
        has(harness, "flaky (current page)") && has(harness, "The log of flaky did not download")
    });
    assert_eq!(downloads(&calls, CUT_OFF), 1);
    assert_eq!(whole_logs(&paths), 2, "a cut-off download never looks whole");
    harness.key_press(egui::Key::K);
    wait_for_log(&mut harness, "lint");
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "flaky");
    assert_eq!(downloads(&calls, CUT_OFF), 2, "the next open downloads it again");
    assert_eq!(whole_logs(&paths), 3);
    drop(harness);

    // After a restart on the same data folder, every finished job's log opens from disk.
    let before = calls.lock().unwrap_or_else(PoisonError::into_inner).logs.len();
    let mut harness = App::reopen(ID, Scenario::LogReuse, &calls, &scratch, SIZE);
    open_run(&mut harness);
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "build");
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "lint");
    harness.key_press(egui::Key::J);
    wait_for_log(&mut harness, "flaky");
    assert_eq!(
        calls.lock().unwrap_or_else(PoisonError::into_inner).logs.len(),
        before,
        "no request after the restart"
    );
}

fn open_run(harness: &mut Harness<'_, ReviewApp>) {
    wait_until(harness, "the tabs", |harness| harness.query_by_label("Pipelines").is_some());
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_until(harness, "the checks table", |harness| {
        harness.query_by_label("Filter checks").is_some()
    });
    harness.click_at(harness.get_by_label(&format!("{RUN}, Succeeded")).rect());
    wait_until(harness, "the stage graph", |harness| {
        harness.query_by_label("Stage test").is_some()
    });
}

/// Waits until `job`'s log is shown and nothing loads any more.
fn wait_for_log(harness: &mut Harness<'_, ReviewApp>, job: &str) {
    wait_until(harness, &format!("the log of {job}"), |harness| {
        has(harness, &format!("{job} (current page)"))
            && harness.query_by_label("Copy log").is_some()
            && harness.state().services.background.running() == 0
    });
}

/// How often the log `log_id` was downloaded.
fn downloads(calls: &Arc<Mutex<Calls>>, log_id: &str) -> usize {
    let calls = calls.lock().unwrap_or_else(PoisonError::into_inner);
    calls.logs.iter().filter(|id| *id == log_id).count()
}

/// The whole logs in the cache.
fn whole_logs(paths: &AppPaths) -> usize {
    count_whole(&paths.connection_logs_dir(ID))
}

fn count_whole(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".whole.log"))
            .count()
    })
}
