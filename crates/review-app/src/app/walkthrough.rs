//! The walkthrough kit: how a test starts the whole app, waits for it, clicks and reads it.
//!
//! The waits differ in when they return. [`wait_until`] runs [`SETTLE_FRAMES`] more frames after
//! its condition held, so what the frame that met it started has landed. [`wait_until_unsettled`] returns on the
//! frame the condition held. [`wait_for`] returns a value, also on that frame, and
//! [`wait_until_still`] a value that two frames in a row gave (a place at rest). The other ways to
//! let time pass: [`settle_background`] (three frames without background work, before a place is
//! read), [`wait_idle`] (twenty such frames, for chains of work), [`pump`] (simulated seconds) and
//! `harness.run_steps(n)` (only after pure UI input on data already loaded).
//!
//! Every wait is bounded: it fails after [`IDLE_LIMIT`] with nothing in flight in the background,
//! or after [`LIMIT`] at all, and names what it waited for and what the screen showed. Rule
//! `testing` still names this kit by its old home, `action_e2e`.
//!
//! The app runs headless (`egui_kittest`), driven through its accessibility tree and pointer
//! events like a user: [`app`] on the real providers against mock hosts ([`super::github_mock`]),
//! [`app_with`] on providers of the test's own.

use std::cell::Cell;
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{
    CursorIcon, Event, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, Rect,
    Role, TouchPhase, pos2, vec2,
};
use egui_kittest::kittest::{AccessKitNode, NodeT, Queryable};
use egui_kittest::{Harness, HarnessBuilder};
use review_core::{Connection, Provider};
use review_store::{AppPaths, AppSettings, ConnectionStore, DiffLayout};

use super::{ReviewApp, startup};
use cv_testkit::{Clicks, ScratchDir};

/// How long a wait may go on at all. Work in flight counts only here: it goes at the pace of git
/// and the machine, which the whole suite and whatever else runs there share (a git start took
/// 1 s at the median while another project's tests kept the machine 83 % busy).
pub(super) use cv_testkit::WAIT_LIMIT as LIMIT;

/// How long a wait may go on while nothing is in flight in the background: the app is idle, and
/// what it waits for did not come.
const IDLE_LIMIT: Duration = Duration::from_secs(60);

/// `scratch`'s data folders with `connections` saved.
pub(super) fn saved_paths(scratch: &ScratchDir, connections: &[Connection]) -> AppPaths {
    let paths = scratch.app_paths();
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    for connection in connections {
        store.upsert(connection.clone()).expect("store the connection");
    }
    paths
}

/// The app on `paths`, its providers from `factory`, once its start settled ([`startup::start`]).
pub(super) fn app_with(
    builder: HarnessBuilder<ReviewApp>,
    paths: AppPaths,
    factory: impl Fn(Connection, reqwest::Client) -> Arc<dyn Provider> + Send + Sync + 'static,
) -> Harness<'static, ReviewApp> {
    startup::start(builder, move |creation| ReviewApp::new(creation, paths, factory))
}

/// The real providers on `connections` (mock hosts), in a 1440 × 900 window, with a side-by-side
/// diff and the "My pull requests" start page.
pub(super) fn app(scratch: &ScratchDir, connections: &[Connection]) -> Harness<'static, ReviewApp> {
    let mut harness = app_with(
        Harness::builder().with_size([1440.0, 900.0]),
        saved_paths(scratch, connections),
        cv_providers::create_provider,
    );
    harness.state_mut().services.settings.diff_layout = DiffLayout::SideBySide;
    harness
}

/// The simulated seconds a frame of [`long_session_builder`] moves egui's clock on by.
pub(super) const LONG_SESSION_STEP: f32 = 0.5;

/// A harness for scenes that let simulated minutes pass (the request budget, the memory
/// walkthrough): a 1440 × 900 window, [`LONG_SESSION_STEP`] a frame, before the renderer is
/// picked.
pub(super) fn long_session_builder() -> HarnessBuilder<ReviewApp> {
    Harness::builder().with_size([1440.0, 900.0]).with_step_dt(LONG_SESSION_STEP)
}

/// Wheels the list of a 1440 × 900 window down by `points` from over its rows, then lets two
/// simulated seconds pass.
pub(super) fn scroll_list(harness: &mut Harness<'_, ReviewApp>, points: f32) {
    harness.hover_at(pos2(720.0, 600.0));
    let mouse = |delta: f32, phase| wheel(delta, phase, MouseWheelSource::Wheel);
    harness.event(mouse(0.0, TouchPhase::Start));
    for _ in 0..4 {
        harness.event(mouse(-points / 4.0, TouchPhase::Move));
        harness.step();
    }
    harness.event(mouse(0.0, TouchPhase::End));
    pump(harness, 2.0);
}

/// Steps the app until `find` finds something and returns it on the frame it came, without
/// settling; `each` runs before every frame (a hover, a click). Fails after [`IDLE_LIMIT`] with
/// nothing in flight in the background, or after [`LIMIT`].
pub(super) fn wait_for<T>(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    mut find: impl FnMut(&Harness<'_, ReviewApp>) -> Option<T>,
    each: impl Fn(&mut Harness<'_, ReviewApp>),
) -> T {
    let _step = cv_testkit::step(format!("waiting for {what}"));
    let (started, mut idle) = (Instant::now(), Duration::ZERO);
    loop {
        let (is_busy, frame) = (harness.state().services.background.running() > 0, Instant::now());
        each(harness);
        harness.step();
        if let Some(found) = find(harness) {
            return found;
        }
        if idle > IDLE_LIMIT || started.elapsed() > LIMIT {
            let labels: Vec<String> = harness
                .query_all_by(|_| true)
                .filter_map(|node| {
                    let node = node.accesskit_node();
                    let text = match (node.label(), node.value()) {
                        (Some(label), Some(value)) => Some(format!("{label} = {value:?}")),
                        (label, value) => label.or(value),
                    };
                    let state =
                        [(node.is_disabled(), " (disabled)"), (node.is_focused(), " (focused)")];
                    text.map(|text| {
                        state.iter().filter(|(is, _)| *is).fold(text, |text, (_, tag)| text + tag)
                    })
                })
                .collect();
            panic!("timed out waiting for {what}; screen: {labels:?}");
        }
        cv_testkit::let_threads_run(Duration::from_millis(20));
        if !is_busy {
            idle += frame.elapsed();
        }
    }
}

/// Steps the app until `is_done` holds, then four more frames.
pub(super) fn wait_until(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    wait_until_unsettled(harness, what, is_done);
    settle(harness);
}

/// Steps the app until `is_done` holds and returns on that frame.
pub(super) fn wait_until_unsettled(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    wait_for(harness, what, |harness| is_done(harness).then_some(()), |_| {});
}

/// [`wait_for`], then the four frames of [`wait_until`].
pub(super) fn wait_until_found<T>(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    find: impl Fn(&Harness<'_, ReviewApp>) -> Option<T>,
) -> T {
    let found = wait_for(harness, what, find, |_| {});
    settle(harness);
    found
}

/// Steps the app until `measure` gives the same value in two frames in a row and returns it: a
/// smooth scroll, a popup or a resized window has come to rest. `what` names what is measured.
pub(super) fn wait_until_still<T: PartialEq>(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    measure: impl Fn(&Harness<'_, ReviewApp>) -> Option<T>,
) -> T {
    let mut last = None;
    wait_for(
        harness,
        &format!("{what} to hold still"),
        |harness| {
            let now = measure(harness);
            if now.is_some() && now == last {
                return now;
            }
            last = now;
            None
        },
        |_| {},
    )
}

/// The frames [`wait_until`] runs after its condition held.
pub(super) const SETTLE_FRAMES: usize = 4;

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    harness.run_steps(SETTLE_FRAMES);
}

/// Steps until no background work (git, the provider) is left for a few frames: what was on its
/// way has landed, and the frame after it took it in. A late answer can still change the layout
/// (a card above growing), so a test reads a place to click only after this. Each step moves
/// egui's clock on by a quarter second, past the dwell of a hover.
pub(super) fn settle_background(harness: &mut Harness<'_, ReviewApp>) {
    let _step = cv_testkit::step("waiting for the background work to end over 3 frames");
    let started = Instant::now();
    let mut idle_frames = 0;
    while idle_frames < 3 {
        harness.step();
        let is_idle = harness.state().services.background.running() == 0;
        idle_frames = if is_idle { idle_frames + 1 } else { 0 };
        assert!(started.elapsed() < LIMIT, "background work never ended");
        cv_testkit::let_threads_run(Duration::from_millis(5));
    }
}

/// Waits until no background task ran for a stretch of frames: one finished task (the mirror)
/// starts the next (file lists, the chunked download of the changed blobs) only in the following
/// frame.
pub(super) fn wait_idle(harness: &mut Harness<'_, ReviewApp>) {
    let _step = cv_testkit::step("waiting for the background work to end over 20 frames");
    let started = Instant::now();
    let mut idle_frames = 0;
    while idle_frames < 20 {
        harness.step();
        cv_testkit::let_threads_run(Duration::from_millis(10));
        idle_frames =
            if harness.state().services.background.running() == 0 { idle_frames + 1 } else { 0 };
        assert!(started.elapsed() < LIMIT, "background work did not finish");
    }
}

/// Waits until the text labelled `label` is a link where it is drawn: the pointer resting on it
/// turns into a hand, and the text stays in place. An answer's citations become links only once
/// the background checked them against the checkout, and their icons then move the text; a click
/// before that lands on plain text. Returns the point to click.
pub(super) fn wait_for_link(harness: &mut Harness<'_, ReviewApp>, label: &str) -> Pos2 {
    let hovered = Cell::new(None);
    wait_for(
        harness,
        &format!("{label} to become a link"),
        |harness| {
            let is_link = harness.output().platform_output.cursor_icon == CursorIcon::PointingHand;
            let now = harness.query_by_label(label).map(|node| node.rect().center());
            hovered.get().filter(|center| is_link && now == Some(*center))
        },
        |harness| {
            let center = harness.query_by_label(label).map(|node| node.rect().center());
            if let Some(center) = center {
                harness.hover_at(center);
            }
            hovered.set(center);
        },
    )
}

/// How much older than the pointer's last move the last click must be before egui shows a
/// tooltip: a pointer resting where it clicked shows none.
const CLICK_HIDES_TOOLTIP_SECS: f32 = 0.1;

/// Rests the pointer on the widget of `role` labelled `label` until `is_shown` holds (`what`: the
/// tooltip it waits for), then runs the four frames of [`wait_until`] with the pointer still
/// there, so the tooltip has its place before a click inside it. The pointer moves again when a
/// late layout moved the widget, and after a click until it moved [`CLICK_HIDES_TOOLTIP_SECS`]
/// past it: resting alone would leave the tooltip hidden for good.
pub(super) fn hover_until(
    harness: &mut Harness<'_, ReviewApp>,
    role: Role,
    label: &str,
    what: &str,
    is_shown: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    wait_for(
        harness,
        &format!("{what} on hovering {label}"),
        |harness| is_shown(harness).then_some(()),
        |harness| {
            let center =
                harness.query_by_role_and_label(role, label).map(|node| node.rect().center());
            let is_resting = |center| {
                harness.ctx.input(|input| {
                    let pointer = &input.pointer;
                    pointer.hover_pos() == Some(center)
                        && pointer.time_since_last_click()
                            >= pointer.time_since_last_movement() + CLICK_HIDES_TOOLTIP_SECS
                })
            };
            if let Some(center) = center
                && !is_resting(center)
            {
                harness.hover_at(center);
            }
        },
    );
    settle(harness);
}

/// Scrolls the labelled widget into view, then clicks it like a pointer would (see
/// [`when_ready`]) and runs the four frames of [`wait_until`]: the press, the release, and the
/// accessibility tree, which trails the click by a frame, showing what it did.
pub(super) fn press(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    let rect = when_ready(harness, label, None);
    harness.click_at(rect);
    settle(harness);
}

/// [`press`] for the button labelled `label`: for a submit button that is enabled only with text,
/// and where another widget carries the same label (a composer's text area is named after its
/// submit button).
pub(super) fn press_button(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    press_role(harness, Role::Button, label);
}

/// [`press`] for the widget of `role` labelled `label` (a radio button among labels of the same
/// text).
pub(super) fn press_role(harness: &mut Harness<'_, ReviewApp>, role: Role, label: &str) {
    let rect = when_ready(harness, label, Some(role));
    harness.click_at(rect);
    settle(harness);
}

/// [`press`] with the secondary button: the labelled widget's context menu.
pub(super) fn secondary_press(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    let rect = when_ready(harness, label, None);
    harness.secondary_click_at(rect);
    settle(harness);
}

/// The widget labelled `label`, of `role` where one is given.
fn labelled<'h>(
    harness: &'h Harness<'_, ReviewApp>,
    label: &'h str,
    role: Option<Role>,
) -> Option<egui_kittest::Node<'h>> {
    match role {
        Some(role) => harness.query_by_role_and_label(role, label),
        None => harness.query_by_label(label),
    }
}

/// Scrolls the widget labelled `label` into view and waits until it is enabled with its centre
/// inside the window (scrolling again where late content pushed it out), the pointer resting on
/// it as a mouse's does; returns its place in that frame, for the click. A fixed number of frames
/// after the scroll proves none of that: the click would land on a disabled button, or past the
/// window's edge where nothing takes it, and the test would wait for a request that never comes.
/// The pointer moves before each frame, and a place counts only where it rested in that frame: a
/// frame that scrolled hovered nothing, so the click never lands without a hover before it.
fn when_ready(harness: &mut Harness<'_, ReviewApp>, label: &str, role: Option<Role>) -> Rect {
    wait_until(harness, label, |harness| labelled(harness, label, role).is_some());
    labelled(harness, label, role).expect("just found").scroll_to_me();
    settle(harness);
    let window = harness.ctx.content_rect();
    let hovered = Cell::new(None);
    wait_for(
        harness,
        &format!("{label} to take a click"),
        |harness| {
            let node = labelled(harness, label, role)?;
            let rect = node.rect();
            let is_enabled = !node.accesskit_node().is_disabled();
            (is_enabled && hovered.get() == Some(rect.center())).then_some(rect)
        },
        |harness| {
            let center = labelled(harness, label, role).and_then(|node| {
                let center = node.rect().center();
                if window.contains(center) {
                    return Some(center);
                }
                node.scroll_to_me();
                None
            });
            if let Some(center) = center {
                harness.hover_at(center);
            }
            hovered.set(center);
        },
    )
}

/// A right click at `position`, the pointer resting there first: the menu is the hovered line's.
pub(super) fn secondary_click(harness: &mut Harness<'_, ReviewApp>, position: Pos2) {
    harness.hover_at(position);
    harness.step();
    harness.click_point(position, PointerButton::Secondary);
    harness.step();
}

/// Clicks the first widget whose label contains `text`.
pub(super) fn click_first(harness: &mut Harness<'_, ReviewApp>, text: &str) {
    harness.click_at(
        harness
            .query_all_by_label_contains(text)
            .next()
            .unwrap_or_else(|| panic!("nothing labelled {text}"))
            .rect(),
    );
    harness.step();
}

/// Types into the text field whose hint starts with `placeholder` and returns once the field holds
/// the text: a field takes typed text only while it has the focus, so a step that follows (a
/// submit button that is enabled only with text) never runs on a field that missed it.
pub(super) fn type_into(harness: &mut Harness<'_, ReviewApp>, placeholder: &str, text: &str) {
    let is_field = |node: &AccessKitNode<'_>| {
        node.placeholder().is_some_and(|hint| hint.starts_with(placeholder))
    };
    wait_until(harness, placeholder, |harness| harness.query_by(is_field).is_some());
    harness.get_by(is_field).focus();
    harness.step();
    let field = harness.get_by(is_field);
    field.type_text(text);
    // Found by its id: a field with text may drop its hint.
    let id = field.accesskit_node().id();
    wait_until(harness, &format!("{placeholder} holding {text:?}"), |harness| {
        harness
            .query_by(|node| node.id() == id)
            .is_some_and(|field| field.value().is_some_and(|value| value.contains(text)))
    });
}

/// A primary click at `at`, and the frame that takes it.
pub(super) fn click_now(harness: &mut Harness<'_, ReviewApp>, at: Pos2) {
    harness.click_point(at, PointerButton::Primary);
    harness.step();
}

/// A wheel event of `delta_y` points; the caller hovers first and steps the frames its gesture
/// needs between the phases.
pub(super) fn wheel(delta_y: f32, phase: TouchPhase, source: MouseWheelSource) -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, delta_y),
        phase,
        modifiers: Modifiers::NONE,
        source,
    }
}

/// Whether a pull request tab is labelled `name` or `name` with its count ("Files (12)": the
/// count comes once the list is in).
fn is_tab(node: &AccessKitNode<'_>, name: &str) -> bool {
    node.role() == Role::Tab && node.label().is_some_and(|label| label.starts_with(name))
}

/// Where the pull request tab named `name` is, once it shows (see [`is_tab`]).
pub(super) fn tab_rect(harness: &Harness<'_, ReviewApp>, name: &str) -> Option<Rect> {
    harness.query_by(|node| is_tab(node, name)).map(|node| node.rect())
}

/// Clicks the Files tab of the open pull request.
pub(super) fn files_tab(harness: &mut Harness<'_, ReviewApp>) {
    harness.click_at(harness.get_by(|node| is_tab(node, "Files")).rect());
}

/// Clicks `path`'s row in the file tree once the tree lists it.
pub(super) fn open_in_tree(harness: &mut Harness<'_, ReviewApp>, path: &str) {
    wait_until(harness, &format!("{path} in the file tree"), |harness| {
        harness.query_by_role_and_label(Role::TreeItem, path).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, path).rect());
    harness.step();
}

/// Waits until the file view shows `path`'s diff ([`shows_file_view`]).
pub(super) fn wait_for_diff(harness: &mut Harness<'_, ReviewApp>, path: &str) {
    wait_until(harness, &format!("the diff of {path}"), |harness| shows_file_view(harness, path));
}

/// Moves the pointer down the first lines of `diff`, in each of `columns` (fractions of its width)
/// in turn, until a line's "Add comment" button shows: it exists only while its line is hovered.
pub(super) fn hover_until_add_comment(
    harness: &mut Harness<'_, ReviewApp>,
    diff: Rect,
    columns: &[f32],
) {
    for column in columns {
        let x = diff.left() + diff.width() * column;
        let mut y = diff.top() + 2.0;
        while y < diff.top() + 160.0 {
            if harness.query_by_label("Add comment").is_some() {
                return;
            }
            harness.hover_at(pos2(x, y));
            harness.step();
            y += 3.0;
        }
    }
    assert!(
        harness.query_by_label("Add comment").is_some(),
        "no comment button appeared while hovering the first lines"
    );
}

/// Whether a label contains `text`; any number of matches.
pub(super) fn has(harness: &Harness<'_, ReviewApp>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

/// Whether a label is `label`; any number of matches.
pub(super) fn shows(harness: &Harness<'_, ReviewApp>, label: &str) -> bool {
    harness.query_all_by_label(label).next().is_some()
}

/// The file view shows `path`'s diff. Its label alone does not tell: a card of the folder view
/// draws its code with the same diff view, so once the card's code is in, "Diff of {path}" is
/// there while no file is open. The two views never show in one frame.
pub(super) fn shows_file_view(harness: &Harness<'_, ReviewApp>, path: &str) -> bool {
    !shows_folder_view(harness) && harness.query_by_label(&format!("Diff of {path}")).is_some()
}

/// The folder view shows: its header, drawn in every frame it shows, ends the folder's trail at
/// "{folder} (current folder)".
pub(super) fn shows_folder_view(harness: &Harness<'_, ReviewApp>) -> bool {
    harness.query_all_by_label_contains(" (current folder)").next().is_some()
}

/// Advances simulated time; each frame also gives background work a moment of real time. Each
/// frame moves egui's clock on by the harness's step unless the test pinned the next frames' time
/// (`harness.input_mut().time`), which would hold the clock still.
pub(super) fn pump(harness: &mut Harness<'_, ReviewApp>, seconds: f64) {
    let _step = cv_testkit::step(format!("letting {seconds} s of simulated time pass"));
    let time = |harness: &Harness<'_, ReviewApp>| harness.ctx.input(|input| input.time);
    let (started, end) = (Instant::now(), time(harness) + seconds);
    while time(harness) < end {
        assert!(
            started.elapsed() < LIMIT,
            "{seconds} s of simulated time did not pass: egui's clock stands at {} s, the next frame's \
             time is pinned to {:?}",
            time(harness),
            harness.input().time
        );
        harness.step();
        cv_testkit::let_threads_run(Duration::from_millis(3));
    }
}

/// A frame with the window focused or not.
pub(super) fn set_focus(harness: &mut Harness<'_, ReviewApp>, is_focused: bool) {
    let _step = cv_testkit::step(format!("a frame with the window focused: {is_focused}"));
    let viewport = harness.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default();
    viewport.focused = Some(is_focused);
    harness.input_mut().focused = is_focused;
    harness.step();
}

/// The URLs the last frame asked the platform to open.
pub(super) fn opened_urls(harness: &Harness<'_, ReviewApp>) -> Vec<String> {
    harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
            _ => None,
        })
        .collect()
}

/// The stretch of a walkthrough in which a request count rests on GETs joining one in flight.
/// `review_core::http` shares identical GETs only while no write finished in the process since
/// the shared one started, and `cargo test` runs the walkthroughs on threads of one process: a
/// write a neighbour finishes inside the window ends the sharing there, and the page sends its own
/// request. The window tells the two cases apart; a write of the test's own host fails it, since
/// then the app ended the sharing itself.
pub(super) struct SharingWindow {
    epoch: u64,
    own_writes: usize,
}

impl SharingWindow {
    /// Opens it before the first request that others join starts; `own_writes`: the writes the
    /// test's host received so far.
    pub(super) fn open(own_writes: usize) -> Self {
        Self { epoch: review_core::http::write_epoch(), own_writes }
    }

    /// Closes it once the joins happened, with the host's writes by then: none may have come in
    /// meanwhile.
    pub(super) fn close(self, own_writes: usize) -> NeighbourWrites {
        assert_eq!(
            own_writes, self.own_writes,
            "the app wrote to its host inside the window, which ends the sharing the count rests on"
        );
        NeighbourWrites(review_core::http::write_epoch() - self.epoch)
    }
}

/// The writes other tests finished in the process while a [`SharingWindow`] was open.
pub(super) struct NeighbourWrites(u64);

impl NeighbourWrites {
    /// The request counts that hold when `joined` requests go out while every GET joins, of which
    /// `may_join` GETs join one request in flight under one key. Without a neighbour's write the
    /// count is exact. With one, each GET that did not join started a request of its own, which
    /// the later ones join unless another write finished: so every extra request takes a write of
    /// its own, and the count lies between `joined` and `joined` plus the fewer of the writes and
    /// `may_join`.
    fn allowed(&self, joined: usize, may_join: usize) -> RangeInclusive<usize> {
        let extra = usize::try_from(self.0).map_or(may_join, |writes| writes.min(may_join));
        joined..=joined + extra
    }

    /// Asserts that `counted` requests went out; `joined` and `may_join` as in [`Self::allowed`].
    pub(super) fn assert_count(&self, counted: usize, joined: usize, may_join: usize, what: &str) {
        let allowed = self.allowed(joined, may_join);
        assert!(
            allowed.contains(&counted),
            "{what}: {counted} requests, {} to {} expected; other tests finished {} writes inside \
             the window",
            allowed.start(),
            allowed.end(),
            self.0
        );
    }
}

/// The settings file the app saved below its data folders.
pub(super) fn saved_settings(harness: &Harness<'_, ReviewApp>) -> AppSettings {
    AppSettings::load(&harness.state().services.paths.settings_file()).expect("saved settings")
}

/// Whether an open pull request holds `path`'s diff in its cache.
pub(super) fn has_cached_diff(harness: &Harness<'_, ReviewApp>, path: &str) -> bool {
    harness.state().pull_requests.iter().any(|(_, page)| page.has_cached_diff(path))
}

#[cfg(test)]
mod tests {
    use super::{NeighbourWrites, SharingWindow};

    /// Without a neighbour's write the count is exact; each write lets one more GET miss its join,
    /// up to the GETs that join, and never fewer requests are sent.
    #[test]
    fn a_neighbours_write_allows_one_request_more_each_up_to_the_joins() {
        assert_eq!(NeighbourWrites(0).allowed(1, 2), 1..=1);
        assert_eq!(NeighbourWrites(1).allowed(1, 2), 1..=2);
        assert_eq!(NeighbourWrites(5).allowed(1, 2), 1..=3);
    }

    /// A write of the test's own app is never taken for a neighbour's.
    #[test]
    #[should_panic(expected = "the app wrote to its host inside the window")]
    fn a_write_to_the_own_host_inside_the_window_fails() {
        SharingWindow::open(0).close(1);
    }
}
