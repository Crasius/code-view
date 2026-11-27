//! Diffs loaded ahead in the Files tab, end to end: the real app against the mocked GitHub host
//! with a real git mirror. Opening a file loads the next files of its folder and the previous one
//! into the diff cache, a file row hovered past the dwell loads too (in any folder), and switching
//! to such a file shows its diff in the first frame after the click.

use std::time::{Duration, Instant};

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_store::FilesStart;

use super::ReviewApp;
use super::github_mock::{ExtraFile, GitHubHost, open_pull_request};
use super::walkthrough::{app, has_cached_diff, settle_background, wait_for, wait_until};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const A: &str = "src/cart/a.rs";
const B: &str = "src/cart/b.rs";
/// More changed lines than files are loaded ahead with.
const HUGE: &str = "src/cart/c_huge.rs";
const D: &str = "src/cart/d.rs";
const E: &str = "src/cart/e.rs";
const F: &str = "src/cart/f.rs";
const G: &str = "src/cart/g.rs";
const H: &str = "src/cart/h.rs";
const APP: &str = "web/app.rs";
const UTIL: &str = "web/util.rs";

/// `lines` lines of Rust, every `every`th one changed in the head version.
fn source(lines: usize, every: usize) -> ExtraFile {
    let line = |index: usize, version: &str| format!("pub fn f{index}() -> u32 {{ {version} }}\n");
    let base: String = (0..lines).map(|index| line(index, "1")).collect();
    let head: String =
        (0..lines).map(|index| line(index, if index % every == 0 { "2" } else { "1" })).collect();
    ExtraFile { path: "", base: base.into_bytes(), head: head.into_bytes() }
}

/// The tree's last folder, far from `src/cart/`.
const FAR: [&str; 3] = ["zz/a.rs", "zz/b.rs", "zz/c.rs"];

/// Tree order: `src/cart/` (a, b, `c_huge`, d to h), `src/lib.rs` (the mock's own file), `web/`,
/// `zz/`. Every file but the huge one has 3,000 lines with 600 changed.
fn files() -> Vec<ExtraFile> {
    let sized = |path: &'static str| ExtraFile { path, ..source(3_000, 10) };
    let mut files: Vec<ExtraFile> = FAR.into_iter().map(sized).collect();
    files.extend([A, B, D, E, F, G, H, APP, UTIL].map(sized));
    // 12,000 lines with 6,000 changed: above the limit for loading ahead.
    files.push(ExtraFile { path: HUGE, ..source(12_000, 4) });
    files
}

/// The app on the fixture's pull request, its Files tab open on its first file, [`A`].
fn opened_on_first_file(scratch: &ScratchDir) -> (GitHubHost, Harness<'static, ReviewApp>) {
    let host = GitHubHost::with_pull_request_files(scratch.path(), &files());
    let mut harness = app(scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().services.settings.files_start = FilesStart::FirstFile;
    open_pull_request(&mut harness);
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files"))
            })
            .rect(),
    );
    wait_until(&mut harness, "the first file's diff", |harness| shows_diff(harness, A));
    (host, harness)
}

fn shows_diff(harness: &Harness<'_, ReviewApp>, path: &str) -> bool {
    harness.query_by_label(&format!("Diff of {path}")).is_some()
}

/// Steps frames for `duration` of wall clock, so background work goes on.
fn pump(harness: &mut Harness<'_, ReviewApp>, duration: Duration) {
    let started = Instant::now();
    while started.elapsed() < duration {
        harness.step();
        cv_testkit::let_threads_run(Duration::from_millis(5));
    }
}

/// Clicks `path` in the tree and steps until its diff shows: the wall clock and the frames from
/// the click to the first frame that shows it.
fn switch_to(harness: &mut Harness<'_, ReviewApp>, path: &str) -> (Duration, usize) {
    let started = Instant::now();
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, path).rect());
    let mut frames = 0;
    wait_for(
        harness,
        &format!("{path}'s diff"),
        |harness| {
            frames += 1;
            shows_diff(harness, path).then_some(())
        },
        |_| {},
    );
    (started.elapsed(), frames)
}

/// The frame of the click lays the tree out after the diff and hands the pick on, so the first
/// frame after it is the earliest that can show the picked file.
const FIRST_FRAME_AFTER_THE_CLICK: usize = 2;

fn assert_shows_at_once(harness: &mut Harness<'_, ReviewApp>, path: &str, why: &str) {
    settle_background(harness);
    let (_, frames) = switch_to(harness, path);
    assert_eq!(frames, FIRST_FRAME_AFTER_THE_CLICK, "{path} ({why}) shows in the first frame");
    assert!(harness.query_by_label("Loading the diff").is_none());
}

/// The cache tells, since how many frames the load takes is the machine's speed: a local git diff
/// can land in the click's next frame.
fn assert_loads_when_opened(harness: &mut Harness<'_, ReviewApp>, path: &str, why: &str) {
    settle_background(harness);
    assert!(!has_cached_diff(harness, path), "{path} ({why}) was not loaded ahead");
    switch_to(harness, path);
}

#[test]
fn files_loaded_ahead_show_in_the_first_frame_after_the_click() {
    let scratch = ScratchDir::new("prefetch-walkthrough");
    let (_host, mut harness) = opened_on_first_file(&scratch);
    // Opening `a.rs` loaded `b.rs`, `d.rs`, `e.rs` and `f.rs`.
    assert_shows_at_once(&mut harness, E, "the third file after the open one, past the huge one");
    assert_shows_at_once(&mut harness, D, "the file before the open one");
    assert_loads_when_opened(&mut harness, HUGE, "a file above the limit");
    assert_loads_when_opened(&mut harness, FAR[1], "a file far from the open one");
    harness.get_by_role_and_label(Role::TreeItem, UTIL).hover();
    assert_shows_at_once(&mut harness, UTIL, "the file the pointer rested on");
}

/// Switch times on the fixture, printed: `cargo test -p review-app --bin CodeView
/// prefetch_latency -- --ignored --nocapture`. Numbers, no assertions: timings never assert.
#[test]
#[ignore = "a measurement, run by hand"]
fn prefetch_latency() {
    let scratch = ScratchDir::new("prefetch-latency");
    let (_host, mut harness) = opened_on_first_file(&scratch);
    let mut report = Vec::new();
    let mut measure = |harness: &mut Harness<'_, ReviewApp>, what: &str, path: &str| {
        pump(harness, Duration::from_millis(1_500));
        let (elapsed, frames) = switch_to(harness, path);
        report.push(format!(
            "{what:<40} {path:<20} {:>7.1} ms {frames:>3} frames",
            elapsed.as_secs_f64() * 1000.0
        ));
    };
    // Each target is near no file opened before it, unless the line says otherwise.
    measure(&mut harness, "next file", B);
    measure(&mut harness, "third file after the open one", E);
    measure(&mut harness, "far file (cold)", FAR[2]);
    measure(&mut harness, "opened before (warm)", A);
    measure(&mut harness, "huge file (cold)", HUGE);
    harness.get_by_role_and_label(Role::TreeItem, UTIL).hover();
    measure(&mut harness, "hovered file", UTIL);
    // The git part of a cold switch alone, on the hosted repository (all blobs local).
    let hosted = scratch.path().join("hosted/acme/widgets.git");
    for path in [B, HUGE] {
        let started = Instant::now();
        cv_testkit::git(
            &hosted,
            &["diff", "--no-color", "-M", "-U3", "main", "feature", "--", path],
        );
        report.push(format!(
            "{:<40} {path:<20} {:>7.1} ms",
            "git diff alone",
            started.elapsed().as_secs_f64() * 1000.0
        ));
    }
    for line in report {
        println!("{line}");
    }
}
