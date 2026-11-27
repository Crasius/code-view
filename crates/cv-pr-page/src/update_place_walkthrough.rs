//! An update of the pull request that leaves the open file alone leaves the Files tab alone: the
//! same file stays open, its diff keeps its rows and its scroll position (down and sideways), the
//! file tree keeps its scroll, and no frame in between shows a loading state. Checked for every way
//! an update comes in: the live probe, the refresh after the user's own write, and F5; for changes
//! of the pull request's metadata, a comment elsewhere, a target branch that moved on, and a push
//! that does not touch the open file, both for a code diff and a rendered Markdown file. The header
//! keeps its height and its slid-away state, so nothing under it moves on screen either. Text in
//! the vote composer stays through F5 and a switch to another tab.
//!
//! Offline: a scripted provider serves the pull request from a local git repository the mirror
//! fetches from. The clock is the harness's simulated frames; between two frames the answers get
//! the real time they need, so a frame stands for any round trip.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cv_testkit::Clicks;
use egui::{Modifiers, Pos2, Rect, Role, vec2};
use egui_kittest::kittest::{NodeT as _, Queryable};
use review_core::{
    DiffSide, GitRemote, Label, MergeStatus, PullRequestDetail, PullRequestSummary, Result,
    ReviewThread, ReviewVote, Reviewer, ThreadStatus,
};
use review_store::{DiffLayout, FilesStart, PullRequestStart};

use crate::PullRequestPage;
use crate::page_harness::PageApp;
use cv_pr_files::probe;
use cv_pr_session::actions::Refresh;
use cv_pr_threads::samples;
use cv_runtime::route::{PullRequestKey, PullRequestTab as Tab};
use cv_services::writes::Effect;
use cv_shell::focus_mode::{self, Surface};
use cv_testkit::provider::{Script, Scripted};
use cv_testkit::{ScratchDir, WAIT_LIMIT};

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
    let harness = &app.kit.harness;
    let pane = harness.get_by_label_contains(&format!("Diff of {BIG}")).rect();
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, GUIDE).rect());
    app.frame_until(&format!("the rendered {GUIDE}"), |app| {
        app.rendered().1.is_some() && app.is_settled()
    });
    app.wheel(pane.center(), vec2(0.0, -1500.0));
    let place = app.frame_until_still("the rendered file's scroll", App::rendered);
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

/// Text typed into the vote composer lives with the page: F5 and a switch to another tab keep it,
/// as they keep the page.
#[test]
fn the_vote_composers_text_survives_a_reload_and_a_tab_switch() {
    let mut app = App::start("keep-vote-draft");
    assert!(!app.page().has_unsent_input(), "nothing is typed yet");
    app.kit.harness.click_at(app.kit.harness.get_by_label("More votes").rect());
    app.frame_until("the vote menu", |app| app.shows("Approve with comment…"));
    app.kit.harness.click_at(app.kit.harness.get_by_label("Approve with comment…").rect());
    app.frame_until("the focused composer", |app| app.kit.harness.ctx.text_edit_focused());
    app.kit.harness.event(egui::Event::Text("Ship it once the cache is warm.".to_owned()));
    app.kit.frame();
    assert!(app.page().has_unsent_input(), "the composer holds the text");

    app.update(
        "F5 with the vote composer open",
        Way::F5,
        |_| {},
        |app, what| assert!(app.page().has_unsent_input(), "{what} lost the composer's text"),
    );
    app.kit.page_mut().show_tab(Tab::Overview);
    app.frame_until("the Overview", |app| app.shows("Merge status"));
    assert_eq!(app.page().tab(), Tab::Overview);
    assert!(app.page().has_unsent_input(), "the Overview tab lost the composer's text");
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

struct App {
    kit: PageApp,
    host: Arc<Mutex<Host>>,
    origin: Origin,
}

impl App {
    /// The pull request open on the Files tab with [`BIG`] shown side by side, everything loaded.
    fn start(id: &str) -> Self {
        let origin = Origin::create(&format!("{id}-origin"));
        let host = Arc::new(Mutex::new(Host {
            detail: detail(&origin.target, &origin.head),
            threads: vec![samples::thread(
                "t-0",
                ThreadStatus::Active,
                Some(samples::lines(&small_file(0), 1, 1)),
                vec![samples::comment("c-0", &samples::jonas(), 30, "Is this used?")],
            )],
            clone_url: cv_testkit::file_url(origin.dir.path()),
        }));
        let key = PullRequestKey {
            connection_id: id.to_owned(),
            repository_key: REPOSITORY.to_owned(),
            number: NUMBER,
        };
        let platform = host.clone();
        let kit = PageApp::start(
            key,
            FRAME,
            Some("ghp_walkthrough"),
            |services| {
                services.settings.pull_request_start = PullRequestStart::Files;
                services.settings.files_start = FilesStart::FirstFile;
                services.settings.diff_layout = DiffLayout::SideBySide;
            },
            move |_, _| Arc::new(Scripted(Platform(platform.clone()))),
        );
        let mut app = Self { kit, host, origin };
        app.kit.set_focus(true);
        // Colors come from their own worker threads, which need real time, not frames.
        app.frame_until("the open file, colored and counted", |app| {
            app.shows_diff() && app.is_settled()
        });
        app
    }

    fn page(&self) -> &PullRequestPage {
        self.kit.page()
    }

    fn host(&self) -> MutexGuard<'_, Host> {
        self.host.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One frame and a moment of real time, without waiting for the answers: what is on its way
    /// stays on its way for a few frames, as over a network.
    fn glance(&mut self) {
        self.kit.harness.step();
        cv_testkit::let_threads_run(Duration::from_millis(1));
    }

    /// Nothing is loading: the page, the open file's colors and the line counts are in.
    fn is_settled(&self) -> bool {
        let page = self.page();
        let Ok(session) = &page.session else {
            return false;
        };
        let files = &page.files;
        let preview = probe::preview(files);
        let is_shown = match probe::selected_path(files) {
            Some(path) if preview.shows_rendered(path) => !preview.is_loading(),
            _ => probe::coloring_is_done(files),
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
            probe::coloring_is_done(&page.files),
            self.shows_diff(),
            probe::selected_path(&page.files),
            page.live.describe(),
        )
    }

    /// Frames until `is_there` holds.
    fn frame_until(&mut self, what: &str, is_there: impl Fn(&Self) -> bool) {
        cv_testkit::poll_until(what, || {
            let is_done = is_there(self);
            if !is_done {
                self.kit.frame();
            }
            is_done
        });
    }

    /// Frames until `look` sees the same on two frames in a row, as when a smooth scroll came to
    /// rest; returns what it saw.
    fn frame_until_still<T: PartialEq>(&mut self, what: &str, look: impl Fn(&Self) -> T) -> T {
        let mut last = look(self);
        cv_testkit::poll_for(what, || {
            self.kit.frame();
            let now = look(self);
            if now == last {
                return Some(now);
            }
            last = now;
            None
        })
    }

    fn shows(&self, label: &str) -> bool {
        self.kit.harness.query_all_by_label(label).next().is_some()
    }

    fn shows_diff(&self) -> bool {
        let label = format!("Diff of {BIG}");
        self.kit.harness.query_all_by_label_contains(&label).next().is_some()
    }

    /// Scrolls the file tree and the open file's diff down and its head side sideways; returns
    /// the place they came to rest at.
    fn scroll_everything(&mut self) -> Place {
        let before = self.place();
        let tree_row =
            self.kit.harness.get_by_role_and_label(Role::TreeItem, &small_file(3)).rect();
        self.wheel(tree_row.center(), vec2(0.0, -400.0));
        let diff = self.kit.harness.get_by_label_contains(&format!("Diff of {BIG}")).rect();
        let head_side = Pos2::new(diff.left() + diff.width() * 0.75, diff.center().y);
        self.wheel(head_side, vec2(0.0, -1500.0));
        self.wheel(head_side, vec2(-300.0, 0.0));
        let place = self.frame_until_still("the scrolled Files tab", Self::place);
        assert_ne!(place.tree, before.tree, "the file tree scrolled");
        assert!(place.view.0 > 1000.0, "the diff scrolled down: {:?}", place.view);
        assert!(place.view.1[1] > 100.0, "the head side scrolled sideways: {:?}", place.view);
        assert_eq!(place.selected.as_deref(), Some(BIG));
        place
    }

    fn wheel(&mut self, at: Pos2, delta: egui::Vec2) {
        self.kit.harness.hover_at(at);
        self.kit.frame();
        self.kit.harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
            source: egui::MouseWheelSource::Wheel,
        });
        self.kit.frame();
    }

    fn place(&self) -> Place {
        let files = &self.page().files;
        Place {
            selected: probe::selected_path(files).map(str::to_owned),
            top_line: focus_mode::top_line(&self.kit.harness.ctx, Surface::Files),
            view: probe::kept_place(files),
            diff: probe::ready_diff(files).map_or(0, |diff| Arc::as_ptr(diff) as usize),
            diff_rect: self.kit.harness.get_by_label_contains(&format!("Diff of {BIG}")).rect(),
            header_hidden: self.page().header_reveal.drawn_hidden(),
            tree: self.first_tree_row(),
        }
    }

    fn rendered(&self) -> RenderedPlace {
        let page = self.page();
        let session = page.session.as_ref().expect("the pull request opened");
        let guide = session.files().and_then(|files| files.iter().find(|file| file.path == GUIDE));
        (
            probe::selected_path(&page.files).map(str::to_owned),
            guide.and_then(|file| probe::preview(&page.files).shown(file)),
            probe::preview(&page.files).top_line(),
            page.header_reveal.drawn_hidden(),
            self.first_tree_row(),
        )
    }

    fn first_tree_row(&self) -> Option<(String, Rect)> {
        let node = self.kit.harness.query_all_by_role(Role::TreeItem).next()?;
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
                let scene = self.kit.scene_mut();
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
                self.kit.harness.key_press(egui::Key::F5);
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
            assert!(started.elapsed() < WAIT_LIMIT, "{what} did not settle: {}", self.loads());
            assert!(
                shows_host || waited_idle <= UPDATE_SECONDS,
                "{what} did not come within {UPDATE_SECONDS} s: {}",
                self.loads()
            );
            let is_idle = self.kit.scene().services.background.running() == 0;
            let before = self.kit.now();
            self.glance();
            if is_idle {
                waited_idle += self.kit.now() - before;
            }
        }
        // The frames after it change nothing either (late answers, colors, the next probe).
        for _ in 0..8 {
            self.kit.frame();
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
        assert!(
            probe::ready_diff(&page.files).is_some(),
            "{what}: the open file's diff is loading"
        );
        assert!(self.shows_diff(), "{what}: the diff of {BIG} is not on screen");
        assert_eq!(&self.place(), place, "{what} moved the Files tab");
    }
}

/// The repository behind the pull request: [`BIG`] and the small files on `main`, each changed on
/// `feature`.
struct Origin {
    dir: ScratchDir,
    target: String,
    head: String,
}

impl Origin {
    /// The repository in a scratch folder of its own, `name`.
    fn create(name: &str) -> Self {
        let scratch = ScratchDir::new(name);
        let dir = scratch.path();
        std::fs::create_dir_all(dir.join("app")).expect("origin folder");
        std::fs::create_dir_all(dir.join("pkg")).expect("origin folder");
        std::fs::create_dir_all(dir.join("docs")).expect("origin folder");
        let git = |args: &[&str]| cv_testkit::git(dir, args);
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
        Self { dir: scratch, target, head }
    }

    fn git(&self, args: &[&str]) -> String {
        cv_testkit::git(self.dir.path(), args)
    }

    /// A commit on `main` that touches nothing the pull request changes; returns it.
    fn commit_on_main(&self, step: usize) -> String {
        self.git(&["switch", "-q", "main"]);
        std::fs::write(self.dir.path().join("NOTES.md"), format!("Release {step}\n"))
            .expect("notes");
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", &format!("Release {step}")]);
        let target = self.git(&["rev-parse", "HEAD"]);
        self.git(&["switch", "-q", "feature"]);
        target
    }

    /// A commit on `feature` that adds a line to small file `index`; returns the new head.
    fn push_to_other_file(&self, index: usize) -> String {
        let path = self.dir.path().join(small_file(index));
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
    let detail = cv_testkit::samples::detail(REPOSITORY, NUMBER);
    PullRequestDetail {
        summary: PullRequestSummary {
            title: "Speed up the widget cache".to_owned(),
            author: samples::erika(),
            is_draft: true,
            source_branch: "feature".to_owned(),
            ..detail.summary
        },
        description: "Caches each widget once.".to_owned(),
        target_sha: target.to_owned(),
        head_sha: head.to_owned(),
        git_fetch_refs: vec!["refs/heads/feature".to_owned(), "refs/heads/main".to_owned()],
        ..detail
    }
}

/// The platform as the page sees it: what the host has now, cloned from the origin.
struct Platform(Arc<Mutex<Host>>);

impl Platform {
    fn host(&self) -> MutexGuard<'_, Host> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl Script for Platform {
    async fn git_remote(&self, _repo: &str) -> Result<GitRemote> {
        Ok(GitRemote { clone_url: self.host().clone_url.clone(), authorization_header: None })
    }

    async fn pull_request(&self, _repo: &str, _number: u64) -> Result<PullRequestDetail> {
        Ok(self.host().detail.clone())
    }

    async fn threads(&self, _repo: &str, _number: u64) -> Result<Vec<ReviewThread>> {
        Ok(self.host().threads.clone())
    }
}
