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
//! The probes stay `#[ignore]`, so that command never runs them. One test runs in the ordinary
//! `cargo test`: `a_closed_app_frees_its_context`, the leak check of a closed app.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
#[cfg(any(windows, feature = "measurements"))]
use std::process::Command;
use std::sync::atomic::Ordering;
#[cfg(feature = "measurements")]
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, LazyLock, Mutex};
#[cfg(any(windows, feature = "measurements"))]
use std::time::Duration;
#[cfg(feature = "measurements")]
use std::time::Instant;

#[cfg(feature = "measurements")]
use egui_kittest::kittest::{NodeT as _, Queryable};
use egui_kittest::{Harness, TestRenderer};
#[cfg(feature = "measurements")]
use review_agent::{AgentEvent, Usage};
use review_agent::{AgentKind, ModelInfo};
use review_core::{Connection, ProviderKind};
use serde_json::{Value, json};
use wiremock::{MockServer, Request, Respond, ResponseTemplate};

use super::ReviewApp;
#[cfg(feature = "measurements")]
use super::assistant_script;
use super::gitlab_mock::{PROJECT_ID, merge_request, merge_request_route};
#[cfg(feature = "measurements")]
use super::request_budget::scroll_list;
use super::request_budget::{diff_stats_summaries, harness_builder, user};
use super::walkthrough::{self, has, saved_paths, wait_idle, wait_until_unsettled};
#[cfg(feature = "measurements")]
use super::walkthrough::{click_first, pump};
#[cfg(any(windows, feature = "measurements"))]
use cv_base::platform::heap::live_bytes;
use cv_runtime::route::{PullRequestKey, Route};
use cv_services::agents::FakeCli;
#[cfg(feature = "measurements")]
use cv_syntax::code_colors;
#[cfg(any(windows, feature = "measurements"))]
use cv_syntax::{grammar, highlight};
#[cfg(feature = "measurements")]
use cv_testkit::Clicks;
use cv_testkit::{ScratchDir, fast_import, git_in, samples};

const PROJECT: &str = "group/app";
const PIPELINE_ID: u64 = 1001;
/// The merge requests the session reviews, one after the other.
const MERGE_REQUESTS: u64 = 5;
/// Small Rust files each merge request changes, next to the large and the Markdown ones.
const RUST_FILES: usize = 30;
/// Lines of the large TypeScript file: about 1 MB, a generated client or a bundled module.
const LARGE_LINES: usize = 15_000;
/// Lines of the long job log: about 4 MB.
const LOG_LINES: usize = 60_000;
#[cfg(any(windows, feature = "measurements"))]
const CHILD_MARKER: &str = "CODEVIEW_MEMORY_TEST_CHILD";
#[cfg(any(windows, feature = "measurements"))]
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
#[cfg(feature = "measurements")]
const IDLE_STEP: f64 = 0.5;
/// What the idle Overview may gain over all rounds: our number, the 1 MB the measurement settles
/// within. The rounds run 25 simulated minutes, 100 polls of the running pipeline at 15 s, so a
/// poll that kept 10 KB fails it.
#[cfg(feature = "measurements")]
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
    cv_testkit::poll_until("the closed app's context to go", || is_dropped.load(Ordering::SeqCst));
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
    assistant_script::ask(harness, "Explain it.");
    let (_, agent) = assistant_script::wait_for_session(harness);
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
    let released = || {
        grammar::release_unused();
        grammar::kept_languages().is_empty().then_some(())
    };
    walkthrough::wait_for(harness, "the colorings to end", |_| released(), |_| {});
}

/// The textures the app has on the GPU, by the uploads and frees egui hands the renderer. The
/// harness's own renderer would keep every upload on the heap until a render that never comes.
#[derive(Clone, Default)]
struct GpuTextures(Arc<Mutex<HashMap<egui::TextureId, usize>>>);

impl GpuTextures {
    #[cfg(feature = "measurements")]
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
#[cfg(feature = "measurements")]
struct Curve {
    textures: GpuTextures,
    points: Vec<Point>,
}

#[cfg(feature = "measurements")]
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

#[cfg(feature = "measurements")]
impl Curve {
    fn heap_at(&self, step: &str) -> usize {
        self.points
            .iter()
            .find(|point| point.step == step)
            .map_or_else(|| panic!("no measured step {step:?}"), |point| point.heap)
    }

    fn peak_heap(&self) -> usize {
        self.points.iter().map(|point| point.heap).max().unwrap_or(0)
    }

    fn peak_textures(&self) -> usize {
        self.points.iter().map(|point| point.textures).max().unwrap_or(0)
    }

    /// Whether every point after `step` holds the textures and the font atlas `step` held.
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

#[cfg(feature = "measurements")]
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

#[cfg(feature = "measurements")]
impl std::fmt::Display for Curve {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "\n=== Memory by step ===")?;
        for point in &self.points {
            writeln!(formatter, "{point}")?;
        }
        Ok(())
    }
}

#[cfg(any(windows, feature = "measurements"))]
fn megabytes(bytes: usize) -> String {
    format!("{:.1}", bytes as f64 / MB as f64)
}

/// The live bytes once they stopped moving: frames run and background threads work until ten
/// samples 100 ms apart stay within 1 MB of each other (within [`walkthrough::LIMIT`]).
#[cfg(feature = "measurements")]
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
        assert!(started.elapsed() < walkthrough::LIMIT, "the heap never settled");
    }
}

/// Runs the test `name` again in a child process, alone, and returns true here (the parent), which
/// then has nothing left to do; returns false in that child, which runs the test's body.
#[cfg(any(windows, feature = "measurements"))]
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

/// The mock hosts' runtime, for the process's life.
static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("mock runtime")
});

struct Fixture {
    server: MockServer,
    /// What [`Self::forget_requests`] mounts again.
    #[cfg(feature = "measurements")]
    gitlab: MockGitLab,
    connection_id: String,
    /// The repository and the app's data folders, removed with the fixture.
    scratch: ScratchDir,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let scratch = ScratchDir::new(&format!("memory-{name}"));
        let repository = scratch.path().join("remote.git");
        let (base, heads) = build_repository(&repository);
        // Not recording: the requests would grow the heap this test measures.
        let server = RUNTIME.block_on(MockServer::builder().disable_request_recording().start());
        cv_testkit::fresh_origin(&server.uri());
        let gitlab = MockGitLab {
            base_url: server.uri(),
            clone_url: cv_testkit::file_url(&repository),
            base,
            heads: Arc::new(heads),
            log: Arc::new(long_log()),
        };
        let fixture = Self {
            server,
            #[cfg(feature = "measurements")]
            gitlab: gitlab.clone(),
            connection_id: format!("memory-{name}"),
            scratch,
        };
        fixture.mount(gitlab);
        fixture
    }

    /// Answers every request through `gitlab`.
    fn mount(&self, gitlab: MockGitLab) {
        let mock = wiremock::Mock::given(wiremock::matchers::any()).respond_with(gitlab);
        RUNTIME.block_on(mock.mount(&self.server));
    }

    /// Mounts the mock afresh: wiremock keeps every request its mock matched (with the body that
    /// pins the connection's read buffer, about 8 KB a request), which a long idle would count as
    /// the app's.
    #[cfg(feature = "measurements")]
    fn forget_requests(&self) {
        RUNTIME.block_on(self.server.reset());
        self.mount(self.gitlab.clone());
    }

    /// The app on the textures' renderer with the mock's connection, once its start settled, with
    /// the first file opening and a signed-in coding agent.
    fn harness(&self, textures: &GpuTextures) -> Harness<'static, ReviewApp> {
        let connection = Connection {
            name: "Mock GitLab".to_owned(),
            organizations: vec!["group".to_owned()],
            token: Some("glpat-mock-only".to_owned()),
            ..samples::connection(&self.connection_id, ProviderKind::GitLab, &self.server.uri())
        };
        let paths = saved_paths(&self.scratch, &[connection]);
        let builder = harness_builder().renderer(textures.clone());
        let mut harness = walkthrough::app_with(builder, paths, cv_providers::create_provider);
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
}

impl Respond for MockGitLab {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path().to_owned();
        let project = format!("/api/v4/projects/{}", urlencoding::encode(PROJECT));
        let body = match (path.as_str(), merge_request_route(&path, PROJECT)) {
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
            (_, Some((_, "reviewers" | "statuses" | "discussions"))) => json!([]),
            (_, Some((_, "approvals"))) => json!({ "approved_by": [] }),
            (_, Some((iid, "pipelines"))) => json!([self.pipeline(iid)]),
            (p, _)
                if p.starts_with(&format!("/api/v4/projects/{PROJECT_ID}/pipelines/"))
                    && p.ends_with("/jobs") =>
            {
                jobs(5)
            }
            (p, _) if p.ends_with("/bridges") || p.ends_with("/statuses") => json!([]),
            (p, _) if p == format!("{project}/uploads/{UPLOAD_SECRET}/shot.png") => {
                return ResponseTemplate::new(200)
                    .set_body_raw(image_file(MERGE_REQUESTS + 1), "image/png");
            }
            (p, _) if p.ends_with("/trace") => {
                return ResponseTemplate::new(200).set_body_string(self.log.as_str());
            }
            _ => {
                return ResponseTemplate::new(404)
                    .set_body_json(json!({ "message": "404 Not Found" }));
            }
        };
        ResponseTemplate::new(200).set_body_json(body)
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
        let mut description = format!(
            "## What changes\n\nReworks the client. See `{LARGE_FILE}` and the screenshot in \
             `{MARKDOWN_FILE}`.\n\n```ts\nexport const answer: number = {iid};\n```\n"
        );
        if iid == IDLE_MERGE_REQUEST {
            write!(
                description,
                "\n![The upload]({}/{PROJECT}/uploads/{UPLOAD_SECRET}/shot.png)\n",
                self.base_url
            )
            .expect("writing to a string");
        }
        let mut value = merge_request(&self.base_url, PROJECT, iid, self.head(iid), &self.base);
        if iid <= IDLE_MERGE_REQUEST {
            value["title"] = json!(title(iid));
        }
        value["description"] = json!(description);
        value["head_pipeline"] = self.pipeline(iid);
        value
    }

    fn pipeline(&self, iid: u64) -> Value {
        json!({
            "id": PIPELINE_ID, "project_id": PROJECT_ID, "sha": self.head(iid), "ref": format!("refs/merge-requests/{iid}/head"),
            "status": "running", "source": "merge_request_event", "created_at": "2026-10-18T09:00:00Z",
            "updated_at": "2026-10-18T09:00:00Z", "started_at": "2026-10-18T09:00:10Z",
            "web_url": format!("{}/{PROJECT}/-/pipelines/{PIPELINE_ID}", self.base_url),
        })
    }
}

/// Eight jobs in each of the first `stage_count` stages: those of the first two stages succeeded,
/// the third's first job runs, the rest wait.
fn jobs(stage_count: usize) -> Value {
    let stages =
        ["build", "lint", "test", "package", "deploy", "verify", "e2e", "scan", "docs", "release"];
    let mut jobs = Vec::new();
    for (stage_index, stage) in stages.iter().take(stage_count).enumerate() {
        for job in 1..=8 {
            let status = match stage_index {
                2 if job == 1 => "running",
                0..=2 => "success",
                _ => "created",
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
            stream,
            "commit {reference}\nmark :{mark}\ncommitter Test <test@example.com> {} +0000\ndata 6\nchange\n",
            1_700_000_000 + mark
        )
        .expect("writing to a buffer");
        if let Some(parent) = parent {
            writeln!(stream, "from :{parent}").expect("writing to a buffer");
        }
        let mut files: Vec<(String, Vec<u8>)> = (0..RUST_FILES)
            .map(|index| (format!("src/m{}/f{index:02}.rs", index / 10), rust_file(index, version)))
            .collect();
        files.push((LARGE_FILE.to_owned(), large_file(version)));
        files.push((MARKDOWN_FILE.to_owned(), markdown_file(version)));
        files.push((IMAGE_FILE.to_owned(), image_file(version)));
        for (path, content) in files {
            write!(stream, "M 100644 inline {path}\ndata {}\n", content.len())
                .expect("writing to a buffer");
            stream.extend_from_slice(&content);
            stream.push(b'\n');
        }
    };
    commit("refs/heads/main", 1, None, 0);
    for iid in 1..=MERGE_REQUESTS {
        commit(&format!("refs/merge-requests/{iid}/head"), 1 + iid, Some(1), iid);
    }
    fast_import(git_dir, &stream);
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
