//! Opening a pull request, end to end against the mocked GitHub host with a real git mirror: the
//! repository's mirror opens alongside the detail request instead of after it, so only the fetch
//! of the pull request's commits waits for the detail.

use std::path::Path;
use std::time::{Duration, Instant};

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::github_mock::{ExtraFile, GitHubHost, NUMBER, TITLE};
use super::walkthrough::{LIMIT, app, wait_until};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

/// True once a mirror's bare repository exists in the app's cache.
fn has_mirror(scratch: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(scratch.join("cache").join("mirrors")) else {
        return false;
    };
    entries.flatten().any(|entry| entry.path().join("HEAD").is_file())
}

/// Clicks the pull request's row in "My pull requests" once it shows.
fn click_row(harness: &mut Harness<'_, ReviewApp>) {
    let row = format!("{NUMBER} {TITLE}");
    wait_until(harness, "the pull request row", |harness| harness.query_by_label(&row).is_some());
    harness.click_at(harness.get_by_label(&row).rect());
}

#[test]
fn the_mirror_opens_while_the_detail_is_still_loading() {
    let scratch = ScratchDir::new("open-path-overlap");
    let host = GitHubHost::with_pull_request(scratch.path());
    // The detail never answers, so a mirror that opens cannot have waited for it.
    host.fail_pull_request();
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    click_row(&mut harness);
    wait_until(&mut harness, "the mirror", |_| has_mirror(scratch.path()));
}

/// Time from the click on the row to the Files tab's count (the fast file list), printed:
/// `cargo test -p review-app --bin CodeView open_path_latency -- --ignored --nocapture`. Numbers,
/// no assertions: timings never assert.
#[test]
#[ignore = "a measurement, run by hand"]
fn open_path_latency() {
    const RUNS: usize = 7;
    let sized = |path: &'static str| {
        let line =
            |index: usize, version: &str| format!("pub fn f{index}() -> u32 {{ {version} }}\n");
        let base: String = (0..3_000).map(|index| line(index, "1")).collect();
        let head: String =
            (0..3_000).map(|index| line(index, if index % 10 == 0 { "2" } else { "1" })).collect();
        ExtraFile { path, base: base.into_bytes(), head: head.into_bytes() }
    };
    let files: Vec<ExtraFile> = ["src/cart/a.rs", "src/cart/b.rs", "web/app.rs", "zz/a.rs"]
        .into_iter()
        .map(sized)
        .collect();
    let mut first_open = Vec::new();
    let mut after_restart = Vec::new();
    for run in 0..RUNS {
        let scratch = ScratchDir::new(&format!("open-path-latency-{run}"));
        let host = GitHubHost::with_pull_request_files(scratch.path(), &files);
        let connection = host.connection("gh", "Acme");
        // A new mirror, then the same mirror after an app restart (its commits are local).
        for times in [&mut first_open, &mut after_restart] {
            let mut harness = app(&scratch, std::slice::from_ref(&connection));
            times.push(time_to_files(&mut harness));
        }
    }
    let median = |times: &mut Vec<Duration>| {
        times.sort();
        times[times.len() / 2].as_secs_f64() * 1000.0
    };
    println!("first open of the repository: median {:>7.1} ms of {RUNS}", median(&mut first_open));
    println!(
        "open after a restart:         median {:>7.1} ms of {RUNS}",
        median(&mut after_restart)
    );
}

fn time_to_files(harness: &mut Harness<'_, ReviewApp>) -> Duration {
    click_row(harness);
    let started = Instant::now();
    loop {
        harness.step();
        let is_listed = harness
            .query_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files ("))
            })
            .is_some();
        if is_listed {
            return started.elapsed();
        }
        assert!(started.elapsed() < LIMIT, "the files never listed");
        cv_testkit::let_threads_run(Duration::from_millis(1));
    }
}
