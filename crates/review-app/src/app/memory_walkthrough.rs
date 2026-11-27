//! Memory: drives the real app headless against a mock GitLab, as a person reviewing for a while
//! would (several merge requests, every file of each among them a large TypeScript file and
//! Markdown with an image, scrolling, the pipelines with a long job log, the assistant), and reads
//! the heap's live bytes ([`cv_base::platform::heap`]) after each step. The bytes are counted by
//! the app's own allocator, not read from the system, so the numbers repeat from run to run.
//!
//! The heap is process-wide, so each test runs itself again in a child process where it is the
//! only test, and checks the curve there.
//!
//! The measurements (the tests named `memory_*`: a review session of five merge requests until
//! the heap settles, 3000 idle frames, eight languages' grammars) are their own group: they take
//! most of the binary's time, so they compile only with the feature `measurements` and the
//! ordinary `cargo test` leaves them out. They run on `main` and when memory-relevant code
//! changes (caches, grammars, pages kept or dropped), and print their curves:
//! `cargo test -p review-app --bin CodeView --features measurements -- app::memory_walkthrough::memory_ --nocapture`.
//! The hand-run reproductions and probes stay `#[ignore]`, so that command never runs them.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(feature = "measurements")]
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(feature = "measurements")]
use egui_kittest::kittest::NodeT as _;
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, TestRenderer};
#[cfg(feature = "measurements")]
use review_agent::{AgentEvent, Usage};
use review_agent::{AgentKind, ModelInfo};
use review_core::{Connection, ProviderKind};
use review_store::AppPaths;
use serde_json::{Value, json};
use wiremock::{MockServer, Request, Respond, ResponseTemplate};

use super::ReviewApp;
#[cfg(feature = "measurements")]
use super::request_budget::scroll_list;
use super::request_budget::{Text, diff_stats_summaries, harness_builder, user};
use super::startup;
#[cfg(feature = "measurements")]
use super::walkthrough::click_first;
use super::walkthrough::{self, has, pump, saved_paths, wait_idle, wait_until_unsettled};
use cv_base::platform::heap::live_bytes;
use cv_runtime::route::{PullRequestKey, Route};
use cv_services::agents::FakeCli;
#[cfg(feature = "measurements")]
use cv_syntax::code_colors;
#[cfg(any(windows, feature = "measurements"))]
use cv_syntax::{grammar, highlight};
use cv_testkit::{Clicks, ScratchDir, git_in, samples};

const PROJECT: &str = "group/app";
const PROJECT_ID: u64 = 42;
const PIPELINE_ID: u64 = 1001;
/// The merge requests the session reviews, one after the other.
const MERGE_REQUESTS: u64 = 5;
/// Small Rust files each merge request changes, next to the large and the Markdown ones.
const RUST_FILES: usize = 30;
/// Lines of the large TypeScript file: about 1 MB, a generated client or a bundled module.
const LARGE_LINES: usize = 15_000;
/// Lines of the long job log: about 4 MB.
const LOG_LINES: usize = 60_000;
const CHILD_MARKER: &str = "CODEVIEW_MEMORY_TEST_CHILD";
const MB: usize = 1024 * 1024;
/// What one kept pull request page may hold once left: our number, the about 6 MB measured here
/// (its views and the parts the process keeps after the first page; the caches go when it is
/// left, see `cv_pr_files::tab::hibernate`) with room.
#[cfg(feature = "measurements")]
const PAGE_ALLOWANCE: usize = 8 * MB;
/// The open page on top: our number, its diff, color and text caches (which a left page lets go)
/// and a file colored and laid out while the next loads.
#[cfg(feature = "measurements")]
const IN_FLIGHT: usize = 80 * MB;

/// The scripted review session (165 files opened over five merge requests): the live bytes stay
/// within the compiled grammars' budget and the kept pages, and once every merge request was left
/// for the list and the grammars went unused, only the kept pages remain.
#[test]
#[cfg(feature = "measurements")]
fn memory_returns_after_the_merge_requests_are_left() {
    if run_in_child("app::memory_walkthrough::memory_returns_after_the_merge_requests_are_left") {
        return;
    }
    let fixture = Fixture::new("left");
    let textures = GpuTextures::default();
    let mut harness = fixture.harness(&textures);
    let mut curve = Curve { textures, points: Vec::new() };
    wait_until_unsettled(&mut harness, "the merge request list", |harness| {
        has(harness, "Change 30")
    });
    curve.measure(&mut harness, "start: the list");
    for iid in 1..=MERGE_REQUESTS {
        review(&mut harness, &fixture, iid, &mut curve);
        if iid == 1 {
            pipelines_and_log(&mut harness, &mut curve);
            assistant(&mut harness, &mut curve);
        }
    }
    let open_file = current_file(&harness);
    harness.state_mut().navigator.go(Route::MyPullRequests);
    curve.measure(&mut harness, "back on the list");
    pump(&mut harness, 40.0);
    curve.measure(&mut harness, "40 s on the list");
    release_grammars(&mut harness);
    curve.measure(&mut harness, "grammars unused for 5 min dropped");
    fixture.forget_requests();
    curve.measure(&mut harness, "the mock's requests forgotten");
    println!("{curve}");
    let start = curve.heap_at("start: the list");
    let pages = super::OPEN_PULL_REQUESTS * PAGE_ALLOWANCE;
    assert!(
        curve.peak_heap() <= start + grammar::BUDGET_BYTES + pages + IN_FLIGHT,
        "the session stays within the grammar budget and the kept pages: {curve}"
    );
    assert!(
        curve.heap_at("the mock's requests forgotten") <= start + pages,
        "only the kept pages stay once the merge requests are left: {curve}"
    );
    assert!(curve.peak_textures() <= 16 * MB, "the textures stay small: {curve}");
    // The last merge request shows again where it was left, its open file's diff loaded anew.
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key(MERGE_REQUESTS)));
    wait_until_unsettled(&mut harness, "the open file's diff", |_| {
        cv_runtime::cache::memory::holders()
            .iter()
            .any(|(name, bytes)| *name == "diffs" && *bytes > 0)
    });
    assert!(open_file.is_some(), "a file was open when the merge request was left");
    assert_eq!(current_file(&harness), open_file, "the page keeps its open file");
    harness.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::F);
    harness.step();
    assert!(has(&harness, "heap ") && has(&harness, " MB live"), "the overlay shows the heap");
    assert!(has(&harness, "diffs "), "the overlay names the largest holders");
}

/// Rounds of the idle test; each runs [`IDLE_FRAMES`] frames of [`IDLE_STEP`] seconds.
#[cfg(feature = "measurements")]
const IDLE_ROUNDS: usize = 3;
#[cfg(feature = "measurements")]
const IDLE_FRAMES: usize = 1000;
const IDLE_STEP: f64 = 0.5;
/// What the idle Overview may gain over all rounds: our number, the 1 MB the measurement settles
/// within. The rounds run 25 simulated minutes, 100 polls of the running pipeline at 15 s, so a
/// poll that kept 10 KB fails it.
const IDLE_GROWTH: usize = MB;

/// A merge request's Overview left open with a running pipeline: the pipeline polls every 15 s
/// and the live probe runs, for 25 simulated minutes. The live bytes stay where they were once the
/// page had loaded, and nothing but those timers asks for a frame.
#[test]
#[cfg(feature = "measurements")]
fn memory_stays_flat_while_the_overview_idles() {
    if run_in_child("app::memory_walkthrough::memory_stays_flat_while_the_overview_idles") {
        return;
    }
    let fixture = Fixture::new("idle");
    let textures = GpuTextures::default();
    let mut harness = fixture.harness(&textures);
    // The card lists the pipelines with their stage bars, so the stages poll too.
    harness.state_mut().services.settings.pipelines.is_overview_expanded = true;
    let mut curve = Curve { textures, points: Vec::new() };
    wait_until_unsettled(&mut harness, "the merge request list", |harness| {
        has(harness, "Change 30")
    });
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key(IDLE_MERGE_REQUEST)));
    wait_until_unsettled(&mut harness, "the overview", |harness| {
        has(harness, &title(IDLE_MERGE_REQUEST))
    });
    wait_until_unsettled(&mut harness, "the picture", |harness| has(harness, "The upload"));
    fixture.forget_requests();
    curve.measure(&mut harness, "overview loaded");
    let mut causes = std::collections::BTreeSet::new();
    for round in 1..=IDLE_ROUNDS {
        let mut time = harness.ctx.input(|input| input.time);
        for _ in 0..IDLE_FRAMES {
            time += IDLE_STEP;
            harness.input_mut().time = Some(time);
            harness.step();
            causes.extend(harness.ctx.repaint_causes().iter().map(|cause| cause.file.to_owned()));
            cv_testkit::let_threads_run(Duration::from_millis(3));
        }
        fixture.forget_requests();
        curve.measure(&mut harness, &format!("idle {} min", round * IDLE_FRAMES / 120));
    }
    println!("{curve}");
    let loaded = curve.heap_at("overview loaded");
    assert!(
        curve.peak_heap() <= loaded + IDLE_GROWTH,
        "the idle Overview keeps nothing per poll or frame: {curve}"
    );
    // The renderer keeps textures outside the heap counted here: none may come per poll or frame.
    assert!(
        curve.are_textures_flat_after("overview loaded"),
        "the idle Overview uploads no texture and grows no font atlas: {curve}"
    );
    let timers = ["cv-runtime/src/refresh.rs", "cv-runtime/src/background.rs"];
    let others: Vec<_> = causes
        .iter()
        .filter(|file| !timers.iter().any(|timer| file.replace('\\', "/").ends_with(timer)))
        .collect();
    assert!(
        others.is_empty(),
        "only the poll timers and finished tasks ask for frames: {others:?}"
    );
}

/// A closed app frees its egui context and everything the context holds (fonts, textures, the
/// background runtime's driver), also after a pull request page published its images to it: the
/// context keeps the page's image source only weakly, so the source's tasks, which hold the
/// context, cannot keep it alive.
#[test]
fn a_closed_app_frees_its_context() {
    struct Dropped(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let fixture = Fixture::new("closed");
    let mut harness = fixture.harness(&GpuTextures::default());
    wait_until_unsettled(&mut harness, "the merge request list", |harness| {
        has(harness, "Change 30")
    });
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key(IDLE_MERGE_REQUEST)));
    wait_until_unsettled(&mut harness, "the picture", |harness| has(harness, "The upload"));
    wait_idle(&mut harness);
    let is_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The context owns its repaint callback: the marker goes when the context goes.
    let marker = Dropped(is_dropped.clone());
    harness.ctx.set_request_repaint_callback(move |_| {
        let _marker = &marker;
    });
    drop(harness);
    let started = Instant::now();
    while !is_dropped.load(Ordering::SeqCst) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the closed app's context stays alive"
        );
        cv_testkit::let_threads_run(Duration::from_millis(20));
    }
}

/// The merge request of the owner's session: its own head, a running pipeline.
const OWNER_MERGE_REQUEST: u64 = 1;
/// Commits on the adopted checkout's branch that the merge request lacks, as in the owner's.
const LOCAL_COMMITS: usize = 531;

/// The owner's session, condition by condition: their settings (assistant and mini chat on,
/// UI scale 1.5, 45 fps, 50 context lines, side by side, a large window, two search folders),
/// an adopted checkout with an empty head whose update 531 local commits block, the checkout's
/// menu opened and closed, a real-sized merge request, then the Overview idling with its polls.
/// `OWNER_OFF=settings,record,menu,heavy` leaves conditions out, `OWNER_STILL=runs,notes,jobs`
/// keeps that part of the mock's answers unchanged, `OWNER_ROUNDS`, `OWNER_FRAMES` and
/// `OWNER_STEP` set the idle and `OWNER_HOVER=1` sweeps the pointer over the window meanwhile.
#[test]
#[ignore = "a reproduction run by hand, see the doc comment"]
fn memory_owner_session() {
    if run_in_child("app::memory_walkthrough::memory_owner_session") {
        return;
    }
    let off = std::env::var("OWNER_OFF").unwrap_or_default();
    let is_on = |condition: &str| !off.split(',').any(|item| item.trim() == condition);
    let number = |name: &str, default: f64| {
        std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default)
    };
    let fixture = if is_on("heavy") { Fixture::heavy("owner") } else { Fixture::new("owner") };
    let (mut harness, mut curve) = owner_overview(&fixture, &is_on);
    let rounds = number("OWNER_ROUNDS", 4.0) as usize;
    let frames = number("OWNER_FRAMES", 1000.0) as usize;
    let step = number("OWNER_STEP", IDLE_STEP);
    let mut causes = std::collections::BTreeMap::<String, usize>::new();
    let mut simulated = 0.0;
    // The pointer sweeps the window, a frame at each spot, as a hand on the mouse.
    let is_hovering = std::env::var_os("OWNER_HOVER").is_some();
    let screen = harness.ctx.content_rect();
    let mut spot = 0_usize;
    for round in 1..=rounds {
        let mut time = harness.ctx.input(|input| input.time);
        for _ in 0..frames {
            time += step;
            simulated += step;
            harness.input_mut().time = Some(time);
            if is_hovering {
                spot += 1;
                let (column, row) = ((spot * 37) % 100, (spot / 100 * 13) % 100);
                let x = screen.left() + screen.width() * column as f32 / 100.0;
                let y = screen.top() + screen.height() * row as f32 / 100.0;
                harness.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(x, y)));
            }
            harness.step();
            for cause in harness.ctx.repaint_causes() {
                *causes.entry(format!("{}:{}", cause.file, cause.line)).or_default() += 1;
            }
            cv_testkit::let_threads_run(Duration::from_millis(3));
        }
        fixture.forget_requests();
        curve.measure(&mut harness, &format!("round {round}: {simulated:.0} s idle"));
    }
    println!("{curve}");
    println!("=== Repaint causes ===\n{causes:#?}");
    let loaded = curve.heap_at("overview loaded");
    assert!(
        curve.peak_heap() <= loaded + IDLE_GROWTH,
        "the owner's idle Overview keeps nothing per poll or frame: {curve}"
    );
}

/// The app with the owner's conditions that `is_on`, on the Overview of their merge request once
/// it loaded (the checkout's update blocked, its menu opened and closed), measured once.
fn owner_overview(
    fixture: &Fixture,
    is_on: &dyn Fn(&str) -> bool,
) -> (Harness<'static, ReviewApp>, Curve) {
    let projects = fixture.scratch.path().join("RiderProjects");
    std::fs::create_dir_all(&projects).expect("projects folder");
    if is_on("settings") {
        let folders = [fixture.scratch.path().to_path_buf(), projects.clone()];
        owner_settings(&folders).save(&fixture.paths.settings_file()).expect("settings");
    }
    if is_on("record") {
        adopted_checkout(fixture, &projects, OWNER_MERGE_REQUEST);
    }
    let textures = GpuTextures::default();
    // The owner's 2249 × 2119 window at the UI scale of 1.5.
    let builder = harness_builder().with_size([1499.0, 1413.0]).renderer(textures.clone());
    let mut harness = fixture.harness_from(builder);
    let mut curve = Curve { textures, points: Vec::new() };
    wait_until_unsettled(&mut harness, "the merge request list", |harness| {
        has(harness, "Change 30")
    });
    let key = fixture.key(OWNER_MERGE_REQUEST);
    harness.state_mut().navigator.go(Route::PullRequest(key.clone()));
    wait_until_unsettled(&mut harness, "the overview", |harness| {
        has(harness, &title(OWNER_MERGE_REQUEST))
    });
    if is_on("record") {
        wait_until_unsettled(&mut harness, "the blocked update", |harness| {
            let checkouts = &harness.state().services.checkouts;
            checkouts.status(&key).is_some_and(|status| status.blocked.is_some())
        });
    }
    if is_on("menu") {
        let label = if is_on("record") { "Checkout options" } else { "Check out" };
        wait_until_unsettled(&mut harness, "the checkout button", |harness| has(harness, label));
        harness.click_at(harness.get_by_label(label).rect());
        pump(&mut harness, 2.0);
        if is_on("record") && has(&harness, "Show log") {
            harness.click_at(harness.get_by_label("Show log").rect());
            pump(&mut harness, 2.0);
        }
        harness.key_press(egui::Key::Escape);
        pump(&mut harness, 1.0);
    }
    wait_idle(&mut harness);
    fixture.forget_requests();
    curve.measure(&mut harness, "overview loaded");
    (harness, curve)
}

/// The owner's settings.json, the search folders and the window aside.
fn owner_settings(folders: &[PathBuf]) -> review_store::AppSettings {
    let mut settings: review_store::AppSettings =
        serde_json::from_str(OWNER_SETTINGS).expect("the owner's settings parse");
    settings.checkouts.search_folders = Some(folders.to_vec());
    settings
}

const OWNER_SETTINGS: &str = r#"{
  "theme": "System", "diff_layout": "SideBySide", "context_lines": 50, "ui_scale": 1.5,
  "code_font_size": 11.0, "wrap_lines": false, "ignore_whitespace": false,
  "show_comments_in_diff": true, "show_frame_stats": false, "frame_rate": { "Fps": 45 },
  "panes": { "file_tree": 427.8961, "diff_split": 0.53516686, "overview_side": 557.801,
    "preview_split": 0.46689987, "pipeline_jobs": 300.0, "threads_panel": 599.33325 },
  "pipelines": { "status_filter": [], "is_overview_expanded": true },
  "file_filters": { "categories": ["Code"], "hide_viewed": false, "markdown_preview": true,
    "image_compare": "SideBySide" },
  "threads_panel": { "is_open": false,
    "filters": { "active": true, "resolved": false, "unread": false, "mine": false } },
  "window": { "x": 1460, "y": 0, "width": 2249, "height": 2119, "is_maximized": false },
  "focus_file_tree": false,
  "favorites": { "repositories": [], "is_lane_shown": true, "is_collapsed": false },
  "files_start": "AllFiles", "pull_request_start": "Overview", "open_clipboard_links": true,
  "rendered_width": { "full": true, "reading": 787.4285 }, "file_tree_side": "Left",
  "assistant": { "enabled": true, "agent": "Claude", "claude_model": "Fastest",
    "codex_model": { "Named": "gpt-6.1-sol" }, "card_height": 526.9791,
    "mini_chat_beside_code": true, "mini_chat_corner": { "left": false, "top": false } },
  "header_modes": { "overview": "AlwaysShown", "files": "HideOnScroll", "commits": "AlwaysShown",
    "pipelines": "AlwaysShown" },
  "scroll_past_end": true, "highlight_selection_occurrences": true, "open_next_unviewed": true,
  "comment_editors": { "github": "Rich", "gitlab": "Rich", "azure_devops": "Rich" },
  "checkouts": { "search_folders": [], "worktree_pattern": "{clone}-wt/pr-{number}-{branch}",
    "when_finished": "RemoveAutomatically", "open_by_default": "Ask", "terminal": "System",
    "last_opened": "Nothing", "pinned_clones": {}, "remembered": {} }
}"#;

/// A clone of the fixture's repository in `projects` whose branch `feat/invoicing`, checked out
/// in a worktree beside it, has [`LOCAL_COMMITS`] commits the merge request lacks, recorded as
/// an adopted checkout of merge request `iid` with an empty head (as "Open it" records one).
fn adopted_checkout(fixture: &Fixture, projects: &Path, iid: u64) {
    let clone = projects.join("app");
    let remote = fixture.scratch.path().join("remote.git");
    git_in(None, &["clone", "-q", &remote.display().to_string(), &clone.display().to_string()]);
    let mut stream = Vec::new();
    for index in 0..LOCAL_COMMITS {
        let mark = index + 1;
        write!(
            Text(&mut stream),
            "commit refs/heads/feat/invoicing\nmark :{mark}\ncommitter Dev <dev@example.com> {} \
             +0000\ndata 12\ninvoice {index:04}\n",
            1_710_000_000 + mark
        )
        .expect("writing to a buffer");
        if index == 0 {
            writeln!(Text(&mut stream), "from {}", fixture.gitlab.base).expect("writing");
        }
        let content = format!("invoice line {index}\n");
        write!(
            Text(&mut stream),
            "M 100644 inline invoicing/i{index:04}.txt\ndata {}\n{content}\n",
            content.len()
        )
        .expect("writing to a buffer");
    }
    let git_dir = clone.join(".git");
    let mut child = Command::new("git")
        .args(["--git-dir", &git_dir.display().to_string(), "fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("git fast-import");
    std::io::Write::write_all(child.stdin.as_mut().expect("stdin"), &stream)
        .expect("feed fast-import");
    drop(child.stdin.take());
    assert!(child.wait().expect("fast-import exit").success(), "fast-import failed");
    let worktree = projects.join("app-invoicing");
    cv_testkit::git(
        &clone,
        &["worktree", "add", "-q", &worktree.display().to_string(), "feat/invoicing"],
    );
    let record = json!([{
        "connection_id": fixture.connection_id, "repository_key": PROJECT, "number": iid,
        "clone": clone, "path": worktree, "kind": "Adopted", "branch": "feat/invoicing",
        "is_own_branch": false, "pull_ref": format!("refs/codeview/pull/{iid}/head"), "head": "",
        "last_opened": "Nothing", "took_ms": 0, "merged_here": false, "log": [],
    }]);
    std::fs::create_dir_all(fixture.paths.config_dir()).expect("config folder");
    std::fs::write(fixture.paths.checkouts_file(), record.to_string()).expect("checkouts.json");
}

/// After a restart the list and a merge request's overview paint their last answers from the disk
/// tier at once, each saying it is updating while the host takes its time; the list's note leaves
/// with the answer.
#[test]
fn saved_pages_show_at_once_after_a_restart() {
    let fixture = Fixture::new("restart");
    {
        let mut harness = fixture.harness(&GpuTextures::default());
        wait_until_unsettled(&mut harness, "the merge request list", |harness| {
            has(harness, "Change 30")
        });
        harness.state_mut().navigator.go(Route::PullRequest(fixture.key(1)));
        wait_until_unsettled(&mut harness, "the overview", |harness| has(harness, &title(1)));
        wait_idle(&mut harness);
        let saved = fixture.paths.cold_cache_dir().join("page");
        let started = Instant::now();
        // The list, the overview and its threads, written on the cold tier's own thread.
        while std::fs::read_dir(&saved).map_or(0, Iterator::count) < 3 {
            assert!(started.elapsed() < Duration::from_secs(30), "the pages reach the disk tier");
            cv_testkit::let_threads_run(Duration::from_millis(20));
        }
    }
    // Every answer of the restarted app's host comes this long after its request, and no request
    // leaves before the restart: whatever shows sooner after it came from the disk.
    let host_delay = Duration::from_secs(10);
    fixture.delay.store(host_delay.as_millis() as u64, Ordering::Relaxed);
    let restarted_at = Instant::now();
    let mut harness = fixture.restarted();
    let shows = |harness: &mut Harness<'_, ReviewApp>,
                 done: &dyn Fn(&Harness<'_, ReviewApp>) -> bool| {
        while !done(harness) {
            assert!(
                restarted_at.elapsed() < host_delay,
                "the saved page shows before the host can answer"
            );
            harness.step();
            cv_testkit::let_threads_run(Duration::from_millis(10));
        }
    };
    shows(&mut harness, &|harness| has(harness, "Change 30") && has(harness, "Updating…"));
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key(1)));
    shows(&mut harness, &|harness| has(harness, &title(1)) && has(harness, "Updating…"));
    harness.state_mut().navigator.go(Route::MyPullRequests);
    wait_until_unsettled(&mut harness, "the list's answer", |harness| {
        has(harness, "Change 30") && !has(harness, "Updating…")
    });
}

/// Opens merge request `iid`, every one of its files in turn (scrolling the large one), and
/// measures.
#[cfg(feature = "measurements")]
fn review(harness: &mut Harness<'_, ReviewApp>, fixture: &Fixture, iid: u64, curve: &mut Curve) {
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key(iid)));
    wait_until_unsettled(harness, "the overview", |harness| has(harness, &title(iid)));
    wait_idle(harness);
    curve.measure(harness, &format!("MR {iid}: overview"));
    let files_label = format!("Files ({})", RUST_FILES + 3);
    wait_until_unsettled(harness, "the file list", |harness| {
        harness.query_by_label(&files_label).is_some()
    });
    harness.click_at(harness.get_by_label(&files_label).rect());
    wait_until_unsettled(harness, "the first file", |harness| {
        current_file(harness).as_deref() == Some("docs/README.md")
    });
    wait_until_unsettled(harness, "the README's image", |harness| has(harness, "The screen"));
    for _ in 0..RUST_FILES + 3 {
        harness.key_press(egui::Key::J);
        pump(harness, 0.5);
        if current_file(harness).is_some_and(|path| path.ends_with("app.ts")) {
            wait_idle(harness);
            scroll_list(harness, 20_000.0);
        }
    }
    wait_idle(harness);
    curve.measure(harness, &format!("MR {iid}: all files"));
}

#[cfg(feature = "measurements")]
fn pipelines_and_log(harness: &mut Harness<'_, ReviewApp>, curve: &mut Curve) {
    harness.click_at(harness.get_by_label("Pipelines").rect());
    wait_until_unsettled(harness, "the jobs", |harness| has(harness, "test-3-1"));
    harness.get_by_label("Filter checks").focus();
    harness.step();
    harness.get_by_label("Filter checks").type_text("test-3-2");
    harness.step();
    click_first(harness, "test-3-2 in");
    let lines = format!("{} lines", cv_widgets::numbers::group_digits(LOG_LINES));
    wait_until_unsettled(harness, "the job log", |harness| has(harness, &lines));
    curve.measure(harness, "MR 1: pipelines and a long log");
}

#[cfg(feature = "measurements")]
fn assistant(harness: &mut Harness<'_, ReviewApp>, curve: &mut Curve) {
    harness.click_at(harness.get_by_label("Overview").rect());
    wait_until_unsettled(harness, "the assistant's card", |harness| {
        harness.query_by_label("Ask about this PR").is_some()
    });
    super::walkthrough::type_into(harness, "Ask anything about this pull request", "Explain it.");
    harness.key_press(egui::Key::Enter);
    harness.step();
    let started = Instant::now();
    let (_, agent) = loop {
        if let Some(session) = harness.state().services.agents.fake.take_session() {
            break session;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "no assistant session");
        harness.step();
        cv_testkit::let_threads_run(Duration::from_millis(10));
    };
    agent.emit(AgentEvent::Started { session_id: "s-1".into(), model: "claude-haiku".into() });
    let answer: String = (0..400).fold(String::new(), |mut answer, line| {
        writeln!(answer, "Line {line} of a long answer about `web/app.ts`.\n")
            .expect("writing to a string");
        answer
    });
    agent.emit(AgentEvent::Text(answer));
    let usage = Usage { input: 1_200, output: 340, cache_read: 12_000, cache_write: 0 };
    agent.emit(AgentEvent::TurnDone(usage));
    wait_until_unsettled(harness, "the answer", |harness| {
        has(harness, "Line 399 of a long answer")
    });
    curve.measure(harness, "MR 1: assistant answered");
}

/// The file open in the Files tab: the path its header shows before its "Copy path" button.
#[cfg(feature = "measurements")]
fn current_file(harness: &Harness<'_, ReviewApp>) -> Option<String> {
    let mut path = None;
    for node in harness.query_all_by(|_| true) {
        // A plain text node carries its text as the value.
        let Some(text) = node.accesskit_node().label().or_else(|| node.value()) else {
            continue;
        };
        if text == "Copy path" {
            return path;
        }
        path = Some(text);
    }
    None
}

/// Drops the compiled grammars as the sweeper does once they were unused for five minutes,
/// after the colorings still running (whole files of the pages kept) let go of theirs.
#[cfg(feature = "measurements")]
fn release_grammars(harness: &mut Harness<'_, ReviewApp>) {
    let started = Instant::now();
    loop {
        grammar::release_unused();
        if grammar::kept_languages().is_empty() {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(120), "a coloring never ended");
        harness.step();
        cv_testkit::let_threads_run(Duration::from_millis(100));
    }
}

/// The textures the app has on the GPU, by the uploads and frees egui hands the renderer. The
/// harness's own renderer would keep every upload on the heap until a render that never comes.
#[derive(Clone, Default)]
struct GpuTextures(Arc<Mutex<HashMap<egui::TextureId, usize>>>);

impl GpuTextures {
    fn bytes(&self) -> usize {
        self.0.lock().expect("texture map").values().sum()
    }
}

impl TestRenderer for GpuTextures {
    fn handle_delta(&mut self, delta: &mut egui::TexturesDelta) {
        let mut textures = self.0.lock().expect("texture map");
        for (id, images) in &delta.set {
            for image in images.iter().filter(|image| image.pos.is_none()) {
                let [width, height] = image.image.size();
                textures.insert(*id, width * height * 4);
            }
        }
        for id in &delta.free {
            textures.remove(id);
        }
        delta.clear();
    }
}

/// Live heap bytes and GPU texture bytes after each step of the session.
struct Curve {
    textures: GpuTextures,
    points: Vec<Point>,
}

struct Point {
    step: String,
    heap: usize,
    textures: usize,
    /// Textures egui's manager holds, by count: a texture loaded per poll or per frame shows here
    /// first, whatever its size.
    texture_count: usize,
    /// The font atlas's width and height.
    atlas: [usize; 2],
}

impl Curve {
    fn heap_at(&self, step: &str) -> usize {
        self.points.iter().find(|point| point.step == step).map_or(0, |point| point.heap)
    }

    fn peak_heap(&self) -> usize {
        self.points.iter().map(|point| point.heap).max().unwrap_or(0)
    }

    #[cfg(feature = "measurements")]
    fn peak_textures(&self) -> usize {
        self.points.iter().map(|point| point.textures).max().unwrap_or(0)
    }

    /// Whether every point after `step` holds the textures and the font atlas `step` held.
    #[cfg(feature = "measurements")]
    fn are_textures_flat_after(&self, step: &str) -> bool {
        let Some(start) = self.points.iter().position(|point| point.step == step) else {
            return false;
        };
        let first = &self.points[start];
        self.points[start..].iter().all(|point| {
            (point.texture_count, point.textures, point.atlas)
                == (first.texture_count, first.textures, first.atlas)
        })
    }

    /// Lets the background work of the step finish (the heap stops changing), then records it.
    fn measure(&mut self, harness: &mut Harness<'_, ReviewApp>, step: &str) {
        let heap = settled_bytes(harness);
        let holders = cv_runtime::cache::memory::holders()
            .iter()
            .filter(|(_, bytes)| *bytes > 0)
            .fold(String::new(), |mut holders, (name, bytes)| {
                write!(holders, " {name} {}", megabytes(*bytes)).expect("writing to a string");
                holders
            });
        let (texture_count, atlas) = {
            let manager = harness.ctx.tex_manager();
            let manager = manager.read();
            // The font atlas is the first managed texture.
            let atlas = manager.meta(egui::TextureId::default()).map_or([0, 0], |meta| meta.size);
            (manager.num_allocated(), atlas)
        };
        let point = Point {
            step: step.to_owned(),
            heap,
            textures: self.textures.bytes(),
            texture_count,
            atlas,
        };
        println!("{point}  [{holders} ]");
        self.points.push(point);
    }
}

impl std::fmt::Display for Point {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (heap, textures) = (megabytes(self.heap), megabytes(self.textures));
        let [width, height] = self.atlas;
        write!(
            formatter,
            "{heap:>7} MB heap {textures:>6} MB in {:>3} textures, atlas {width}x{height}  {}",
            self.texture_count, self.step
        )
    }
}

impl std::fmt::Display for Curve {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "\n=== Memory by step ===")?;
        for point in &self.points {
            writeln!(formatter, "{point}")?;
        }
        Ok(())
    }
}

fn megabytes(bytes: usize) -> String {
    format!("{:.1}", bytes as f64 / MB as f64)
}

/// The live bytes once they stopped moving: frames run and background threads work until ten
/// samples 100 ms apart stay within 1 MB of each other (for at most two minutes).
fn settled_bytes(harness: &mut Harness<'_, ReviewApp>) -> usize {
    wait_idle(harness);
    let started = Instant::now();
    let mut samples: Vec<usize> = Vec::new();
    loop {
        harness.step();
        cv_testkit::let_threads_run(Duration::from_millis(100));
        samples.push(live_bytes());
        let recent = &samples[samples.len().saturating_sub(10)..];
        let spread = recent.iter().max().unwrap_or(&0) - recent.iter().min().unwrap_or(&0);
        if recent.len() == 10 && spread < MB {
            return *recent.last().unwrap_or(&0);
        }
        assert!(started.elapsed() < Duration::from_secs(120), "the heap never settled");
    }
}

/// Runs the test `name` again in a child process, alone, and returns true here (the parent), which
/// then has nothing left to do; returns false in that child, which runs the test's body.
#[expect(clippy::disallowed_methods, reason = "a test, not the UI thread, waits for its child")]
fn run_in_child(name: &str) -> bool {
    if std::env::var_os(CHILD_MARKER).is_some() {
        return false;
    }
    let output = Command::new(std::env::current_exe().expect("the test binary has a path"))
        .args(["--exact", name, "--include-ignored", "--test-threads=1", "--nocapture"])
        .env(CHILD_MARKER, "1")
        .output()
        .expect("the test binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("{stdout}");
    assert!(output.status.success(), "{stdout}{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("1 passed"), "the child ran the session: {stdout}");
    true
}

/// The merge request the idle test sits on: its description shows an uploaded picture.
const IDLE_MERGE_REQUEST: u64 = MERGE_REQUESTS + 1;
const UPLOAD_SECRET: &str = "0123456789abcdef0123456789abcdef";
const LARGE_FILE: &str = "web/app.ts";
const MARKDOWN_FILE: &str = "docs/README.md";
const IMAGE_FILE: &str = "docs/shot.png";

fn title(iid: u64) -> String {
    format!("Memory scenario {iid}")
}

struct Fixture {
    runtime: tokio::runtime::Runtime,
    server: MockServer,
    gitlab: MockGitLab,
    paths: AppPaths,
    connection_id: String,
    /// The mock host's answer delay in milliseconds ([`MockGitLab::delay`]).
    delay: Arc<AtomicU64>,
    scratch: ScratchDir,
}

impl Fixture {
    fn new(name: &str) -> Self {
        Self::build(name, false)
    }

    /// [`Self::new`] whose merge requests read like real ones: a long description, many threads,
    /// a pipeline with many jobs ([`MockGitLab::heavy`]).
    fn heavy(name: &str) -> Self {
        Self::build(name, true)
    }

    fn build(name: &str, heavy: bool) -> Self {
        let scratch = ScratchDir::new(&format!("memory-{name}"));
        let repository = scratch.path().join("remote.git");
        let (base, heads) = build_repository(&repository);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("mock runtime");
        // Not recording: the requests would grow the heap this test measures.
        let server = runtime.block_on(MockServer::builder().disable_request_recording().start());
        cv_testkit::fresh_origin(&server.uri());
        let delay = Arc::new(AtomicU64::new(0));
        let gitlab = MockGitLab {
            base_url: server.uri(),
            clone_url: format!("file:///{}", repository.display().to_string().replace('\\', "/")),
            base,
            heads: Arc::new(heads),
            log: Arc::new(long_log()),
            delay: delay.clone(),
            heavy,
            polls: Arc::default(),
        };
        runtime.block_on(
            wiremock::Mock::given(wiremock::matchers::any())
                .respond_with(gitlab.clone())
                .mount(&server),
        );
        let connection_id = format!("memory-{name}");
        let connection = Connection {
            name: "Mock GitLab".to_owned(),
            organizations: vec!["group".to_owned()],
            token: Some("glpat-mock-only".to_owned()),
            ..samples::connection(&connection_id, ProviderKind::GitLab, &server.uri())
        };
        let paths = saved_paths(&scratch, &[connection]);
        Self { runtime, server, gitlab, paths, connection_id, delay, scratch }
    }

    /// Mounts the mock afresh: wiremock keeps every request its mock matched (with the body that
    /// pins the connection's read buffer, about 8 KB a request), which a long idle would count as
    /// the app's.
    fn forget_requests(&self) {
        self.runtime.block_on(async {
            self.server.reset().await;
            wiremock::Mock::given(wiremock::matchers::any())
                .respond_with(self.gitlab.clone())
                .mount(&self.server)
                .await;
        });
    }

    fn harness(&self, textures: &GpuTextures) -> Harness<'static, ReviewApp> {
        self.harness_from(harness_builder().renderer(textures.clone()))
    }

    /// The app in its first frames after a restart, the refresh of the list still running.
    fn restarted(&self) -> Harness<'static, ReviewApp> {
        let builder = harness_builder().renderer(GpuTextures::default());
        let paths = self.paths.clone();
        Self::prepared(startup::first_frames(builder, |creation| {
            ReviewApp::new(creation, paths, cv_providers::create_provider)
        }))
    }

    /// The app from `builder`, once its start settled.
    fn harness_from(
        &self,
        builder: egui_kittest::HarnessBuilder<ReviewApp>,
    ) -> Harness<'static, ReviewApp> {
        let harness =
            walkthrough::app_with(builder, self.paths.clone(), cv_providers::create_provider);
        Self::prepared(harness)
    }

    /// `harness` with the first file opening and a signed-in coding agent.
    fn prepared(mut harness: Harness<'static, ReviewApp>) -> Harness<'static, ReviewApp> {
        harness.state_mut().services.settings.files_start = review_store::FilesStart::FirstFile;
        let model = ModelInfo {
            id: "claude-haiku".to_owned(),
            display_name: "Haiku".to_owned(),
            description: String::new(),
            is_default: true,
            efforts: Vec::new(),
        };
        let cli = FakeCli::Installed { models: vec![model], is_signed_in: true };
        harness.state().services.agents.fake.set(AgentKind::Claude, cli);
        harness
    }

    fn key(&self, iid: u64) -> PullRequestKey {
        PullRequestKey {
            connection_id: self.connection_id.clone(),
            repository_key: PROJECT.to_owned(),
            number: iid,
        }
    }
}

/// A scripted GitLab REST API v4: 30 merge requests; 1 to [`MERGE_REQUESTS`] have their own head
/// commit with a running pipeline (40 jobs) whose job logs are [`LOG_LINES`] long.
#[derive(Clone)]
struct MockGitLab {
    base_url: String,
    clone_url: String,
    base: String,
    heads: Arc<Vec<String>>,
    log: Arc<String>,
    /// How long every answer takes, in milliseconds: a slow host.
    delay: Arc<AtomicU64>,
    /// Real-sized merge requests: [`HEAVY_SECTIONS`] sections of description, [`HEAVY_THREADS`]
    /// threads and [`HEAVY_STAGES`] stages of jobs.
    heavy: bool,
    /// Polls of the pipelines so far: a heavy mock's answers move on with each, as a real host's
    /// do (the running job advances, a new run starts every [`POLLS_PER_RUN`], notes arrive).
    polls: Arc<AtomicU64>,
}

impl Respond for MockGitLab {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let delay = Duration::from_millis(self.delay.load(Ordering::Relaxed));
        let path = request.url.path().to_owned();
        let project = format!("/api/v4/projects/{}", urlencoding::encode(PROJECT));
        let merge_request = path
            .strip_prefix(&format!("{project}/merge_requests/"))
            .map(|rest| rest.split_once('/').map_or((rest, ""), |(iid, tail)| (iid, tail)))
            .and_then(|(iid, tail)| Some((iid.parse::<u64>().ok()?, tail)));
        let body = match (path.as_str(), merge_request) {
            ("/api/v4/user", _) => user(1, "ada"),
            ("/api/graphql", _) => diff_stats_summaries(request),
            ("/api/v4/groups/group/merge_requests", _) => self.list(),
            (p, _) if p == project => json!({
                "id": PROJECT_ID, "name": "app", "path_with_namespace": PROJECT,
                "web_url": format!("{}/{PROJECT}", self.base_url), "default_branch": "main",
                "http_url_to_repo": self.clone_url,
            }),
            (p, _) if p == format!("{project}/merge_requests") => self.list(),
            (_, Some((iid, ""))) => self.merge_request(iid),
            (_, Some((iid, "discussions"))) if self.heavy => {
                if request.url.query_pairs().any(|(name, value)| name == "page" && value != "1") {
                    json!([])
                } else {
                    discussions(self.head(iid), &self.base, self.tick())
                }
            }
            (_, Some((_, "reviewers" | "statuses" | "discussions"))) => json!([]),
            (_, Some((_, "approvals"))) => json!({ "approved_by": [] }),
            (_, Some((iid, "pipelines"))) => {
                if self.heavy {
                    self.polls.fetch_add(1, Ordering::Relaxed);
                }
                json!([self.pipeline(iid)])
            }
            (p, _)
                if p.starts_with(&format!("/api/v4/projects/{PROJECT_ID}/pipelines/"))
                    && p.ends_with("/jobs") =>
            {
                if !self.heavy {
                    jobs(5, None)
                } else if request
                    .url
                    .query_pairs()
                    .any(|(name, value)| name == "page" && value != "1")
                {
                    json!([])
                } else {
                    jobs(HEAVY_STAGES, (!still("jobs")).then(|| self.tick()))
                }
            }
            (p, _) if p.ends_with("/bridges") || p.ends_with("/statuses") => json!([]),
            (p, _) if p == format!("{project}/uploads/{UPLOAD_SECRET}/shot.png") => {
                return ResponseTemplate::new(200)
                    .set_body_raw(image_file(MERGE_REQUESTS + 1), "image/png")
                    .set_delay(delay);
            }
            (p, _) if p.ends_with("/trace") => {
                return ResponseTemplate::new(200)
                    .set_body_string(self.log.as_str())
                    .set_delay(delay);
            }
            _ => {
                return ResponseTemplate::new(404)
                    .set_body_json(json!({ "message": "404 Not Found" }));
            }
        };
        ResponseTemplate::new(200).set_body_json(body).set_delay(delay)
    }
}

impl MockGitLab {
    fn list(&self) -> Value {
        Value::Array((1..=30).rev().map(|iid| self.merge_request(iid)).collect())
    }

    fn head(&self, iid: u64) -> &str {
        let index = usize::try_from(iid).unwrap_or(0).saturating_sub(1);
        self.heads.get(index).map_or(self.base.as_str(), String::as_str)
    }

    fn merge_request(&self, iid: u64) -> Value {
        let title = if iid <= IDLE_MERGE_REQUEST { title(iid) } else { format!("Change {iid}") };
        let mut description = format!(
            "## What changes\n\nReworks the client. See `{LARGE_FILE}` and the screenshot in \
             `{MARKDOWN_FILE}`.\n\n```ts\nexport const answer: number = {iid};\n```\n"
        );
        if iid == IDLE_MERGE_REQUEST || self.heavy {
            write!(
                description,
                "\n![The upload]({}/{PROJECT}/uploads/{UPLOAD_SECRET}/shot.png)\n",
                self.base_url
            )
            .expect("writing to a string");
        }
        if self.heavy {
            description.push_str(&long_description());
        }
        json!({
            "id": 1000 + iid, "iid": iid, "project_id": PROJECT_ID, "title": title, "description": description,
            "state": "opened", "author": user(2, "grace"), "source_branch": format!("feature-{iid}"), "target_branch": "main",
            "created_at": "2026-10-01T10:00:00Z", "updated_at": self.updated_at(),
            "web_url": format!("{}/{PROJECT}/-/merge_requests/{iid}", self.base_url),
            "references": { "full": format!("{PROJECT}!{iid}") }, "labels": [], "reviewers": [], "sha": self.head(iid),
            "diff_refs": { "base_sha": self.base, "start_sha": self.base, "head_sha": self.head(iid) },
            "detailed_merge_status": "mergeable", "head_pipeline": self.pipeline(iid),
        })
    }

    fn pipeline(&self, iid: u64) -> Value {
        let id =
            if still("runs") { PIPELINE_ID } else { PIPELINE_ID + self.tick() / POLLS_PER_RUN };
        json!({
            "id": id, "project_id": PROJECT_ID, "sha": self.head(iid), "ref": format!("refs/merge-requests/{iid}/head"),
            "status": "running", "source": "merge_request_event", "created_at": "2026-10-18T09:00:00Z",
            "updated_at": self.updated_at(), "started_at": "2026-10-18T09:00:10Z",
            "web_url": format!("{}/{PROJECT}/-/pipelines/{id}", self.base_url),
        })
    }

    /// The polls so far; always 0 for a mock that is not heavy.
    fn tick(&self) -> u64 {
        self.polls.load(Ordering::Relaxed)
    }

    /// Moves on by 15 s with each poll.
    fn updated_at(&self) -> String {
        let seconds = self.tick() * 15;
        format!(
            "2026-10-18T{:02}:{:02}:{:02}Z",
            9 + seconds / 3600 % 14,
            seconds / 60 % 60,
            seconds % 60
        )
    }
}

/// Eight jobs in each of the first `stage_count` stages. With a `tick`, the jobs before the
/// tick's one (counted within its run) succeeded, it runs and the later ones wait.
fn jobs(stage_count: usize, tick: Option<u64>) -> Value {
    let stages =
        ["build", "lint", "test", "package", "deploy", "verify", "e2e", "scan", "docs", "release"];
    let mut jobs = Vec::new();
    for (stage_index, stage) in stages.iter().take(stage_count).enumerate() {
        for job in 1..=8 {
            let status = match tick {
                Some(tick) => {
                    let index = (stage_index * 8 + job - 1) as u64;
                    let running = tick % POLLS_PER_RUN * (stage_count as u64 * 8) / POLLS_PER_RUN;
                    match index.cmp(&running) {
                        std::cmp::Ordering::Less => "success",
                        std::cmp::Ordering::Equal => "running",
                        std::cmp::Ordering::Greater => "created",
                    }
                }
                None => match stage_index {
                    2 if job == 1 => "running",
                    0..=2 => "success",
                    _ => "created",
                },
            };
            jobs.push(json!({
                "id": 5000 + stage_index * 10 + job, "name": format!("{stage}-{}-{job}", stage_index + 1), "stage": stage,
                "status": status, "allow_failure": false, "started_at": "2026-10-18T09:00:20Z",
                "finished_at": "2026-10-18T09:04:00Z",
            }));
        }
    }
    Value::Array(jobs)
}

/// `OWNER_STILL=runs,notes,jobs` keeps that part of a heavy mock's answers as it was.
fn still(part: &str) -> bool {
    std::env::var("OWNER_STILL").is_ok_and(|still| still.split(',').any(|item| item == part))
}

/// Sections of a heavy merge request's description, threads on it and stages of its pipeline.
const HEAVY_SECTIONS: usize = 40;
const HEAVY_THREADS: usize = 80;
const HEAVY_STAGES: usize = 10;
/// Polls of a heavy mock's pipelines per run: a new run starts after this many.
const POLLS_PER_RUN: u64 = 8;
/// Polls of a heavy mock's pipelines per new thread.
const POLLS_PER_NOTE: u64 = 10;

/// A real merge request's description: headings, lists, a table and code in each section.
fn long_description() -> String {
    let mut text = String::new();
    for section in 0..HEAVY_SECTIONS {
        write!(
            text,
            "\n## Part {section}: invoicing\n\nThe invoice run now batches **{section}** \
             customers and writes `invoice_{section}.pdf` once the totals agree.\n\n\
             - [x] totals per customer\n- [ ] reminders for part {section}\n\n\
             | Field | Before | After |\n|---|---|---|\n| net | {section}0 | {section}5 |\n\n\
             ```ts\nexport function part{section}(total: number): number {{\n  return total * \
             {section};\n}}\n```\n"
        )
        .expect("writing to a string");
    }
    text
}

/// [`HEAVY_THREADS`] threads of three notes each, every other one on a line of a changed file,
/// and one more thread for each [`POLLS_PER_NOTE`] polls (`tick`).
fn discussions(head: &str, base: &str, tick: u64) -> Value {
    let author =
        |id: u64| json!({ "id": id, "username": format!("dev{id}"), "name": format!("Dev {id}") });
    let added = if still("notes") { 0 } else { (tick / POLLS_PER_NOTE) as usize };
    let threads: Vec<Value> = (0..HEAVY_THREADS + added)
        .map(|thread| {
            let position = (thread % 2 == 0).then(|| {
                let path = format!("src/m0/f{:02}.rs", thread % 10);
                json!({ "base_sha": base, "start_sha": base, "head_sha": head, "old_path": path,
                        "new_path": path, "position_type": "text", "old_line": null, "new_line": 5 })
            });
            let notes: Vec<Value> = (0..3)
                .map(|note| {
                    json!({
                        "id": 90_000 + thread * 10 + note,
                        "type": if position.is_some() { "DiffNote" } else { "DiscussionNote" },
                        "body": format!("Thread {thread}, note {note}: the totals of part {thread} \
                                         round half up here, see `invoice_{thread}.pdf`."),
                        "author": author(1 + (thread + note) as u64 % 4),
                        "created_at": "2026-10-20T10:00:00Z", "updated_at": "2026-10-20T10:00:00Z",
                        "system": false, "resolvable": true, "resolved": thread % 3 == 0,
                        "position": position,
                    })
                })
                .collect();
            json!({ "id": format!("d{thread:04}"), "individual_note": false, "notes": notes })
        })
        .collect();
    Value::Array(threads)
}

fn long_log() -> String {
    let mut log = String::with_capacity(LOG_LINES * 72);
    for line in 0..LOG_LINES {
        writeln!(
            log,
            "\u{1b}[32mlog line {line}\u{1b}[0m: Compiling crate_{line} v0.1.0 (/builds/app)"
        )
        .expect("writing to a string");
    }
    log
}

/// A bare repository with `main` and, for each merge request, `refs/merge-requests/{iid}/head`
/// changing [`RUST_FILES`] Rust files, [`LARGE_FILE`], [`MARKDOWN_FILE`] and [`IMAGE_FILE`].
/// Returns the base commit and the heads.
fn build_repository(git_dir: &Path) -> (String, Vec<String>) {
    git_in(None, &["init", "--bare", "--quiet", &git_dir.display().to_string()]);
    let mut stream = Vec::new();
    let mut commit = |reference: &str, mark: u64, parent: Option<u64>, version: u64| {
        write!(
            Text(&mut stream),
            "commit {reference}\nmark :{mark}\ncommitter Test <test@example.com> {} +0000\ndata 6\nchange\n",
            1_700_000_000 + mark
        )
        .expect("writing to a buffer");
        if let Some(parent) = parent {
            writeln!(Text(&mut stream), "from :{parent}").expect("writing to a buffer");
        }
        let mut files: Vec<(String, Vec<u8>)> = (0..RUST_FILES)
            .map(|index| (format!("src/m{}/f{index:02}.rs", index / 10), rust_file(index, version)))
            .collect();
        files.push((LARGE_FILE.to_owned(), large_file(version)));
        files.push((MARKDOWN_FILE.to_owned(), markdown_file(version)));
        files.push((IMAGE_FILE.to_owned(), image_file(version)));
        for (path, content) in files {
            write!(Text(&mut stream), "M 100644 inline {path}\ndata {}\n", content.len())
                .expect("writing to a buffer");
            stream.extend_from_slice(&content);
            stream.push(b'\n');
        }
    };
    commit("refs/heads/main", 1, None, 0);
    for iid in 1..=MERGE_REQUESTS {
        commit(&format!("refs/merge-requests/{iid}/head"), 1 + iid, Some(1), iid);
    }
    let mut child = Command::new("git")
        .args(["--git-dir", &git_dir.display().to_string(), "fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("git fast-import");
    std::io::Write::write_all(child.stdin.as_mut().expect("stdin"), &stream)
        .expect("feed fast-import");
    drop(child.stdin.take());
    let status = child.wait().expect("fast-import exit");
    assert!(status.success(), "fast-import failed");
    for (key, value) in
        [("uploadpack.allowFilter", "true"), ("uploadpack.allowAnySHA1InWant", "true")]
    {
        git_in(Some(git_dir), &["config", key, value]);
    }
    let base = git_in(Some(git_dir), &["rev-parse", "refs/heads/main"]);
    let heads = (1..=MERGE_REQUESTS)
        .map(|iid| {
            git_in(Some(git_dir), &["rev-parse", &format!("refs/merge-requests/{iid}/head")])
        })
        .collect();
    (base, heads)
}

/// Version 0 is the base; each merge request changes a few lines of its own.
fn rust_file(index: usize, version: u64) -> Vec<u8> {
    let mut lines: Vec<String> =
        (0..60).map(|line| format!("pub fn item_{index}_{line}() -> u32 {{ {line} }}")).collect();
    if version > 0 {
        lines[4] = format!("pub fn item_{index}_4() -> u32 {{ {version}00 }} // changed");
        lines[40] = format!("pub fn item_{index}_40() -> u32 {{ {version}000 }} // changed");
        lines.push(format!("pub fn added_{index}_{version}() -> u32 {{ {version} }}"));
    }
    lines.join("\n").into_bytes()
}

/// Every fourth line changes in a merge request: a long diff of many hunks.
fn large_file(version: u64) -> Vec<u8> {
    let mut text = String::with_capacity(LARGE_LINES * 72);
    for line in 0..LARGE_LINES {
        let value = if version > 0 && line % 4 == 0 { line as u64 * 10 + version } else { 0 };
        writeln!(
            text,
            "export function item{line}(input: number): string {{ return `${{input + {value}}}`; }}"
        )
        .expect("writing to a string");
    }
    text.into_bytes()
}

fn markdown_file(version: u64) -> Vec<u8> {
    let mut text = format!("# Client, version {version}\n\n![The screen](shot.png)\n\n");
    for section in 0..40 {
        write!(
            text,
            "## Part {section}\n\nThe client sends **{section}** requests per page and keeps \
             `item{section}` cached for version {version}.\n\n```ts\nconst part{section} = \
             item{section}({version});\n```\n\n"
        )
        .expect("writing to a string");
    }
    text.into_bytes()
}

/// A 1200 × 800 screenshot whose colors depend on the version, so each merge request's differs.
fn image_file(version: u64) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(1200, 800, |x, y| {
        image::Rgba([(x % 256) as u8, (y % 256) as u8, (version * 40 % 256) as u8, 255])
    });
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).expect("encode the screenshot");
    png.into_inner()
}

/// Code in each of the eight languages of a .NET and web pull request, as fences of its
/// description and its files show them: a block of the usual shapes, repeated to `FENCE_BYTES`.
#[cfg(any(windows, feature = "measurements"))]
const EIGHT_LANGUAGES: [(&str, &str); 8] = [
    (
        "ts",
        "import { Injectable } from \"@angular/core\";\nexport class Store<T extends object> {\n    private readonly items = new Map<string, T>();\n    async open(uri: string, value?: T): Promise<void> { const text = `uri ${uri}`; if (!value) { throw new Error(text); } }\n}\n",
    ),
    (
        "csharp",
        "using System.Collections.Generic;\nnamespace App;\npublic sealed record Line(string Name, decimal Amount);\npublic class Totals { public int Count { get; init; } = 0; public string Describe(object? value) => value switch { int n and > 0 => $\"{n}\", _ => \"other\" }; }\n",
    ),
    (
        "json",
        "{\"name\": \"app\", \"version\": \"1.0.0\", \"items\": [true, false, null, 1.5], \"nested\": {\"key\": \"value\"}}\n",
    ),
    (
        "yaml",
        "jobs:\n  build:\n    runs-on: windows-latest\n    steps:\n      - uses: actions/checkout@v4\n      - run: cargo test --workspace # all of it\n",
    ),
    (
        "markdown",
        "## Part\n\nThe client sends **many** requests and keeps `item` cached, see [the docs](https://x.test).\n\n- one\n- two\n",
    ),
    (
        "rust",
        "use std::collections::HashMap;\n#[derive(Debug)]\npub struct Name<'a> { field: &'a str }\npub fn function(argument: &str) -> Result<Vec<u8>, Error> { let mut x = vec![1]; x.push(2); Ok(x) }\n",
    ),
    (
        "xml",
        "<?xml version=\"1.0\"?>\n<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup><TargetFramework>net9.0</TargetFramework></PropertyGroup>\n  <!-- a comment -->\n</Project>\n",
    ),
    (
        "powershell",
        "param([string]$Name = \"App\")\n$items = Get-ChildItem -Path $PSScriptRoot -Filter *.ps1\nforeach ($item in $items) { Write-Host \"Found $($item.Name)\" }\n",
    ),
];
/// Bytes of each block: within what a fence colors at all (256 KiB), and enough lines that it
/// reads in parts on several reader threads.
#[cfg(feature = "measurements")]
const FENCE_BYTES: usize = 200 * 1024;
/// Blocks per language, as in a description and its threads' comments.
#[cfg(feature = "measurements")]
const FENCES: usize = 2;
/// The heap the eight languages' grammars may take while they color: our bound. Measured 107 MB
/// (226 MB with fancy-regex 0.16 and uncapped lazy DFA caches): 85 MB compiled on the warm-up
/// thread and 7.8 MB for each reader thread that searched with them. Which reader threads take a
/// block's parts varies, so the bound holds all eight.
#[cfg(feature = "measurements")]
const EIGHT_LANGUAGES_PEAK: usize = 150 * MB;
/// What stays once the grammars went: the fences' colors, within their cache's 4 MB, and room.
#[cfg(feature = "measurements")]
const KEPT_COLORS: usize = 8 * MB;

/// A pull request of the eight languages: each is warmed up as its file list asks for, then a long
/// block of it colors as a fence of the description does (on the coloring thread, in parts on the
/// reader threads). The live heap's peak stays within [`EIGHT_LANGUAGES_PEAK`], and once the
/// grammars went unused only the colors are left.
#[test]
#[cfg(feature = "measurements")]
fn memory_of_eight_languages_grammars_stays_small() {
    if run_in_child("app::memory_walkthrough::memory_of_eight_languages_grammars_stays_small") {
        return;
    }
    let code: Vec<(&str, String)> = EIGHT_LANGUAGES
        .iter()
        .flat_map(|(language, block)| {
            (0..FENCES).map(|fence| (*language, block.repeat(FENCE_BYTES / block.len() - fence)))
        })
        .collect();
    // The grammar set itself loads once for the process's life.
    grammar::for_language("rust").expect("a Rust grammar");
    let start = live_bytes();
    let peak = Arc::new(AtomicUsize::new(start));
    let is_done = Arc::new(AtomicBool::new(false));
    let sampler = {
        let (peak, is_done) = (peak.clone(), is_done.clone());
        std::thread::spawn(move || {
            while !is_done.load(Ordering::Relaxed) {
                peak.fetch_max(live_bytes(), Ordering::Relaxed);
                cv_testkit::let_threads_run(Duration::from_millis(1));
            }
        })
    };
    let warm = std::thread::spawn(|| {
        for (language, _) in EIGHT_LANGUAGES {
            highlight::warm_up(grammar::for_language(language).expect("a grammar"));
        }
    });
    warm.join().expect("the warm-up ran");
    let ctx = egui::Context::default();
    for (language, text) in &code {
        let started = Instant::now();
        code_colors::colors(&ctx, language, text);
        while code_colors::computed_count(language, text) == 0 {
            assert!(started.elapsed() < Duration::from_secs(120), "{language} never colored");
            cv_testkit::let_threads_run(Duration::from_millis(5));
        }
    }
    let colored = live_bytes();
    is_done.store(true, Ordering::Relaxed);
    sampler.join().expect("the sampler ran");
    grammar::release_unused();
    let released = live_bytes();
    let peak = peak.load(Ordering::Relaxed);
    println!(
        "eight languages: start {} MB, peak {} MB, colored {} MB, grammars dropped {} MB",
        megabytes(start),
        megabytes(peak),
        megabytes(colored),
        megabytes(released)
    );
    assert!(
        peak - start <= EIGHT_LANGUAGES_PEAK,
        "the eight languages took {} MB, over our bound of {} MB",
        megabytes(peak - start),
        megabytes(EIGHT_LANGUAGES_PEAK)
    );
    let kept = released - start;
    assert!(kept <= KEPT_COLORS, "only the colors stay: {} MB", megabytes(kept));
}

/// Frames the unfocused Overview runs while its skeletons are checked.
const UNFOCUSED_FRAMES: usize = 60;

/// A window in the background holds its skeletons still: the merge policy loads only while the
/// window is active, so the Overview's merge type stays a placeholder there, and its sweep asked
/// for a frame every millisecond, the full frame rate for as long as the window sat behind others.
#[test]
fn an_unfocused_overview_holds_its_skeletons_still() {
    let fixture = Fixture::new("unfocused");
    let mut harness = fixture.harness(&GpuTextures::default());
    wait_until_unsettled(&mut harness, "the merge request list", |harness| {
        has(harness, "Change 30")
    });
    walkthrough::set_focus(&mut harness, false);
    harness.state_mut().navigator.go(Route::PullRequest(fixture.key(IDLE_MERGE_REQUEST)));
    wait_until_unsettled(&mut harness, "the overview", |harness| {
        has(harness, &title(IDLE_MERGE_REQUEST))
    });
    let mut causes = std::collections::BTreeSet::new();
    let mut time = harness.ctx.input(|input| input.time);
    for _ in 0..UNFOCUSED_FRAMES {
        time += 0.1;
        harness.input_mut().time = Some(time);
        harness.step();
        causes.extend(harness.ctx.repaint_causes().iter().map(|cause| cause.file.to_owned()));
        cv_testkit::let_threads_run(Duration::from_millis(3));
    }
    let sweeps: Vec<_> = causes
        .iter()
        .filter(|file| file.replace('\\', "/").ends_with("cv-widgets/src/skeleton.rs"))
        .collect();
    assert!(sweeps.is_empty(), "no skeleton sweeps in the background: {causes:?}");
}

/// Compile-and-free cycles the grammar churn probe runs.
#[cfg(windows)]
const CHURN_CYCLES: usize = 60;
/// Lines of the text each language reads on one reader thread.
#[cfg(windows)]
const ONE_PART_LINES: usize = 400;
/// Lines of the text each language reads in four parts at once (on four reader threads).
#[cfg(windows)]
const FOUR_PART_LINES: usize = 4_400;
/// Diff texts the interleaved probe keeps alive at once, the newest ones.
#[cfg(windows)]
const KEPT_DIFFS: usize = 4;
/// Lines of each diff text: about 3 MB, a large pull request's file.
#[cfg(windows)]
const DIFF_LINES: usize = 40_000;
/// How far the gap between private bytes and live heap may rise over the first cycles' highest:
/// our bound, a little over one cycle's noise (2 MB measured).
#[cfg(windows)]
const CHURN_GROWTH: usize = 16 * MB;

/// Grammar churn on Windows: each cycle compiles the eight languages' grammars, reads a text of
/// each on one reader thread and one in four parts, then drops the compiled sets as the sweeper
/// does once they went unused, and reads the live heap and the private bytes. Private bytes minus
/// live heap is what the system heap keeps of freed blocks; it must level off, not climb with each
/// cycle (see [`CHURN_GROWTH`]). Measured 2026-10-28 in release over 60 cycles: the gap flips
/// between about 27 and 105 MB from cycle 7 on (the heap keeps one cycle's peak or gives it back)
/// and its highest value per ten cycles stays 104.8 to 106.0 MB; with diffs between, 120.4 to
/// 122.2 MB. Ignored because a run takes about 100 s in release and minutes in debug; run it by
/// hand: `cargo test --release -p review-app --bin CodeView grammar_churn -- --ignored
/// --nocapture`.
#[test]
#[ignore = "a probe run by hand in release, see the doc comment"]
#[cfg(windows)]
fn grammar_churn_probe() {
    if run_in_child("app::memory_walkthrough::grammar_churn_probe") {
        return;
    }
    churn(false);
}

/// [`grammar_churn_probe`] with large diff texts allocated between the languages, the newest
/// [`KEPT_DIFFS`] kept alive, as the app holds diffs while grammars come and go.
#[test]
#[ignore = "a probe run by hand in release, see grammar_churn_probe"]
#[cfg(windows)]
fn grammar_churn_probe_between_diffs() {
    if run_in_child("app::memory_walkthrough::grammar_churn_probe_between_diffs") {
        return;
    }
    churn(true);
}

/// Runs [`CHURN_CYCLES`] cycles, prints live, private and their gap per cycle, and checks that the
/// gap's highest value over the last third stays within [`CHURN_GROWTH`] of the first third's.
#[cfg(windows)]
fn churn(with_diffs: bool) {
    use cv_base::platform::heap::private_bytes;
    use cv_syntax::blob_text::BlobText;

    let text = |block: &str, count: usize| -> BlobText {
        let lines: Vec<&str> = block.lines().collect();
        let joined: String =
            lines.iter().cycle().take(count).flat_map(|line| [*line, "\n"]).collect();
        BlobText::from_bytes(joined.into_bytes())
    };
    let texts: Vec<(&str, BlobText, BlobText)> = EIGHT_LANGUAGES
        .iter()
        .map(|(language, block)| {
            (*language, text(block, ONE_PART_LINES), text(block, FOUR_PART_LINES))
        })
        .collect();
    grammar::for_language("rust").expect("a Rust grammar");
    let mut diffs = std::collections::VecDeque::new();
    let mut gaps = Vec::new();
    println!("cycle  live MB  private MB  private-live MB");
    for cycle in 0..=CHURN_CYCLES {
        for (language, one_part, four_parts) in &texts {
            if with_diffs {
                diffs.push_back(diff_text(cycle));
                if diffs.len() > KEPT_DIFFS {
                    diffs.pop_front();
                }
            }
            let grammar = grammar::for_language(language).expect("a grammar");
            let working = highlight::warm_up(grammar);
            for text in [one_part, four_parts] {
                let mut lines = 0;
                highlight::highlight(text, grammar.syntax, &working, |chunk| {
                    lines += chunk.line_count();
                    true
                });
                assert_eq!(lines, text.line_count(), "{language} read every line");
            }
        }
        grammar::release_unused();
        cv_testkit::let_threads_run(Duration::from_millis(50));
        let (live, private) =
            (live_bytes(), private_bytes().expect("the process's memory counters read"));
        println!(
            "{cycle:>5}  {:>7}  {:>10}  {:>15}",
            megabytes(live),
            megabytes(private),
            megabytes(private.saturating_sub(live))
        );
        gaps.push(private.saturating_sub(live));
    }
    std::hint::black_box(&diffs);
    let third = gaps.len() / 3;
    let early = gaps[1..=third].iter().max().copied().unwrap_or(0);
    let late = gaps[gaps.len() - third..].iter().max().copied().unwrap_or(0);
    assert!(
        late <= early + CHURN_GROWTH,
        "the heap keeps more with each cycle: {} MB, then {} MB",
        megabytes(early),
        megabytes(late)
    );
}

/// A diff's lines as the app holds them, one string each, of varied lengths.
#[cfg(windows)]
fn diff_text(seed: usize) -> Vec<String> {
    (0..DIFF_LINES)
        .map(|line| {
            let width = 20 + (line * 7 + seed * 13) % 110;
            format!("+{:width$}", line * seed, width = width)
        })
        .collect()
}
