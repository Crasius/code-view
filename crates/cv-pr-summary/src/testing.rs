//! The summary's test support: a scene that shows the folder summary on its own with a fake diff
//! source that counts its loads, the harness around it, the wait until its cards are loaded and
//! colored, and the changed files the walkthroughs use. No network, no tokens.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use review_core::{ProviderKind, ReviewThread, ThreadAnchor};
use review_git::{ChangeKind, ChangedFile, DiffOptions, FileDiff};
use review_store::{AppSettings, DiffLayout, ThemePreference};

use super::{DiffSource, FolderSummary, MARGIN, SummaryEvent, SummaryInputs};
use cv_base::theme;
use cv_pr_diff::diff_view::DiffView;
use cv_pr_diff::testing::thread_on_old_line;
use cv_pr_rendered::preview::Loaded;
use cv_pr_threads::ThreadsUi;
use cv_runtime::background::Background;
use cv_runtime::load::Load;
use cv_shell::zoom::Zoom;
use cv_syntax::color_worker::observe_repaints;
use cv_syntax::highlight::LineSpans;
use cv_testkit::WAIT_LIMIT;
use cv_widgets::{focus, surfaces};

/// The window most walkthroughs show the summary in.
pub(super) const SIZE: [f32; 2] = [1180.0, 860.0];

/// `path` changed as `kind` with `counts` (added, deleted) lines, or not counted yet.
pub(super) fn file(path: &str, kind: ChangeKind, counts: Option<(u32, u32)>) -> ChangedFile {
    ChangedFile {
        additions: counts.map(|(added, _)| added),
        deletions: counts.map(|(_, deleted)| deleted),
        ..ChangedFile::sample(path, kind)
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
    /// Set: every diff load starts on this runtime and never finishes.
    pub hangs_on: Option<Background>,
    cache: HashMap<String, Arc<FileDiff>>,
}

impl DiffSource for FakeSource {
    fn cached(&mut self, file: &ChangedFile) -> Option<Arc<FileDiff>> {
        self.cache.get(&file.path).cloned()
    }

    fn start(&mut self, file: &ChangedFile) -> Option<Load<Arc<FileDiff>>> {
        self.started.push(file.path.clone());
        if let Some(background) = &self.hangs_on {
            return Some(Load::Loading(background.spawn(std::future::pending())));
        }
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

    fn spans(&mut self, _: &ChangedFile) -> Option<Arc<LineSpans>> {
        None
    }
}

pub(super) struct Scene {
    pub summary: FolderSummary,
    pub files: Arc<Vec<ChangedFile>>,
    folder: String,
    order: Vec<usize>,
    order_version: u64,
    pub source: FakeSource,
    pub thread_counts: HashMap<String, usize>,
    pub threads: Vec<ReviewThread>,
    pub threads_ui: ThreadsUi,
    pub events: Vec<SummaryEvent>,
    theme: ThemePreference,
    pub can_load: bool,
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
    pub fn open(&mut self, folder: &str) {
        self.folder = folder.to_owned();
        self.order = (0..self.files.len())
            .filter(|&index| {
                folder.is_empty() || self.files[index].path.starts_with(&format!("{folder}/"))
            })
            .collect();
        self.order_version += 1;
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
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
pub(super) fn shop_files() -> Vec<ChangedFile> {
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
/// thread), one frame per wake-up of the loads and the coloring. Panics naming the cards still
/// unloaded when no wake-up comes within [`WAIT_LIMIT`] or 400 frames do not settle the summary.
pub(super) fn settle(harness: &mut Harness<'_, Scene>) {
    let wakes = observe_repaints(&harness.ctx);
    for _ in 0..400 {
        harness.run_steps(1);
        let summary = &harness.state().summary;
        let unloaded = unloaded(harness.state());
        if unloaded.is_empty() && summary.loads.running() == 0 && !summary.bodies.is_coloring() {
            harness.run_steps(2);
            return;
        }
        // The fake source's loads are ready at once and finish on the next frame, which nothing
        // asks for.
        if summary.loads.running() > 0 {
            continue;
        }
        assert!(
            wakes.recv_timeout(WAIT_LIMIT).is_ok(),
            "no load or coloring woke the summary within {} s; still unloaded: {unloaded:?}",
            WAIT_LIMIT.as_secs()
        );
    }
    let scene = harness.state();
    panic!(
        "400 frames did not settle the summary: unloaded {:?}, {} loads running, coloring {}",
        unloaded(scene),
        scene.summary.loads.running(),
        scene.summary.bodies.is_coloring()
    );
}

/// The paths of the wanted cards whose diff (or, for a rendered card, whose versions) is not
/// loaded yet.
fn unloaded(scene: &Scene) -> Vec<&str> {
    let summary = &scene.summary;
    summary
        .loads
        .wanted()
        .iter()
        .map(|&card| &scene.files[summary.slots[card].file])
        .filter(|file| {
            // A rendered card loads its versions, not its lines.
            if scene.preview && cv_pr_rendered::preview::is_markdown(&file.path) {
                summary.rendered.needs_load(&file.path)
            } else {
                super::diffs::needs_load(summary, file)
            }
        })
        .map(|file| file.path.as_str())
        .collect()
}

pub(super) fn card_of(scene: &Scene, path: &str) -> usize {
    scene
        .summary
        .slots
        .iter()
        .position(|slot| scene.files[slot.file].path == path)
        .expect("a card for the path")
}

pub(super) fn key(harness: &mut Harness<'_, Scene>, key: Key) {
    harness.key_press(key);
    harness.run_steps(2);
}

/// Scrolls the card of `path` to the top of the view, `below_top` points into it, and settles.
pub(super) fn scroll_to_card(harness: &mut Harness<'_, Scene>, path: &str, below_top: f32) {
    let card = card_of(harness.state(), path);
    let top = harness.state().summary.index.top(card);
    harness.state_mut().summary.scroll_to = Some(top + MARGIN + below_top);
    settle(harness);
}

/// The diff view of the loaded card of `path`.
pub(super) fn view_of<'a>(scene: &'a Scene, path: &str) -> &'a DiffView {
    &scene.summary.bodies.get(path).expect("the card's diff is loaded").view
}

/// The row of the card of `path` showing head line `new_line` (old line `old_line` when there is
/// no head line).
pub(super) fn row_of(
    scene: &Scene,
    path: &str,
    old_line: Option<u32>,
    new_line: Option<u32>,
) -> usize {
    let body = scene.summary.bodies.get(path).expect("the card's diff is loaded");
    body.view.row_of(&body.diff, old_line, new_line)
}

/// The primary button going down (`pressed`) or up at `pos`.
pub(super) fn press(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// The thread `id` saying `body` on the old line `old_line` or the head line `new_line` of `path`.
pub(super) fn thread_at(
    id: &str,
    body: &str,
    path: &str,
    old_line: Option<u32>,
    new_line: Option<u32>,
) -> ReviewThread {
    let mut thread = thread_on_old_line(0, body);
    thread.id = id.into();
    thread.anchor =
        Some(ThreadAnchor { path: path.into(), old_path: None, old_line, new_line, range: None });
    thread
}

/// The cards the last pass showed.
pub(super) fn cards_shown(summary: &FolderSummary) -> usize {
    summary.index.overlapping(summary.viewport.top(), summary.viewport.bottom()).len()
}

/// 10,000 files in 100 folders, a quarter each added and deleted.
pub(super) fn ten_thousand_files() -> Vec<ChangedFile> {
    (0..10_000)
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
        .collect()
}
