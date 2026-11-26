//! The folder summary on its own, with a fake diff source that counts its loads: the root and a
//! folder, added and deleted files, a collapsed card, the large-file placeholder and the keyboard's
//! View; that only the cards near the view load; and a 10,000-file pull request with over 200,000
//! changed lines that costs what is on screen. No network, no tokens.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase, ViewportId, pos2, vec2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{FileCategory, ProviderKind, ReviewThread};
use review_git::{ChangeKind, ChangedFile, DiffOptions, FileDiff};
use review_store::{AppSettings, DiffLayout, ThemePreference};

use super::bodies::MAX_BODIES;
use super::loads::{MAX_RUNNING, MAX_WANTED};
use super::{DiffSource, FolderSummary, SummaryEvent, SummaryInputs};
use cv_base::theme;
use cv_pr_rendered::preview::Loaded;
use cv_pr_threads::ThreadsUi;
use cv_runtime::load::Load;
use cv_shell::zoom::Zoom;
use cv_syntax::color_worker::observe_repaints;
use cv_testkit::frame_bench::{Bench, PATTERNS, SCALES};
use cv_widgets::{focus, surfaces};

pub(super) fn file(path: &str, kind: ChangeKind, counts: Option<(u32, u32)>) -> ChangedFile {
    ChangedFile {
        path: path.into(),
        old_path: None,
        kind,
        old_oid: "1".repeat(40),
        new_oid: "2".repeat(40),
        additions: counts.map(|(added, _)| added),
        deletions: counts.map(|(_, deleted)| deleted),
        is_binary: false,
        category: FileCategory::Code,
        whitespace_only: false,
    }
}

/// A Rust-like line, so the syntax colors have something to color.
fn source_line(index: usize, variant: usize) -> String {
    let indent = "    ".repeat(1 + index % 3);
    match (index + variant) % 6 {
        0 => format!(
            "{indent}let total_{index} = items.iter().map(|item| item.price * {variant}).sum::<u64>();"
        ),
        1 => format!("{indent}if let Some(entry) = self.cart.get(&key_{index}) {{"),
        2 => format!("{indent}}}"),
        3 => format!("{indent}// Applies the discount before the tax, as the invoice lists it."),
        4 => format!(
            "{indent}return Err(CheckoutError::Invalid(\"line {index} is out of range\".into()));"
        ),
        _ => {
            format!("{indent}self.totals.insert(key_{index}.clone(), Amount::from_cents({index}));")
        }
    }
}

/// The unified diff the line counts promise: whole files for added and deleted ones, hunks of three
/// deleted and five added lines with three unchanged ones around them for the rest.
fn diff_for(file: &ChangedFile) -> FileDiff {
    let (added, deleted) =
        (file.additions.unwrap_or(0) as usize, file.deletions.unwrap_or(0) as usize);
    let mut text = String::new();
    match file.kind {
        ChangeKind::Added => {
            writeln!(text, "@@ -0,0 +1,{added} @@").unwrap();
            for index in 0..added {
                writeln!(text, "+{}", source_line(index, 1)).unwrap();
            }
        }
        ChangeKind::Deleted => {
            writeln!(text, "@@ -1,{deleted} +0,0 @@").unwrap();
            for index in 0..deleted {
                writeln!(text, "-{}", source_line(index, 0)).unwrap();
            }
        }
        _ => {
            let (mut added_left, mut deleted_left, mut hunk) = (added, deleted, 0);
            while added_left + deleted_left > 0 {
                let (minus, plus) = (deleted_left.min(3), added_left.min(5));
                let start = 1 + hunk * 40;
                writeln!(text, "@@ -{start},{} +{start},{} @@", minus + 6, plus + 6).unwrap();
                for offset in 0..3 {
                    writeln!(text, " {}", source_line(start + offset, 0)).unwrap();
                }
                for offset in 0..minus {
                    writeln!(text, "-{}", source_line(start + 3 + offset, 0)).unwrap();
                }
                for offset in 0..plus {
                    writeln!(text, "+{}", source_line(start + 3 + offset, 1)).unwrap();
                }
                for offset in 0..3 {
                    writeln!(text, " {}", source_line(start + 10 + offset, 0)).unwrap();
                }
                (added_left, deleted_left, hunk) =
                    (added_left - plus, deleted_left - minus, hunk + 1);
            }
        }
    }
    FileDiff::parse_unified(text.as_bytes())
}

/// A Markdown file's versions: `sections` sections of a heading and a paragraph, the new version
/// with every third paragraph edited.
pub(super) fn markdown_versions(sections: usize) -> (String, String) {
    let (mut old, mut new) = (String::new(), String::new());
    for index in 0..sections {
        let paragraph = format!("Section {index} explains how the cart totals its lines.");
        writeln!(
            old,
            "## Part {index}

{paragraph}
"
        )
        .unwrap();
        let edited = if index % 3 == 0 { format!("{paragraph} Edited.") } else { paragraph };
        writeln!(
            new,
            "## Part {index}

{edited}
"
        )
        .unwrap();
    }
    (old, new)
}

/// Diffs made up on request, counted like the mirror's requests.
#[derive(Default)]
pub(super) struct FakeSource {
    pub started: Vec<String>,
    /// The Markdown files whose versions were asked for, for their rendered cards.
    pub rendered: Vec<String>,
    /// The Markdown versions a rendered card loads, forty sections every third edited unless set.
    pub versions: Option<(String, String)>,
    cache: HashMap<String, Arc<FileDiff>>,
}

impl DiffSource for FakeSource {
    fn cached(&mut self, file: &ChangedFile) -> Option<Arc<FileDiff>> {
        self.cache.get(&file.path).cloned()
    }

    fn start(&mut self, file: &ChangedFile) -> Option<Load<Arc<FileDiff>>> {
        self.started.push(file.path.clone());
        let diff = if file.is_binary {
            FileDiff::parse_unified(b"Binary files a/x and b/x differ\n")
        } else {
            diff_for(file)
        };
        Some(Load::Ready(Arc::new(diff)))
    }

    fn store(&mut self, file: &ChangedFile, diff: Arc<FileDiff>) {
        self.cache.insert(file.path.clone(), diff);
    }

    fn start_rendered(&mut self, file: &ChangedFile) -> Option<Load<Arc<Loaded>>> {
        self.rendered.push(file.path.clone());
        let (old, new) = self.versions.clone().unwrap_or_else(|| markdown_versions(40));
        Some(Load::Ready(Arc::new(Loaded::markdown(Some(&old), Some(&new)))))
    }
}

pub(super) struct Scene {
    pub summary: FolderSummary,
    pub files: Arc<Vec<ChangedFile>>,
    folder: String,
    order: Vec<usize>,
    order_version: u64,
    pub source: FakeSource,
    thread_counts: HashMap<String, usize>,
    pub threads: Vec<ReviewThread>,
    pub threads_ui: ThreadsUi,
    pub events: Vec<SummaryEvent>,
    theme: ThemePreference,
    can_load: bool,
    /// The card whose new-comment card is recorded per frame that reached the screen.
    pub recording: Option<String>,
    /// Per recorded frame: where the comment card was, and whether lines showed as picked.
    pub frames: Vec<(Option<egui::Rect>, bool)>,
    /// The user's diff layout (inline unless a test picks another) and side-by-side share.
    pub layout: DiffLayout,
    pub split: f32,
    /// The code size comes from here; the app's zoom handler changes it as in the app.
    pub settings: AppSettings,
    /// The header's Preview: Markdown cards show rendered.
    pub preview: bool,
    /// The unchanged lines the comparison keeps around each change (the file view's choice).
    pub context: u32,
    zoom: Zoom,
}

impl Scene {
    pub fn new(files: Vec<ChangedFile>, theme: ThemePreference) -> Self {
        let mut scene = Self {
            summary: FolderSummary::default(),
            files: Arc::new(files),
            folder: String::new(),
            order: Vec::new(),
            order_version: 0,
            source: FakeSource::default(),
            thread_counts: HashMap::new(),
            threads: Vec::new(),
            threads_ui: ThreadsUi::for_tests(ProviderKind::GitHub),
            events: Vec::new(),
            theme,
            can_load: true,
            recording: None,
            frames: Vec::new(),
            layout: DiffLayout::Inline,
            split: 0.5,
            settings: AppSettings::default(),
            preview: false,
            context: 3,
            zoom: Zoom::default(),
        };
        scene.open("");
        scene
    }

    /// The files below `folder` in list order (the lists here are in tree order already).
    fn open(&mut self, folder: &str) {
        self.folder = folder.to_owned();
        self.order = (0..self.files.len())
            .filter(|&index| {
                folder.is_empty() || self.files[index].path.starts_with(&format!("{folder}/"))
            })
            .collect();
        self.order_version += 1;
    }

    fn show(&mut self, ui: &mut egui::Ui) {
        let threads: Vec<&ReviewThread> = self.threads.iter().collect();
        let inputs = SummaryInputs {
            files: &self.files,
            order: &self.order,
            order_version: self.order_version,
            folder: &self.folder,
            compare: ("base", "head", DiffOptions::context(self.context)),
            font_size: self.settings.code_font_size,
            thread_counts: &self.thread_counts,
            threads: &threads,
            can_comment: true,
            can_comment_outside_hunks: false,
            can_load: self.can_load,
            is_filtered: false,
            layout: self.layout,
            show_threads: true,
            preview: self.preview,
            platform: ProviderKind::GitHub,
            rendered_width: self.settings.rendered_width,
        };
        let event =
            self.summary.show(ui, &inputs, &mut self.source, &mut self.threads_ui, &mut self.split);
        if let Some(path) = &self.recording
            && !ui.ctx().will_discard()
        {
            let card = self
                .summary
                .bodies
                .get(path)
                .map(|body| (body.view.composer_card(), body.view.marks_lines()));
            self.frames.push(card.unwrap_or((None, false)));
        }
        if let Some(event) = event {
            match &event {
                SummaryEvent::OpenFolder(folder) => {
                    let folder = folder.clone();
                    self.open(&folder);
                }
                SummaryEvent::SwitchLayout(layout) => self.layout = *layout,
                SummaryEvent::SwitchPreview(is_on) => self.preview = *is_on,
                _ => {}
            }
            self.events.push(event);
        }
    }
}

pub(super) fn harness(scene: Scene, size: [f32; 2]) -> Harness<'static, Scene> {
    let builder =
        Harness::builder().with_size(size).with_step_dt(1.0 / 60.0).with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, false);
        });
    builder.build_ui_state(
        |ui, scene: &mut Scene| {
            focus::track_modality(ui.ctx());
            theme::activate(ui.ctx(), scene.theme);
            surfaces::begin_frame(ui.ctx());
            scene.zoom.handle_input(ui.ctx(), &mut scene.settings);
            scene.show(ui);
            scene.zoom.show_overlay(ui.ctx(), &mut scene.settings);
            focus::paint_for_focused_widget(ui.ctx());
        },
        scene,
    )
}

/// A small pull request in tree order: folders first, then the root's files.
fn shop_files() -> Vec<ChangedFile> {
    use ChangeKind::{Added, Deleted, Modified, Renamed};
    let binary = ChangedFile { is_binary: true, ..file("assets/logo.png", Modified, None) };
    let renamed = ChangedFile {
        old_path: Some("docs/old-guide.md".into()),
        ..file("docs/guide.md", Renamed, Some((0, 0)))
    };
    vec![
        binary,
        file("data/fixtures.json", Added, Some((30_000, 0))),
        renamed,
        file("src/app/cart.rs", Modified, Some((14, 6))),
        file("src/app/checkout.rs", Modified, Some((8, 3))),
        file("src/app/legacy_payment.rs", Deleted, Some((0, 9))),
        file("src/app/promo.rs", Added, Some((12, 0))),
        file("src/ui/theme.rs", Modified, Some((4, 4))),
        file("README.md", Modified, Some((3, 1))),
    ]
}

/// Frames until every loaded card on screen has its syntax colors (the coloring runs on its own
/// thread), one frame per wake-up of the loads and the coloring, for at most 30 seconds.
pub(super) fn settle(harness: &mut Harness<'_, Scene>) {
    let wakes = observe_repaints(&harness.ctx);
    let deadline = Instant::now() + Duration::from_secs(30);
    for _ in 0..400 {
        harness.run_steps(1);
        let scene = harness.state();
        let summary = &scene.summary;
        let unloaded = summary
            .loads
            .wanted()
            .iter()
            .filter(|&&card| {
                let file = &scene.files[summary.slots[card].file];
                // A rendered card loads its versions, not its lines.
                if scene.preview && cv_pr_rendered::preview::is_markdown(&file.path) {
                    summary.rendered.needs_load(&file.path)
                } else {
                    super::diffs::needs_load(summary, file)
                }
            })
            .count();
        if unloaded + summary.loads.running() == 0 && !summary.bodies.is_coloring() {
            break;
        }
        // The fake source's loads are ready at once and finish on the next frame, which nothing
        // asks for.
        if summary.loads.running() > 0 {
            continue;
        }
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        if wakes.recv_timeout(left).is_err() {
            break;
        }
    }
    harness.run_steps(2);
}

pub(super) fn card_of(scene: &Scene, path: &str) -> usize {
    scene
        .summary
        .slots
        .iter()
        .position(|slot| scene.files[slot.file].path == path)
        .expect("a card for the path")
}

fn key(harness: &mut Harness<'_, Scene>, key: Key) {
    harness.key_press(key);
    harness.run_steps(2);
}

#[test]
fn folder_summary_walkthrough() {
    let mut scene = Scene::new(shop_files(), ThemePreference::Light);
    scene.thread_counts.insert("src/app/checkout.rs".into(), 2);
    let mut harness = harness(scene, [1180.0, 860.0]);
    settle(&mut harness);
    assert!(harness.query_by_label("9 changed files").is_some());
    assert!(harness.query_by_label("View src/app/cart.rs").is_some());
    assert!(
        harness.query_by_label("Show here data/fixtures.json").is_some(),
        "the large file waits for a click"
    );
    assert!(
        harness.query_by_label("Open file assets/logo.png").is_some(),
        "a binary file leads to its file view"
    );
    assert!(harness.query_by_label("Open file docs/guide.md").is_some(), "a pure rename too");
    let started = &harness.state().source.started;
    assert!(
        !started.iter().any(|path| path == "data/fixtures.json"
            || path == "assets/logo.png"
            || path == "docs/guide.md"),
        "placeholders load nothing: {started:?}"
    );

    harness.state_mut().open("src/app");
    settle(&mut harness);
    assert!(harness.query_by_label("4 changed files").is_some());

    // n moves to the cards one by one; the third and fourth are the deleted and added files.
    for _ in 0..3 {
        key(&mut harness, Key::N);
    }
    settle(&mut harness);
    assert_eq!(harness.state().summary.current, Some(2));

    key(&mut harness, Key::Enter);
    let index =
        harness.state().files.iter().position(|file| file.path == "src/app/legacy_payment.rs");
    assert!(
        matches!(harness.state().events.last(), Some(SummaryEvent::View(opened)) if Some(*opened) == index),
        "Enter opens the current card"
    );

    key(&mut harness, Key::Home);
    harness.get_by_label("Collapse src/app/cart.rs").click();
    settle(&mut harness);
    assert!(harness.query_by_label("Expand src/app/cart.rs").is_some());

    harness.get_by_label("View src/app/checkout.rs").click();
    harness.run_steps(2);
    let checkout = harness.state().files.iter().position(|file| file.path == "src/app/checkout.rs");
    assert!(
        matches!(harness.state().events.last(), Some(SummaryEvent::View(opened)) if Some(*opened) == checkout),
        "View opens the file view"
    );

    harness.get_by_label("All files").click();
    harness.run_steps(2);
    harness.state_mut().open("data");
    settle(&mut harness);
    harness.get_by_label("Show here data/fixtures.json").click();
    settle(&mut harness);
    assert!(
        harness.state().source.started.iter().any(|path| path == "data/fixtures.json"),
        "Show here loads it"
    );
}

#[test]
fn collapse_lasts_for_the_session_and_is_exactly_the_header() {
    let mut harness = harness(Scene::new(shop_files(), ThemePreference::Light), [1180.0, 860.0]);
    settle(&mut harness);
    harness.get_by_label("Collapse src/app/cart.rs").click();
    harness.run_steps(3);
    let card = card_of(harness.state(), "src/app/cart.rs");
    let summary = &harness.state().summary;
    let geometry = summary.geometry.expect("laid out");
    assert_eq!(
        summary.index.bottom(card) - summary.index.top(card),
        geometry.header + geometry.gap
    );
    // Another folder and back: still collapsed.
    harness.state_mut().open("src");
    harness.run_steps(3);
    harness.state_mut().open("");
    harness.run_steps(3);
    assert!(harness.query_by_label("Expand src/app/cart.rs").is_some());
}

/// Loads stay with the cards on screen and one screen around them, nothing loads while the
/// window is in the background or the line counts are not there yet, and the request budget for
/// opening a summary is the cards near the view, not the folder.
#[test]
fn only_the_cards_near_the_view_load() {
    let files: Vec<ChangedFile> = (0..400)
        .map(|index| {
            file(
                &format!("src/m{:02}/f{index:04}.rs", index / 20),
                ChangeKind::Modified,
                Some((12, 8)),
            )
        })
        .collect();
    let mut scene = Scene::new(files, ThemePreference::Light);
    scene.can_load = false;
    let mut harness = harness(scene, [1100.0, 800.0]);
    harness.run_steps(10);
    assert!(harness.state().source.started.is_empty(), "nothing loads before the line counts");

    harness.state_mut().can_load = true;
    if let Some(viewport) = harness.input_mut().viewports.get_mut(&ViewportId::ROOT) {
        viewport.focused = Some(false);
    }
    harness.run_steps(10);
    assert!(
        harness.state().source.started.is_empty(),
        "nothing loads while the window is in the background"
    );

    if let Some(viewport) = harness.input_mut().viewports.get_mut(&ViewportId::ROOT) {
        viewport.focused = Some(true);
    }
    harness.run_steps(30);
    let assert_near_view = |harness: &Harness<'_, Scene>, started: &[String]| {
        let summary = &harness.state().summary;
        let (top, bottom) = (summary.viewport.top(), summary.viewport.bottom());
        let height = summary.viewport.height();
        for path in started {
            let card = card_of(harness.state(), path);
            let (card_top, card_bottom) = (summary.index.top(card), summary.index.bottom(card));
            assert!(
                card_bottom > top - height && card_top < bottom + height,
                "{path} at {card_top}..{card_bottom} is beyond one screen around {top}..{bottom}"
            );
        }
    };
    let first = harness.state().source.started.clone();
    let on_screen = harness.state().summary.stats().cards_shown;
    assert!(first.len() >= on_screen, "every card on screen loads: {} of {on_screen}", first.len());
    assert!(
        first.len() <= MAX_WANTED && first.len() < 20,
        "only the cards near the view: {}",
        first.len()
    );
    assert_near_view(&harness, &first);
    println!(
        "opening the summary: {} of 400 diffs loaded, {on_screen} cards on screen",
        first.len()
    );

    // A jump to the end loads the end, once the view has settled there.
    key(&mut harness, Key::End);
    harness.run_steps(30);
    let started = harness.state().source.started.clone();
    let after_jump = &started[first.len()..];
    assert_ne!(after_jump, [] as [String; 0]);
    assert_near_view(&harness, after_jump);
    assert!(
        started.len() < 40,
        "a jump loads the screen it lands on, not the way there: {}",
        started.len()
    );
    println!("after a jump to the end: {} more", after_jump.len());
}

/// 10,000 files in 100 folders, a quarter each added and deleted, and one 20,000-line file.
fn ten_thousand_files() -> Vec<ChangedFile> {
    let mut files: Vec<ChangedFile> = (0..10_000)
        .map(|index| {
            let kind = [
                ChangeKind::Modified,
                ChangeKind::Modified,
                ChangeKind::Added,
                ChangeKind::Deleted,
            ][index % 4];
            let counts = match kind {
                ChangeKind::Added => (10 + (index % 30) as u32, 0),
                ChangeKind::Deleted => (0, 8 + (index % 20) as u32),
                _ => (8 + (index % 25) as u32, 4 + (index % 15) as u32),
            };
            file(
                &format!(
                    "src/area_{}/module_{}/file_{index:05}.rs",
                    index / 1000,
                    index / 100 % 10
                ),
                kind,
                Some(counts),
            )
        })
        .collect();
    files[3].additions = Some(20_000);
    files[3].kind = ChangeKind::Added;
    files
}

/// 10,000 files with about 250,000 changed lines (and one 20,000-line file): every frame looks at
/// the cards and rows on screen only, a fling through all of it loads a handful of diffs, and the
/// bodies kept never pass their cap.
#[test]
fn ten_thousand_files_cost_what_is_on_screen() {
    let files = ten_thousand_files();
    let total: u64 = files
        .iter()
        .map(|file| u64::from(file.additions.unwrap_or(0) + file.deletions.unwrap_or(0)))
        .sum();
    assert!(total > 200_000, "{total} changed lines");

    let mut harness = harness(Scene::new(files, ThemePreference::Light), [1280.0, 900.0]);
    harness.run_steps(40);
    let (mut most_cards, mut most_rows, mut most_bodies) = (0, 0, 0);
    let mut check = |harness: &Harness<'_, Scene>| {
        let summary = &harness.state().summary;
        let stats = summary.stats();
        most_cards = most_cards.max(stats.cards_shown);
        most_rows = most_rows.max(stats.rows_painted);
        most_bodies = most_bodies.max(summary.bodies_kept());
        assert!(stats.wanted <= MAX_WANTED);
        assert!(summary.bodies_kept() <= MAX_BODIES, "{} bodies kept", summary.bodies_kept());
        assert!(summary.loads.running() <= MAX_RUNNING);
    };
    // The 20,000-line card fills the screen: it paints the rows in view, not its 20,000.
    let inside = harness.state().summary.index.top(3) + 5_000.0;
    harness.state_mut().summary.scroll_to = Some(inside);
    harness.run_steps(30);
    check(&harness);
    let tall = harness.state().summary.stats();
    assert!(tall.cards_shown == 1 && tall.rows_painted <= 50, "inside the tall card: {tall:?}");

    let before = harness.state().source.started.len();
    let pointer = pos2(640.0, 500.0);
    harness.input_mut().events.push(Event::PointerMoved(pointer));
    let wheel = |delta: f32, phase: TouchPhase| Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, delta),
        phase,
        modifiers: Modifiers::NONE,
        source: MouseWheelSource::Wheel,
    };
    harness.input_mut().events.push(wheel(0.0, TouchPhase::Start));
    for _ in 0..240 {
        harness.input_mut().events.push(wheel(-6_000.0, TouchPhase::Move));
        harness.run_steps(1);
        check(&harness);
    }
    harness.input_mut().events.push(wheel(0.0, TouchPhase::End));
    harness.run_steps(1);
    let during = harness.state().source.started.len() - before;
    let travelled = harness.state().summary.viewport.top();
    assert!(travelled > 200_000.0, "the fling went far: {travelled}");
    assert!(during <= 2 * MAX_RUNNING, "a fling through {travelled} points loaded {during} diffs");

    harness.run_steps(40);
    check(&harness);
    let settled = harness.state().source.started.len() - before - during;
    assert!(settled > 0 && settled < 20, "once it settles, the screen loads: {settled}");
    assert!(
        most_cards < 30 && most_rows < 120,
        "per frame at most {most_cards} cards and {most_rows} rows"
    );
    println!(
        "10k files, {total} lines: fling of {travelled:.0} pt loaded {during} diffs, {settled} after settling; per frame <= {most_cards} cards, {most_rows} rows; bodies <= {most_bodies}"
    );
}

/// The summary of the 10,000-file pull request at 4K, inline and side by side, scrolled at the
/// standard patterns.
#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_summary() {
    let files: Vec<ChangedFile> = (0..10_000)
        .map(|index| {
            file(
                &format!(
                    "src/area_{}/module_{}/file_{index:05}.rs",
                    index / 1000,
                    index / 100 % 10
                ),
                ChangeKind::Modified,
                Some((8 + (index % 25) as u32, 4 + (index % 15) as u32)),
            )
        })
        .collect();
    let mut reports = Vec::new();
    for (layout, layout_name) in
        [(DiffLayout::Inline, "inline"), (DiffLayout::SideBySide, "side by side")]
    {
        for pixels_per_point in SCALES {
            let mut scene = Scene::new(files.clone(), ThemePreference::Light);
            scene.layout = layout;
            let mut bench =
                Bench::new(pixels_per_point, scene, |ui, scene: &mut Scene| scene.show(ui));
            let pointer = bench.ctx.content_rect().center();
            bench.warm_up(pointer);
            for (pattern, speed) in PATTERNS {
                let name = format!(
                    "summary 10k files {layout_name} @{:.0}% {pattern}",
                    pixels_per_point * 100.0
                );
                reports.push(bench.scroll_best(&name, pointer, vec2(0.0, -speed)));
            }
            println!(
                "loads {layout_name} at {:.0}%: {}",
                pixels_per_point * 100.0,
                bench.state.source.started.len()
            );
        }
    }
    for report in &reports {
        report.print();
    }
    for report in &reports {
        report.assert_within_budget();
    }
}
