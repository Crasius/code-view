//! An update of the pull request that leaves the open file alone leaves the Files tab alone: the
//! same file stays open, its diff keeps its rows and its scroll position (down and sideways), the
//! file tree keeps its scroll, and no frame in between shows a loading state. Checked for every way
//! an update comes in: the live probe, the refresh after the user's own write, and F5; for changes
//! of the pull request's metadata, a comment elsewhere, a target branch that moved on, and a push
//! that does not touch the open file, both for a code diff and a rendered Markdown file. The header
//! keeps its height and its slid-away state, so nothing under it moves on screen either.
//!
//! Offline: a scripted provider serves the pull request from a local git repository the mirror
//! fetches from. The clock is the harness's simulated frames; between two frames the answers get
//! the real time they need, so a frame stands for any round trip.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cv_testkit::Clicks;
use egui::{Modifiers, Pos2, Rect, Role, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable};
use review_core::{
    Capabilities, Connection, DiffSide, GitRemote, Label, MergeOptions, MergeStatus, MergeStrategy,
    NewThread, PipelineJob, PipelineRun, Provider, ProviderError, ProviderKind, PullRequestDetail,
    PullRequestQuery, PullRequestState, PullRequestSummary, RepositoryInfo, Result, ReviewThread,
    ReviewVote, Reviewer, RunningLogs, ThreadComment, ThreadStatus, UserIdentity,
};
use review_store::{AppPaths, ConnectionStore, DiffLayout, FilesStart, PullRequestStart};
use time::macros::datetime;
use tokio::io::AsyncWrite;

use crate::PullRequestPage;
use cv_base::theme;
use cv_pr_session::actions::Refresh;
use cv_pr_threads::samples;
use cv_runtime::route::{Navigator, PullRequestKey};
use cv_services::Services;
use cv_services::writes::Effect;
use cv_shell::focus_mode::{self, Surface};
use cv_testkit::{self as test_support, ScratchDir};
use cv_widgets::surfaces;

const REPOSITORY: &str = "acme/widgets";
const NUMBER: u64 = 31;
/// The open file: long enough to scroll down, its lines wider than the pane.
const BIG: &str = "app/big.rs";
/// A long Markdown file, shown rendered.
const GUIDE: &str = "docs/GUIDE.md";
/// Files beside it, enough to scroll the file tree.
const SMALL_FILES: usize = 48;
const FRAME: f32 = 0.25;
/// Simulated time an update may take to come in: the live probe's interval and its read-ahead.
const UPDATE_SECONDS: f64 = 60.0;
/// Real time the answers in flight may take: git's pace, which the whole suite and whatever else
/// runs on the machine share (see `action_e2e`'s `LIMIT`).
const REAL_LIMIT: Duration = Duration::from_secs(180);

static HOSTS: Mutex<Option<HashMap<String, Arc<Mutex<Host>>>>> = Mutex::new(None);

/// What the scripted platform answers.
struct Host {
    detail: PullRequestDetail,
    threads: Vec<ReviewThread>,
    clone_url: String,
}

/// How an update reaches the page.
#[derive(Debug, Clone, Copy)]
enum Way {
    /// The live probe sees another revision and applies it ([`crate::live`]).
    Live,
    /// The user's own write finished and reloads the pull request (`Refresh::PullRequest`).
    AfterWrite,
    /// The user pressed F5.
    F5,
}

const WAYS: [Way; 3] = [Way::Live, Way::AfterWrite, Way::F5];

/// A change on the host, named, made with the step's number.
type Change = (&'static str, fn(&mut App, usize));

/// Draft to active and back, title and description, a reviewer and a label, a new comment on
/// another file, and the target branch moving on (Azure DevOps reports a new merge target when it
/// recomputes the merge, e.g. after a draft is published): none of them touches the open file.
#[test]
fn metadata_updates_keep_the_open_files_place() {
    let mut app = App::start("keep-place-metadata");
    let place = app.scroll_everything();
    let changes: [Change; 6] = [
        ("draft and active", |app, _| app.host().detail.summary.is_draft ^= true),
        // Azure DevOps recomputes the merge when a draft is published: a new merge target, the
        // merge state on its way.
        ("published with the merge recomputed", |app, step| {
            let target = app.origin.commit_on_main(step);
            let mut host = app.host();
            host.detail.summary.is_draft ^= true;
            host.detail.target_sha = target;
            host.detail.merge_status = if host.detail.merge_status == MergeStatus::Mergeable {
                MergeStatus::Unknown
            } else {
                MergeStatus::Mergeable
            };
        }),
        ("title and description", |app, step| {
            let mut host = app.host();
            host.detail.summary.title = format!("Speed up the widget cache, take {step}");
            host.detail.description = format!("Caches each widget once. Revised {step} times.");
        }),
        ("reviewer and label", |app, step| {
            let mut host = app.host();
            let reviewer =
                samples::person(&format!("reviewer-{step}"), &format!("Reviewer {step}"));
            host.detail.summary.reviewers.push(Reviewer {
                identity: reviewer,
                vote: ReviewVote::None,
                is_required: false,
                is_group: false,
            });
            host.detail.summary.labels.push(Label {
                name: format!("label-{step}"),
                color: None,
                description: None,
            });
        }),
        ("a comment on another file", |app, step| {
            let path = small_file(step % SMALL_FILES);
            let comment = samples::comment(&format!("c-{step}"), &samples::mara(), 1, "Typo here.");
            let thread = samples::thread(
                &format!("t-{step}"),
                ThreadStatus::Active,
                Some(samples::lines(&path, 1, 1)),
                vec![comment],
            );
            app.host().threads.push(thread);
        }),
        ("the target branch moving on", |app, step| {
            let target = app.origin.commit_on_main(step);
            app.host().detail.target_sha = target;
        }),
    ];
    let mut step = 0;
    for (what, change) in changes {
        for way in WAYS {
            step += 1;
            let what = format!("{what} through {way:?}");
            app.update(
                &what,
                way,
                |app| change(app, step),
                |app, what| app.assert_steady(&place, what),
            );
        }
    }
}

/// A push that changes another file moves the head: the open file's content stays the same, so
/// its diff stays as it is, rows, scroll and all.
#[test]
fn a_push_that_leaves_the_open_file_alone_keeps_its_place() {
    let mut app = App::start("keep-place-push");
    let place = app.scroll_everything();
    for (step, way) in WAYS.into_iter().enumerate() {
        let what = format!("a push to another file through {way:?}");
        app.update(&what, way, |app| app.push(step), |app, what| app.assert_steady(&place, what));
        let files = app.page().session.as_ref().ok().and_then(|session| session.files().cloned());
        let other = small_file(step);
        let pushed = files.iter().flat_map(|files| files.iter()).find(|file| file.path == other);
        assert!(
            pushed.is_some_and(|file| file.additions == Some(2)),
            "the page shows the pushed version of {other}"
        );
    }
}

/// The rendered view of a Markdown file keeps its place through a push to another file as well:
/// its versions stay the same, only the commits its images resolve in moved on.
#[test]
fn a_push_leaves_a_rendered_markdown_file_in_place() {
    let mut app = App::start("keep-place-markdown");
    let pane = app.harness.get_by_label_contains(&format!("Diff of {BIG}")).rect();
    app.harness.click_at(app.harness.get_by_role_and_label(Role::TreeItem, GUIDE).rect());
    let started = Instant::now();
    while app.rendered().1.is_none() || !app.is_settled() {
        assert!(started.elapsed() < REAL_LIMIT, "{GUIDE} did not render");
        app.frame();
        test_support::let_threads_run(Duration::from_millis(10));
    }
    app.wheel(pane.center(), vec2(0.0, -1500.0));
    for _ in 0..20 {
        app.frame();
    }
    let place = app.rendered();
    assert_eq!(place.0.as_deref(), Some(GUIDE));
    assert!(place.2.is_some_and(|(_, line)| line > 20), "the rendered file scrolled: {place:?}");
    for (step, way) in WAYS.into_iter().enumerate() {
        let what = format!("a push to another file through {way:?}");
        app.update(
            &what,
            way,
            |app| app.push(step),
            |app, what| {
                assert_eq!(app.rendered(), place, "{what} moved the rendered {GUIDE}");
            },
        );
    }
}

/// Where the user is in the Files tab.
#[derive(Debug, Clone, PartialEq)]
struct Place {
    selected: Option<String>,
    /// The code row at the top of the diff and how far it is scrolled past.
    top_line: Option<(usize, f32)>,
    /// The diff's scroll offset, each pane's sideways offset and the build of its rows.
    view: (f32, [f32; 2], u64),
    /// The diff on screen, by its address: the same one means nothing was loaded or rebuilt.
    diff: usize,
    /// Where the diff is on screen: a header that changed its height would move it.
    diff_rect: Rect,
    /// How much of the header is slid away, as drawn.
    header_hidden: f32,
    /// The file tree's first row in view and where it is on screen.
    tree: Option<(String, Rect)>,
}

/// Where the user is in a rendered Markdown file: the selected file, the rendered diff by its key
/// (`None` while the placeholder shows), the source line at the top of its view, how much of the
/// header is slid away and the file tree's first row on screen.
type RenderedPlace =
    (Option<String>, Option<u64>, Option<(DiffSide, u32)>, f32, Option<(String, Rect)>);

/// The page with what it needs, as the app hands them to it. The services drop last: the page's
/// last writes run on their runtime.
struct Scene {
    page: PullRequestPage,
    navigator: Navigator,
    services: Services,
}

struct App {
    harness: Harness<'static, Option<Scene>>,
    host: Arc<Mutex<Host>>,
    origin: Origin,
    _scratch: ScratchDir,
}

impl App {
    /// The pull request open on the Files tab with [`BIG`] shown side by side, everything loaded.
    fn start(id: &str) -> Self {
        let scratch = ScratchDir::new(id);
        let origin = Origin::create(&scratch.path().join("origin"));
        let host = Arc::new(Mutex::new(Host {
            detail: detail(&origin.target, &origin.head),
            threads: vec![samples::thread(
                "t-0",
                ThreadStatus::Active,
                Some(samples::lines(&small_file(0), 1, 1)),
                vec![samples::comment("c-0", &samples::jonas(), 30, "Is this used?")],
            )],
            clone_url: origin.clone_url(),
        }));
        HOSTS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(id.to_owned(), host.clone());
        let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
        let mut store = ConnectionStore::open(&paths).expect("empty store");
        store
            .upsert(Connection {
                id: id.to_owned(),
                name: "github.com".to_owned(),
                kind: ProviderKind::GitHub,
                base_url: "https://api.github.com".to_owned(),
                organizations: Vec::new(),
                token: Some("ghp_walkthrough".to_owned()),
            })
            .expect("store the connection");
        drop(store);
        let harness = Harness::builder()
            .with_size([1500.0, 900.0])
            .with_step_dt(FRAME)
            .with_context_setup(theme::fonts::install)
            .allow_missing_glyphs()
            .build_ui_state(
                |ui, scene: &mut Option<Scene>| {
                    let Some(Scene { page, navigator, services }) = scene else {
                        return;
                    };
                    let ctx = ui.ctx().clone();
                    surfaces::begin_frame(&ctx);
                    theme::activate(&ctx, services.settings.theme);
                    focus_mode::begin_frame(&ctx, false);
                    page.show(ui, services, navigator);
                    focus_mode::end_frame(&ctx);
                },
                None,
            );
        let mut app = Self { harness, host, origin, _scratch: scratch };
        let mut services = Services::new(app.harness.ctx.clone(), paths, scripted_provider);
        services.settings.pull_request_start = PullRequestStart::Files;
        services.settings.files_start = FilesStart::FirstFile;
        services.settings.diff_layout = DiffLayout::SideBySide;
        let key = PullRequestKey {
            connection_id: id.to_owned(),
            repository_key: REPOSITORY.to_owned(),
            number: NUMBER,
        };
        let page = PullRequestPage::new(key, &mut services);
        *app.harness.state_mut() = Some(Scene { page, navigator: Navigator::default(), services });
        let input = app.harness.input_mut();
        input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(true);
        input.focused = true;
        // Colors come from their own worker threads, which need real time, not frames.
        let started = Instant::now();
        while !(app.shows_diff() && app.is_settled()) {
            assert!(
                started.elapsed() < REAL_LIMIT,
                "the open file, colored and counted, did not come: {}",
                app.loads()
            );
            app.frame();
            test_support::let_threads_run(Duration::from_millis(10));
        }
        app
    }

    fn scene(&self) -> &Scene {
        self.harness.state().as_ref().expect("the scene is set up")
    }

    fn page(&self) -> &PullRequestPage {
        &self.scene().page
    }

    fn host(&self) -> MutexGuard<'_, Host> {
        self.host.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> f64 {
        self.harness.ctx.input(|input| input.time)
    }

    /// One frame, then the real time the answers it asked for need, so the next frame has them.
    fn frame(&mut self) {
        self.harness.step();
        let started = Instant::now();
        while self.scene().services.background.running() > 0 {
            assert!(started.elapsed() < REAL_LIMIT, "an answer never came");
            test_support::let_threads_run(Duration::from_millis(1));
        }
    }

    /// One frame and a moment of real time, without waiting for the answers: what is on its way
    /// stays on its way for a few frames, as over a network.
    fn glance(&mut self) {
        self.harness.step();
        test_support::let_threads_run(Duration::from_millis(1));
    }

    /// Nothing is loading: the page, the open file's colors and the line counts are in.
    fn is_settled(&self) -> bool {
        let page = self.page();
        let Ok(session) = &page.session else {
            return false;
        };
        let files = &page.files;
        let is_shown = match files.selected_path.as_deref() {
            Some(path) if files.preview.shows_rendered(path) => !files.preview.is_loading(),
            _ => files.diffs.coloring_is_done(),
        };
        !session.detail.is_loading()
            && !session.threads.is_loading()
            && !session.mirror.is_loading()
            && !session.files_fast.is_loading()
            && !session.files_detailed.is_loading()
            && !session.commits.is_loading()
            && is_shown
    }

    /// What is still loading, for a failure message.
    fn loads(&self) -> String {
        let page = self.page();
        let Ok(session) = &page.session else {
            return "the pull request did not open".to_owned();
        };
        format!(
            "detail {}, threads {}, mirror {} ({:?}), fast list {}, counted list {}, commits {}, \
             colored {}, diff shown {}, selected {:?}, live: {}",
            session.detail.is_loading(),
            session.threads.is_loading(),
            session.mirror.is_loading(),
            session.mirror.error(),
            session.files_fast.is_loading(),
            session.files_detailed.is_loading(),
            session.commits.is_loading(),
            page.files.diffs.coloring_is_done(),
            self.shows_diff(),
            page.files.selected_path,
            page.live.describe(),
        )
    }

    fn shows_diff(&self) -> bool {
        let label = format!("Diff of {BIG}");
        self.harness.query_all_by_label_contains(&label).next().is_some()
    }

    /// Scrolls the file tree and the open file's diff down and its head side sideways; returns
    /// the place they came to rest at.
    fn scroll_everything(&mut self) -> Place {
        let before = self.place();
        let tree_row = self.harness.get_by_role_and_label(Role::TreeItem, &small_file(3)).rect();
        self.wheel(tree_row.center(), vec2(0.0, -400.0));
        let diff = self.harness.get_by_label_contains(&format!("Diff of {BIG}")).rect();
        let head_side = Pos2::new(diff.left() + diff.width() * 0.75, diff.center().y);
        self.wheel(head_side, vec2(0.0, -1500.0));
        self.wheel(head_side, vec2(-300.0, 0.0));
        // The wheel scrolls smoothly over a few frames.
        for _ in 0..20 {
            self.frame();
        }
        let place = self.place();
        assert_ne!(place.tree, before.tree, "the file tree scrolled");
        assert!(place.view.0 > 1000.0, "the diff scrolled down: {:?}", place.view);
        assert!(place.view.1[1] > 100.0, "the head side scrolled sideways: {:?}", place.view);
        assert_eq!(place.selected.as_deref(), Some(BIG));
        place
    }

    fn wheel(&mut self, at: Pos2, delta: egui::Vec2) {
        self.harness.hover_at(at);
        self.frame();
        self.harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
            source: egui::MouseWheelSource::Wheel,
        });
        self.frame();
    }

    fn place(&self) -> Place {
        let files = &self.page().files;
        Place {
            selected: files.selected_path.clone(),
            top_line: focus_mode::top_line(&self.harness.ctx, Surface::Files),
            view: files.view.kept_place(),
            diff: files.diffs.ready().map_or(0, |diff| Arc::as_ptr(diff) as usize),
            diff_rect: self.harness.get_by_label_contains(&format!("Diff of {BIG}")).rect(),
            header_hidden: self.page().header_reveal.drawn_hidden(),
            tree: self.first_tree_row(),
        }
    }

    fn rendered(&self) -> RenderedPlace {
        let page = self.page();
        let session = page.session.as_ref().expect("the pull request opened");
        let guide = session.files().and_then(|files| files.iter().find(|file| file.path == GUIDE));
        (
            page.files.selected_path.clone(),
            guide.and_then(|file| page.files.preview.shown(file)),
            page.files.preview.top_line(),
            page.header_reveal.drawn_hidden(),
            self.first_tree_row(),
        )
    }

    fn first_tree_row(&self) -> Option<(String, Rect)> {
        let node = self.harness.query_all_by_role(Role::TreeItem).next()?;
        Some((node.accesskit_node().label().unwrap_or_default(), node.rect()))
    }

    /// A push to small file `step` on the host.
    fn push(&mut self, step: usize) {
        let head = self.origin.push_to_other_file(step);
        self.host().detail.head_sha = head;
    }

    /// Lets the host change through `change`, brings the change to the page `way`, and frames
    /// until it is on screen and everything settled; `steady` checks every frame on the way.
    fn update(
        &mut self,
        what: &str,
        way: Way,
        change: impl FnOnce(&mut Self),
        steady: impl Fn(&Self, &str),
    ) {
        match way {
            Way::Live => change(self),
            Way::AfterWrite => {
                // The write itself is what changes the pull request, as publishing a draft does;
                // its effect shows at once, then the pull request reloads.
                change(self);
                let is_draft = self.host().detail.summary.is_draft;
                let scene = self.harness.state_mut().as_mut().expect("the scene is set up");
                let ticket = scene.page.actions.run(
                    &scene.services,
                    "Saved.",
                    Refresh::PullRequest,
                    (&scene.page.key, Effect::Draft(is_draft)),
                    async { Ok(()) },
                );
                assert!(ticket.is_some(), "no other write runs");
            }
            Way::F5 => {
                change(self);
                self.harness.key_press(egui::Key::F5);
            }
        }
        // Simulated time counts only the frames that start with nothing in flight: the host
        // answers within a frame, so the time an answer takes in real time (git under the load
        // of the whole suite) is no simulated time the app spent waiting.
        let (mut waited_idle, started) = (0.0, Instant::now());
        loop {
            steady(self, what);
            let shows_host = self.shows_host();
            if shows_host && self.is_settled() {
                break;
            }
            // The update has to come within the probe's time; its answers and counting lines take
            // real time.
            assert!(started.elapsed() < REAL_LIMIT, "{what} did not settle: {}", self.loads());
            assert!(
                shows_host || waited_idle <= UPDATE_SECONDS,
                "{what} did not come within {UPDATE_SECONDS} s: {}",
                self.loads()
            );
            let is_idle = self.scene().services.background.running() == 0;
            let before = self.now();
            self.glance();
            if is_idle {
                waited_idle += self.now() - before;
            }
        }
        // The frames after it change nothing either (late answers, colors, the next probe).
        for _ in 0..8 {
            self.frame();
            steady(self, what);
        }
    }

    /// The page shows what the host has now.
    fn shows_host(&self) -> bool {
        let Ok(session) = &self.page().session else {
            return false;
        };
        let host = self.host();
        let is_mirrored =
            session.mirror.ready().is_some_and(|mirror| mirror.head == host.detail.head_sha);
        session.detail.ready() == Some(&host.detail)
            && session.threads.ready().is_some_and(|threads| threads.len() == host.threads.len())
            && is_mirrored
    }

    /// The frame shows the open file's diff at `place`: no preparing screen, no skeleton, no
    /// empty area, nothing loaded or laid out again, nothing scrolled.
    fn assert_steady(&self, place: &Place, what: &str) {
        let page = self.page();
        let session = page.session.as_ref().expect("the pull request opened");
        assert!(session.files().is_some(), "{what}: the file list went away");
        assert!(page.files.diffs.ready().is_some(), "{what}: the open file's diff is loading");
        assert!(self.shows_diff(), "{what}: the diff of {BIG} is not on screen");
        assert_eq!(&self.place(), place, "{what} moved the Files tab");
    }
}

/// The repository behind the pull request: [`BIG`] and the small files on `main`, each changed on
/// `feature`.
struct Origin {
    dir: PathBuf,
    target: String,
    head: String,
}

impl Origin {
    fn create(dir: &Path) -> Self {
        std::fs::create_dir_all(dir.join("app")).expect("origin folder");
        std::fs::create_dir_all(dir.join("pkg")).expect("origin folder");
        std::fs::create_dir_all(dir.join("docs")).expect("origin folder");
        let git = |args: &[&str]| test_support::git(dir, args);
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "uploadpack.allowFilter", "true"]);
        git(&["config", "uploadpack.allowAnySHA1InWant", "true"]);
        std::fs::write(dir.join(BIG), big_file(false)).expect("write the big file");
        std::fs::write(dir.join(GUIDE), guide(false)).expect("write the guide");
        for index in 0..SMALL_FILES {
            std::fs::write(dir.join(small_file(index)), format!("pub fn f{index}() {{}}\n"))
                .expect("write a small file");
        }
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "Widgets"]);
        let target = git(&["rev-parse", "HEAD"]);
        git(&["switch", "-q", "-c", "feature"]);
        std::fs::write(dir.join(BIG), big_file(true)).expect("change the big file");
        std::fs::write(dir.join(GUIDE), guide(true)).expect("change the guide");
        for index in 0..SMALL_FILES {
            std::fs::write(
                dir.join(small_file(index)),
                format!("pub fn f{index}() -> u8 {{ 0 }}\n"),
            )
            .expect("change a small file");
        }
        git(&["commit", "-q", "-am", "Cache the widgets"]);
        let head = git(&["rev-parse", "HEAD"]);
        Self { dir: dir.to_owned(), target, head }
    }

    fn clone_url(&self) -> String {
        format!("file:///{}", self.dir.display().to_string().replace('\\', "/"))
    }

    fn git(&self, args: &[&str]) -> String {
        test_support::git(&self.dir, args)
    }

    /// A commit on `main` that touches nothing the pull request changes; returns it.
    fn commit_on_main(&self, step: usize) -> String {
        self.git(&["switch", "-q", "main"]);
        std::fs::write(self.dir.join("NOTES.md"), format!("Release {step}\n")).expect("notes");
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", &format!("Release {step}")]);
        let target = self.git(&["rev-parse", "HEAD"]);
        self.git(&["switch", "-q", "feature"]);
        target
    }

    /// A commit on `feature` that adds a line to small file `index`; returns the new head.
    fn push_to_other_file(&self, index: usize) -> String {
        let path = self.dir.join(small_file(index));
        let text = std::fs::read_to_string(&path).expect("small file");
        std::fs::write(&path, format!("// Pushed.\n{text}")).expect("pushed file");
        self.git(&["commit", "-q", "-am", &format!("Document f{index}")]);
        self.git(&["rev-parse", "HEAD"])
    }
}

fn small_file(index: usize) -> String {
    format!("pkg/f{index:02}.rs")
}

/// 400 lines wider than the pane; the head changes every fifth, so the diff shows every line.
fn big_file(is_head: bool) -> String {
    (1..=400)
        .map(|line| {
            let padding = "w".repeat(260);
            if is_head && line % 5 == 0 {
                format!("let cached_{line} = \"{padding}\";\n")
            } else {
                format!("let widget_{line} = \"{padding}\";\n")
            }
        })
        .collect()
}

/// 200 sections of a paragraph each; the head rewords every tenth.
fn guide(is_head: bool) -> String {
    use std::fmt::Write as _;

    let mut text = String::new();
    for section in 1..=200 {
        let verb = if is_head && section % 10 == 0 { "caches" } else { "keeps" };
        write!(text, "## Section {section}\n\nThe widget {verb} its layout for {section}.\n\n")
            .expect("a string takes any text");
    }
    text
}

fn detail(target: &str, head: &str) -> PullRequestDetail {
    PullRequestDetail {
        summary: PullRequestSummary {
            repository_key: REPOSITORY.to_owned(),
            repository_name: "widgets".to_owned(),
            number: NUMBER,
            title: "Speed up the widget cache".to_owned(),
            author: samples::erika(),
            state: PullRequestState::Open,
            is_draft: true,
            source_branch: "feature".to_owned(),
            target_branch: "main".to_owned(),
            created_at: datetime!(2026-10-27 08:30 UTC),
            updated_at: None,
            web_url: format!("https://github.com/{REPOSITORY}/pull/{NUMBER}"),
            labels: Vec::new(),
            reviewers: Vec::new(),
            checks: None,
            diff_stat: review_core::DiffStat::default(),
            head_sha: None,
            target_sha: None,
        },
        description: "Caches each widget once.".to_owned(),
        target_sha: target.to_owned(),
        head_sha: head.to_owned(),
        merge_status: MergeStatus::Mergeable,
        git_fetch_refs: vec!["refs/heads/feature".to_owned(), "refs/heads/main".to_owned()],
        closed_at: None,
        auto_complete: None,
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "a ProviderFactory, whose signature hands the connection and the client over"
)]
fn scripted_provider(connection: Connection, _http: reqwest::Client) -> Arc<dyn Provider> {
    let host = HOSTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|hosts| hosts.get(&connection.id).cloned())
        .expect("every connection of this walkthrough has a host");
    let capabilities = Capabilities {
        votes: vec![ReviewVote::Approved, ReviewVote::None],
        merge_strategies: vec![MergeStrategy::Merge],
        has_label_catalog: false,
        running_logs: RunningLogs::WhenFinished,
    };
    Arc::new(ScriptedProvider { host, capabilities })
}

struct ScriptedProvider {
    host: Arc<Mutex<Host>>,
    capabilities: Capabilities,
}

impl ScriptedProvider {
    fn unused<T>() -> Result<T> {
        Err(ProviderError::Unsupported("Not part of the update place walkthrough.".to_owned()))
    }

    fn host(&self) -> MutexGuard<'_, Host> {
        self.host.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(samples::jonas())
    }

    async fn list_repositories(&self, _search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        Ok(Vec::new())
    }

    async fn git_remote(&self, _repo: &str) -> Result<GitRemote> {
        Ok(GitRemote { clone_url: self.host().clone_url.clone(), authorization_header: None })
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        Ok(Vec::new())
    }

    async fn pull_request(&self, _repo: &str, _number: u64) -> Result<PullRequestDetail> {
        Ok(self.host().detail.clone())
    }

    async fn merge(&self, _repo: &str, _number: u64, _options: &MergeOptions) -> Result<()> {
        Self::unused()
    }

    async fn close(&self, _repo: &str, _number: u64) -> Result<()> {
        Self::unused()
    }

    async fn reopen(&self, _repo: &str, _number: u64) -> Result<()> {
        Self::unused()
    }

    async fn set_draft(&self, _repo: &str, _number: u64, _is_draft: bool) -> Result<()> {
        Self::unused()
    }

    async fn update_title_description(
        &self,
        _repo: &str,
        _number: u64,
        _title: &str,
        _description: &str,
    ) -> Result<()> {
        Self::unused()
    }

    async fn set_vote(
        &self,
        _repo: &str,
        _number: u64,
        _vote: ReviewVote,
        _comment: Option<&str>,
    ) -> Result<()> {
        Self::unused()
    }

    async fn add_reviewer(&self, _repo: &str, _number: u64, _user: &str) -> Result<()> {
        Self::unused()
    }

    async fn remove_reviewer(&self, _repo: &str, _number: u64, _reviewer_id: &str) -> Result<()> {
        Self::unused()
    }

    async fn threads(&self, _repo: &str, _number: u64) -> Result<Vec<ReviewThread>> {
        Ok(self.host().threads.clone())
    }

    async fn create_thread(
        &self,
        _repo: &str,
        _number: u64,
        _thread: &NewThread,
    ) -> Result<ReviewThread> {
        Self::unused()
    }

    async fn reply(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _body: &str,
    ) -> Result<ThreadComment> {
        Self::unused()
    }

    async fn set_thread_status(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _status: ThreadStatus,
    ) -> Result<()> {
        Self::unused()
    }

    async fn available_labels(&self, _repo: &str) -> Result<Vec<Label>> {
        Ok(Vec::new())
    }

    async fn set_labels(&self, _repo: &str, _number: u64, _labels: &[String]) -> Result<()> {
        Self::unused()
    }

    async fn pipeline_runs(&self, _repo: &str, _number: u64) -> Result<Vec<PipelineRun>> {
        Ok(Vec::new())
    }

    async fn pipeline_jobs(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        Ok(Vec::new())
    }

    async fn download_log(
        &self,
        _repo: &str,
        _number: u64,
        _log_id: &str,
        _sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()> {
        Self::unused()
    }
}
