//! The walkthrough kit: how a test starts the whole app, waits for it, clicks and reads it.
//!
//! The waits differ in when they return. [`wait_until`] runs four more frames after its condition
//! held, so what the frame that met it started has landed. [`wait_until_unsettled`] returns on the
//! frame the condition held. [`wait_for`] returns a value, also on that frame. The other ways to
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
use std::time::{Duration, Instant};

use egui::{CursorIcon, PointerButton, Pos2, Role};
use egui_kittest::kittest::{AccessKitNode, NodeT, Queryable};
use egui_kittest::{Harness, HarnessBuilder};
use review_core::Connection;
use review_store::{AppPaths, ConnectionStore, DiffLayout};

use super::{ReviewApp, startup};
use cv_services::connection::providers::ProviderFactory;
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
    let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
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
    factory: ProviderFactory,
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

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    harness.run_steps(4);
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

/// Scrolls the labelled widget into view, then clicks it like a pointer would (see
/// [`click_when_ready`]).
pub(super) fn press(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    click_when_ready(harness, label, None);
}

/// [`press`] for the button labelled `label`: for a submit button that is enabled only with text,
/// and where another widget carries the same label (a composer's text area is named after its
/// submit button).
pub(super) fn press_button(harness: &mut Harness<'_, ReviewApp>, label: &str) {
    click_when_ready(harness, label, Some(Role::Button));
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

/// Scrolls the widget labelled `label` into view, waits until it is enabled with its centre inside
/// the window (scrolling again where late content pushed it out), then clicks it at its place in
/// that frame ([`Clicks::click_at`]). A fixed number of frames after the scroll proves none of
/// that: the click would land on a disabled button, or past the window's edge where nothing takes
/// it, and the test would wait for a request that never comes.
fn click_when_ready(harness: &mut Harness<'_, ReviewApp>, label: &str, role: Option<Role>) {
    wait_until(harness, label, |harness| labelled(harness, label, role).is_some());
    labelled(harness, label, role).expect("just found").scroll_to_me();
    settle(harness);
    let window = harness.ctx.content_rect();
    let rect = wait_for(
        harness,
        &format!("{label} to take a click"),
        |harness| {
            let node = labelled(harness, label, role)?;
            let rect = node.rect();
            let is_enabled = !node.accesskit_node().is_disabled();
            (is_enabled && window.contains(rect.center())).then_some(rect)
        },
        |harness| {
            if let Some(node) = labelled(harness, label, role)
                && !window.contains(node.rect().center())
            {
                node.scroll_to_me();
            }
        },
    );
    harness.click_at(rect);
    harness.step();
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

/// Whether a label contains `text`; any number of matches.
pub(super) fn has(harness: &Harness<'_, ReviewApp>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
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

/// Whether an open pull request holds `path`'s diff in its cache.
pub(super) fn has_cached_diff(harness: &Harness<'_, ReviewApp>, path: &str) -> bool {
    harness.state().pull_requests.iter().any(|(_, page)| page.has_cached_diff(path))
}
