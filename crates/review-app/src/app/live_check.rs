//! Headless end-to-end check against public repositories, without any token.
//!
//! Drives the real app through its accessibility tree like a user and prints load times, memory
//! and requests. Needs network access, so it is ignored by default:
//!
//! `cargo test -p review-app --bin CodeView app::live_check -- --ignored --nocapture`
//!
//! `REVIEW_LIVE_KIND` picks the platform (`github` default, `gitlab`, `azure`), `REVIEW_LIVE_REPO`
//! the repository (defaults below), `REVIEW_LIVE_PR` a pull request number (default: the newest
//! open one), `REVIEW_LIVE_FILE` the file whose diff is measured (default: the first one),
//! `REVIEW_LIVE_SKIP_DIFF` (any value) goes from the file list straight on to commits and pipelines,
//! `REVIEW_LIVE_PIPELINES_ONLY` (any value) from the overview straight to the pipelines, without
//! waiting for the mirror (large repositories).
//!
//! The harness clock runs at 60 frames per simulated second, slower than the waits step it, so the
//! live probe goes out no more often than in the app; anonymous hourly limits are small.

use std::time::{Duration, Instant};

use cv_base::platform::heap;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{Connection, ProviderKind};
use review_store::{AppPaths, DiffLayout, FilesStart};

use super::ReviewApp;
use super::walkthrough::{saved_paths, wheel};
use cv_runtime::route::{PullRequestKey, Route};
use cv_testkit::{Clicks, ScratchDir, samples};

const CONNECTION_ID: &str = "live";
const NETWORK_TIMEOUT: Duration = Duration::from_secs(600);

#[test]
#[ignore = "needs network; run explicitly"]
fn live_walkthrough_of_a_public_pull_request() {
    cv_testkit::allow_real_network();
    let platform = Platform::from_environment();
    let repository = std::env::var("REVIEW_LIVE_REPO")
        .unwrap_or_else(|_| platform.default_repository.to_owned());
    let scratch = ScratchDir::new("live-public");
    let paths = live_paths(&scratch, &platform);

    let mut harness = realtime_harness(&paths);
    // The diff walkthrough measures a file's diff; the folder view, where the Files tab starts
    // by default, has no New only.
    harness.state_mut().services.settings.files_start = FilesStart::FirstFile;

    if !platform.organizations.is_empty() {
        overview(&mut harness);
    }
    let number = match std::env::var("REVIEW_LIVE_PR") {
        Ok(number) => number.parse().expect("REVIEW_LIVE_PR is a number"),
        Err(_) => newest_pull_request(&mut harness, &repository),
    };
    let key = PullRequestKey {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: repository.clone(),
        number,
    };
    harness.state_mut().navigator.go(Route::PullRequest(key));
    let started = Instant::now();
    if std::env::var_os("REVIEW_LIVE_PIPELINES_ONLY").is_some() {
        wait_until(&mut harness, "the pull request", |harness| {
            harness.query_by_role_and_label(Role::Tab, "Overview").is_some()
        });
        println!("pull request ready after {:.1} s", started.elapsed().as_secs_f32());
        settle(&mut harness, Duration::from_secs(2));
    } else {
        wait_until(&mut harness, "the file list", |harness| files_tab(harness).is_some());
        println!("file list ready after {:.1} s", started.elapsed().as_secs_f32());
        settle(&mut harness, Duration::from_secs(2));
        harness.click_at(files_tab(&harness).expect("the Files tab").rect());
        wait_until(&mut harness, "the file tree", |harness| {
            harness.query_all_by_role(Role::TreeItem).next().is_some()
        });
        settle(&mut harness, Duration::from_secs(4));
        if std::env::var_os("REVIEW_LIVE_SKIP_DIFF").is_none() {
            diff_walkthrough(&mut harness);
        }

        harness.click_at(harness.get_by_label_contains("Commits (").rect());
        settle(&mut harness, Duration::from_secs(1));
    }

    let before_tab = pipeline_requests();
    harness.click_at(harness.get_by_label("Pipelines").rect());
    let started = Instant::now();
    // Without a token gitlab.com allows an IP 60 requests an hour; past that the tab waits.
    let is_waiting = |harness: &Harness<'_, ReviewApp>| {
        harness.query_all_by_label_contains("Waiting for ").next().is_some()
    };
    wait_until(&mut harness, "pipelines", |harness| {
        harness.query_all_by_role(Role::Row).next().is_some()
            || harness.query_by_label("No pipelines").is_some()
            || is_waiting(harness)
    });
    println!("pipelines answered after {:.1} s", started.elapsed().as_secs_f32());
    settle(&mut harness, Duration::from_secs(2));
    report_requests("opening the Pipelines tab", &before_tab, &pipeline_requests());
    if is_waiting(&harness) {
        println!("the pipelines wait for a rate limit");
        return;
    }
    // The most urgent run (job rows read "<job> in <run>, <status>") opens its stage graph.
    let run_row = harness
        .query_all_by_role(Role::Row)
        .find(|node| node.accesskit_node().label().is_some_and(|label| !label.contains(" in ")));
    let Some(row) = run_row else {
        return;
    };
    harness.click_at(row.rect());
    settle(&mut harness, Duration::from_secs(8));
    report_stage_columns(&harness);
    older_run_on_demand(&mut harness);
}

/// Back in the checks table, the filter finds the older runs (of earlier commits, or superseded):
/// opening one loads its jobs then, and only then.
fn older_run_on_demand(harness: &mut Harness<'_, ReviewApp>) {
    harness.click_at(harness.get_by_role_and_label(Role::Link, "Pipelines").rect());
    settle(harness, Duration::from_secs(1));
    let filter = harness.get_by_label("Filter checks");
    harness.click_at(filter.rect());
    filter.type_text("older run");
    settle(harness, Duration::from_secs(1));
    let older = harness
        .query_all_by_role(Role::Row)
        .filter_map(|node| node.accesskit_node().label())
        .find(|label| label.ends_with(", older run"));
    let Some(older) = older else {
        println!("every listed run is a head run; no older run to open");
        return;
    };
    let before = pipeline_requests();
    harness.click_at(harness.get_by_label(&older).rect());
    settle(harness, Duration::from_secs(8));
    report_requests(&format!("opening {older}"), &before, &pipeline_requests());
}

/// The CI requests this connection sent so far, by method and path template.
fn pipeline_requests() -> Vec<(String, u64)> {
    let traffic = review_core::http::traffic::of(CONNECTION_ID).unwrap_or_default();
    // GitHub reads a workflow run's file (`contents`) for its stage graph along with its jobs.
    let is_ci = |path: &str| {
        [
            "pipelines",
            "jobs",
            "bridges",
            "graphql",
            "actions",
            "builds",
            "check-runs",
            "status",
            "contents",
        ]
        .iter()
        .any(|part| path.contains(part))
    };
    traffic
        .endpoints
        .into_iter()
        .filter(|endpoint| is_ci(&endpoint.path))
        .map(|endpoint| (format!("{} {}", endpoint.method, endpoint.path), endpoint.count))
        .collect()
}

fn report_requests(what: &str, before: &[(String, u64)], after: &[(String, u64)]) {
    let sent = |endpoint: &str, counts: &[(String, u64)]| {
        counts.iter().find(|(name, _)| name == endpoint).map_or(0, |(_, count)| *count)
    };
    let deltas: Vec<(String, u64)> = after
        .iter()
        .map(|(endpoint, count)| (endpoint.clone(), count - sent(endpoint, before)))
        .filter(|(_, delta)| *delta > 0)
        .collect();
    println!("CI requests for {what}: {}", deltas.iter().map(|(_, delta)| delta).sum::<u64>());
    for (endpoint, delta) in deltas {
        println!("    {delta:>3}  {endpoint}");
    }
}

/// The "Files (12)" tab; rows elsewhere offer "Open Files (changes) of !12" as well.
fn files_tab<'h>(harness: &'h Harness<'_, ReviewApp>) -> Option<egui_kittest::Node<'h>> {
    harness.query_all_by_label_contains("Files (").find(|node| {
        node.accesskit_node().label().is_some_and(|label| label.starts_with("Files ("))
    })
}

/// How the stage cards are placed: one column per left edge. A pipeline of several stages in a
/// single column means the graph lost its order.
fn report_stage_columns(harness: &Harness<'_, ReviewApp>) {
    let mut lefts: Vec<i32> = harness
        .query_all_by_role(Role::Button)
        .filter(|node| {
            node.accesskit_node().label().is_some_and(|label| label.starts_with("Stage "))
        })
        .map(|node| node.rect().left().round() as i32)
        .collect();
    let cards = lefts.len();
    lefts.sort_unstable();
    lefts.dedup();
    println!("{cards} stage cards in {} columns", lefts.len());
}

/// Opens `REVIEW_LIVE_FILE` through the file filter, waits for its diff and syntax colors, and
/// switches it to Inline and New only and back.
fn diff_walkthrough(harness: &mut Harness<'_, ReviewApp>) {
    let memory_before = memory();
    let started = Instant::now();
    let requested = std::env::var("REVIEW_LIVE_FILE");
    // A requested file may be large: its blobs can take a while to fetch before coloring starts.
    let patience = Duration::from_secs(if requested.is_ok() { 60 } else { 3 });
    if let Ok(path) = requested {
        let filter = harness
            .get_by(|node| node.placeholder().is_some_and(|text| text.contains("Filter files")));
        harness.click_at(filter.rect());
        filter.type_text(&path);
        wait_until(harness, "the filtered file", |harness| {
            harness.query_by_role_and_label(Role::TreeItem, &path).is_some()
        });
        // The tree lays out the filtered rows over a frame or two; clicking earlier hits a stale position.
        settle(harness, Duration::from_millis(300));
        harness.click_at(harness.get_by_role_and_label(Role::TreeItem, &path).rect());
    }
    // Both panes of the side-by-side layout are labelled "Diff of …".
    wait_until(harness, "the diff", |harness| {
        harness.query_all_by_label_contains("Diff of ").next().is_some()
    });
    let shown = started.elapsed();
    let memory_shown = memory();
    let mut memory_peak = memory_shown;
    let colored = wait_for_colors(harness, patience, &mut memory_peak);
    let memory_colored = memory();
    println!(
        "diff shown after {:.2} s, colors after {}",
        shown.as_secs_f32(),
        colored.map_or_else(
            || "no progress shown (small file)".to_owned(),
            |elapsed| format!("{:.2} s", elapsed.as_secs_f32())
        ),
    );
    println!(
        "memory before {memory_before}, diff shown {memory_shown}, peak while coloring {memory_peak}, colored {memory_colored}"
    );
    settle(harness, Duration::from_millis(500));

    // An added or deleted file shows whole, without layouts to switch between.
    if harness.query_by_label("Inline").is_some() {
        harness.click_at(harness.get_by_label("Inline").rect());
        settle(harness, Duration::from_millis(500));
        new_only_walkthrough(harness);
        harness.state_mut().services.settings.diff_layout = DiffLayout::SideBySide;
    } else {
        println!("an added or deleted file: no layouts to switch");
    }
    // The coloring thread keeps its compiled grammars for 20 s after the last file.
    settle(harness, Duration::from_secs(25));
    println!("memory once the grammars were released {}", memory());
}

/// Alt+N into New only and Alt+N back to the layout before.
fn new_only_walkthrough(harness: &mut Harness<'_, ReviewApp>) {
    harness.key_press_modifiers(egui::Modifiers::ALT, egui::Key::N);
    settle(harness, Duration::from_millis(500));
    assert_eq!(
        harness.state().services.settings.diff_layout,
        DiffLayout::NewOnly,
        "Alt+N enters New only"
    );

    harness.key_press_modifiers(egui::Modifiers::ALT, egui::Key::N);
    settle(harness, Duration::from_millis(500));
    assert_eq!(
        harness.state().services.settings.diff_layout,
        DiffLayout::Inline,
        "Alt+N returns to the layout before"
    );
}

/// Waits until the coloring progress (shown for large files) has come and gone, sampling the
/// memory peak meanwhile. Returns how long that took, or `None` when no progress appeared within
/// `patience` (a small file colors at once).
fn wait_for_colors(
    harness: &mut Harness<'_, ReviewApp>,
    patience: Duration,
    peak: &mut Memory,
) -> Option<Duration> {
    const SAMPLE_EVERY: Duration = Duration::from_secs(2);
    let started = Instant::now();
    let mut sampled = Instant::now();
    let mut has_seen_progress = false;
    loop {
        harness.step();
        let is_coloring = harness.query_by_label_contains("Coloring syntax").is_some();
        has_seen_progress |= is_coloring;
        if has_seen_progress && !is_coloring {
            return Some(started.elapsed());
        }
        if is_coloring && sampled.elapsed() > SAMPLE_EVERY {
            *peak = peak.max(memory());
            sampled = Instant::now();
        }
        let is_too_large = harness.query_by_label_contains("Syntax colors are off").is_some();
        if is_too_large || (!has_seen_progress && started.elapsed() > patience) {
            return None;
        }
        assert!(started.elapsed() < NETWORK_TIMEOUT, "timed out while coloring");
        cv_testkit::let_threads_run(Duration::from_millis(20));
    }
}

/// The process's heap bytes and private bytes (none where the system does not count them).
#[derive(Clone, Copy)]
struct Memory {
    heap: usize,
    private: Option<usize>,
}

impl Memory {
    /// Each count's larger value.
    fn max(self, other: Self) -> Self {
        Self { heap: self.heap.max(other.heap), private: self.private.max(other.private) }
    }
}

impl std::fmt::Display for Memory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const MB: usize = 1024 * 1024;
        let private =
            self.private.map_or_else(|| "unknown".to_owned(), |bytes| (bytes / MB).to_string());
        write!(formatter, "{} MB heap / {private} MB private", self.heap / MB)
    }
}

fn memory() -> Memory {
    Memory { heap: heap::live_bytes(), private: heap::private_bytes() }
}

/// The start page lists the organizations' open pull requests without a token.
fn overview(harness: &mut Harness<'_, ReviewApp>) {
    let started = Instant::now();
    let has_answer = |harness: &Harness<'_, ReviewApp>| {
        first_row_number(harness).is_some()
            || harness.query_by_label_contains("Check connection").is_some()
    };
    wait_until(harness, "the overview", has_answer);
    let outcome = if first_row_number(harness).is_some() { "rows" } else { "an error card" };
    println!("overview showed {outcome} after {:.1} s", started.elapsed().as_secs_f32());
    if first_row_number(harness).is_none() {
        let labels: Vec<String> = harness
            .query_all_by(|_| true)
            .filter_map(|node| node.accesskit_node().label())
            .collect();
        println!("the error card reads: {labels:?}");
    }
    settle(harness, Duration::from_millis(500));
}

/// Opens the repository's pull request list and returns the newest pull request's number.
fn newest_pull_request(harness: &mut Harness<'_, ReviewApp>, repository: &str) -> u64 {
    harness.state_mut().navigator.go(Route::RepositoryPullRequests {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: repository.to_owned(),
    });
    // One frame applies the navigation, the next paints the new page: rows of the page before
    // (the overview) are gone from the tree from then on.
    harness.step();
    harness.step();
    let has_rows_or_empty_view = |harness: &Harness<'_, ReviewApp>| {
        first_row_number(harness).is_some() || harness.query_by_label("Show all states").is_some()
    };
    wait_until(harness, "the pull request list", has_rows_or_empty_view);
    // Repositories without active pull requests still have completed ones to walk through.
    if first_row_number(harness).is_none() {
        harness.click_at(harness.get_by_label("Show all states").rect());
        wait_until(harness, "pull requests in all states", |harness| {
            first_row_number(harness).is_some()
        });
    }
    settle(harness, Duration::from_millis(500));
    first_row_number(harness).expect("at least one pull request row")
}

/// Breadcrumbs are links too; list rows are the links labeled with their number first.
fn first_row_number(harness: &Harness<'_, ReviewApp>) -> Option<u64> {
    harness
        .query_all_by_role(Role::Link)
        .find_map(|node| node.accesskit_node().label()?.split_whitespace().next()?.parse().ok())
}

/// A very large pull request end to end: the file list, the tree, the first diff, the tree scrolled
/// to its last file, and `REVIEW_LIVE_FILE` (its largest diff) scrolled from top to bottom.
/// `REVIEW_LIVE_REPO` and `REVIEW_LIVE_PR` name the pull request; prints the load times, the
/// memory and the UI frame times (`Harness::step`, without the GPU).
///
/// `REVIEW_LIVE_REPO=microsoft/TypeScript REVIEW_LIVE_PR=51387 REVIEW_LIVE_FILE=src/compiler/checker.ts cargo test -p review-app --release live_large -- --ignored --nocapture`
#[test]
#[ignore = "needs network; run explicitly"]
fn live_large_pull_request() {
    cv_testkit::allow_real_network();
    let platform = Platform::from_environment();
    let repository =
        std::env::var("REVIEW_LIVE_REPO").expect("REVIEW_LIVE_REPO names the repository");
    let number: u64 = std::env::var("REVIEW_LIVE_PR")
        .expect("REVIEW_LIVE_PR names the pull request")
        .parse()
        .expect("a number");
    let scratch = ScratchDir::new("live-large");
    let paths = live_paths(&scratch, &platform);
    let mut harness = realtime_harness(&paths);
    println!("memory at start {}", memory());

    let key = PullRequestKey {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: repository,
        number,
    };
    harness.state_mut().navigator.go(Route::PullRequest(key));
    let started = Instant::now();
    wait_until(&mut harness, "the file list", |harness| files_tab(harness).is_some());
    let files_label =
        files_tab(&harness).and_then(|tab| tab.accesskit_node().label()).unwrap_or_default();
    println!("{files_label} listed after {:.1} s", started.elapsed().as_secs_f32());
    harness.click_at(files_tab(&harness).expect("the Files tab").rect());
    wait_until(&mut harness, "the file tree", |harness| {
        harness.query_all_by_role(Role::TreeItem).next().is_some()
    });
    println!("file tree after {:.1} s, {}", started.elapsed().as_secs_f32(), memory());
    settle(&mut harness, Duration::from_secs(2));

    // With no file open, "next file" opens the first one.
    let first_diff = Instant::now();
    harness.key_press(egui::Key::J);
    wait_until(&mut harness, "the first diff", |harness| diff_label(harness).is_some());
    println!(
        "{} after {:.2} s",
        diff_label(&harness).unwrap_or_default(),
        first_diff.elapsed().as_secs_f32()
    );
    settle(&mut harness, Duration::from_secs(2));

    let mut tree_frames = FrameTimes::default();
    let tree_center =
        harness.query_all_by_role(Role::TreeItem).next().expect("a tree row").rect().center();
    let mut last_row = String::new();
    // Down the whole tree until its last row stays the last.
    for _ in 0..200 {
        scroll(&mut harness, tree_center, 10, -2400.0, &mut tree_frames);
        let bottom = harness
            .query_all_by_role(Role::TreeItem)
            .filter_map(|node| node.accesskit_node().label())
            .next_back()
            .unwrap_or_default();
        if bottom == last_row {
            break;
        }
        last_row = bottom;
    }
    println!("tree scrolled to {last_row}: {}", tree_frames.summary());

    let last_diff = Instant::now();
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, &last_row).rect());
    let expected = format!("Diff of {last_row}");
    wait_until(&mut harness, "the last file's diff", |harness| {
        diff_label(harness).is_some_and(|label| label.starts_with(&expected))
    });
    println!("last file {last_row} after {:.2} s", last_diff.elapsed().as_secs_f32());
    settle(&mut harness, Duration::from_secs(2));

    match std::env::var("REVIEW_LIVE_FILE") {
        Ok(path) => {
            let mut frames = large_diff(&mut harness, &path);
            frames.extend(&tree_frames);
            println!("all scroll frames: {}", frames.summary());
        }
        Err(_) => println!("REVIEW_LIVE_FILE unset: no large diff scrolled"),
    }
    println!("memory at the end {}", memory());
    report_traffic();
}

/// Every request the connection sent, by method and path template.
fn report_traffic() {
    let Some(traffic) = review_core::http::traffic::of(CONNECTION_ID) else {
        return;
    };
    println!(
        "API requests: {} sent, {} unchanged (304), {} rate-limited",
        traffic.sent, traffic.not_modified, traffic.rate_limited
    );
    for endpoint in traffic.endpoints {
        println!("    {:>3}  {} {}", endpoint.count, endpoint.method, endpoint.path);
    }
}

/// Opens `path` through the file filter, waits for its diff and colors, and scrolls it from top to
/// bottom. Returns the frame times of the scrolling.
fn large_diff(harness: &mut Harness<'_, ReviewApp>, path: &str) -> FrameTimes {
    let filter =
        harness.get_by(|node| node.placeholder().is_some_and(|text| text.contains("Filter files")));
    harness.click_at(filter.rect());
    filter.type_text(path);
    wait_until(harness, "the filtered file", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, path).is_some()
    });
    settle(harness, Duration::from_millis(300));
    let started = Instant::now();
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, path).rect());
    let expected = format!("Diff of {path}");
    wait_until(harness, "the large diff", |harness| {
        diff_label(harness).is_some_and(|label| label.starts_with(&expected))
    });
    let shown = started.elapsed();
    let mut peak = memory();
    let colored = wait_for_colors(harness, Duration::from_secs(60), &mut peak);
    println!(
        "{path}: diff shown after {:.2} s, colors after {}, peak while coloring {peak}, {}",
        shown.as_secs_f32(),
        colored.map_or_else(
            || "no progress shown".to_owned(),
            |elapsed| format!("{:.2} s", elapsed.as_secs_f32())
        ),
        memory()
    );
    settle(harness, Duration::from_secs(1));
    let mut frames = FrameTimes::default();
    let center = diff_rect(harness).center();
    for _ in 0..60 {
        scroll(harness, center, 20, -2400.0, &mut frames);
    }
    println!("{path} scrolled: {}", frames.summary());
    // On to the end of the file.
    for _ in 0..400 {
        scroll(harness, center, 20, -24_000.0, &mut frames);
    }
    frames
}

/// The live harness with a 60 Hz simulated clock: waiting for a long fetch in real time then also
/// spaces the live probe like the app would, instead of probing every few real seconds.
fn realtime_harness(paths: &AppPaths) -> Harness<'static, ReviewApp> {
    let paths = paths.clone();
    let builder = Harness::builder()
        .with_size([1440.0, 900.0])
        .with_step_dt(1.0 / 60.0)
        // Real hosts' text may hold characters no font of the app draws.
        .allow_missing_glyphs();
    super::startup::start(builder, move |creation| {
        ReviewApp::new(creation, paths, cv_providers::create_provider)
    })
}

fn diff_label(harness: &Harness<'_, ReviewApp>) -> Option<String> {
    harness
        .query_all_by_role(Role::Document)
        .filter_map(|node| node.accesskit_node().label())
        .find(|label| label.starts_with("Diff of "))
}

fn diff_rect(harness: &Harness<'_, ReviewApp>) -> egui::Rect {
    harness
        .query_all_by_role(Role::Document)
        .find(|node| {
            node.accesskit_node().label().is_some_and(|label| label.starts_with("Diff of "))
        })
        .expect("the diff area")
        .rect()
}

/// `notches` wheel events of `delta` points over `at`, one per frame, then the glide's frames;
/// every frame's UI time goes into `frames`.
fn scroll(
    harness: &mut Harness<'_, ReviewApp>,
    at: egui::Pos2,
    notches: usize,
    delta: f32,
    frames: &mut FrameTimes,
) {
    harness.hover_at(at);
    frames.step(harness);
    for _ in 0..notches {
        harness.event(wheel(delta, egui::TouchPhase::Move, egui::MouseWheelSource::Wheel));
        frames.step(harness);
    }
    for _ in 0..30 {
        frames.step(harness);
    }
}

/// Wall time of `Harness::step`: the app's UI, layout and tessellation of one frame.
#[derive(Default)]
struct FrameTimes {
    samples: Vec<Duration>,
}

impl FrameTimes {
    fn step(&mut self, harness: &mut Harness<'_, ReviewApp>) {
        let started = Instant::now();
        harness.step();
        self.samples.push(started.elapsed());
    }

    fn extend(&mut self, other: &Self) {
        self.samples.extend_from_slice(&other.samples);
    }

    fn summary(&self) -> String {
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let Some(max) = sorted.last() else {
            return "no frames".to_owned();
        };
        let at = |quantile: f64| {
            sorted[((sorted.len() - 1) as f64 * quantile).round() as usize].as_secs_f64() * 1000.0
        };
        format!(
            "{} frames, p50 {:.1} ms, p95 {:.1} ms, p99 {:.1} ms, max {:.1} ms",
            sorted.len(),
            at(0.5),
            at(0.95),
            at(0.99),
            max.as_secs_f64() * 1000.0
        )
    }
}

struct Platform {
    kind: ProviderKind,
    default_repository: &'static str,
    organizations: &'static [&'static str],
}

impl Platform {
    fn from_environment() -> Self {
        let kind = std::env::var("REVIEW_LIVE_KIND").unwrap_or_default();
        match kind.as_str() {
            "gitlab" => Self {
                kind: ProviderKind::GitLab,
                default_repository: "gitlab-org/cli",
                organizations: &["gitlab-org"],
            },
            "azure" => Self {
                kind: ProviderKind::AzureDevOps,
                default_repository: "dnceng-public/public/dotnet-public-wiki",
                organizations: &["dnceng-public"],
            },
            _ => Self {
                kind: ProviderKind::GitHub,
                default_repository: "cli/cli",
                organizations: &[],
            },
        }
    }
}

/// `scratch`'s data folders with the anonymous connection to `platform` saved.
fn live_paths(scratch: &ScratchDir, platform: &Platform) -> AppPaths {
    let connection = Connection {
        name: format!("{} (anonymous)", platform.kind.label()),
        organizations: platform
            .organizations
            .iter()
            .map(|organization| (*organization).to_owned())
            .collect(),
        ..samples::connection(CONNECTION_ID, platform.kind, platform.kind.default_base_url())
    };
    saved_paths(scratch, &[connection])
}

fn wait_until(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    let started = Instant::now();
    loop {
        harness.step();
        if is_done(harness) {
            return;
        }
        // An error screen will not resolve by waiting longer.
        let has_failed = harness.query_by_label("Pull request unavailable").is_some();
        if has_failed || started.elapsed() > NETWORK_TIMEOUT {
            let reason = if has_failed { "the pull request failed to load" } else { "timed out" };
            let tree: String = format!("{:?}", harness.root()).chars().take(8000).collect();
            panic!("{reason} while waiting for {what}; screen:\n{tree}");
        }
        cv_testkit::let_threads_run(Duration::from_millis(40));
    }
}

/// Keeps stepping so background results that arrive meanwhile get drawn.
fn settle(harness: &mut Harness<'_, ReviewApp>, duration: Duration) {
    let started = Instant::now();
    while started.elapsed() < duration {
        harness.step();
        cv_testkit::let_threads_run(Duration::from_millis(40));
    }
}
