//! The rendered Markdown diff: rows of each layout, blocks lined up side by side whatever
//! scrolls them, gaps as tall as the block across, changed words filled, nothing moving under the
//! pointer, and a 5,000-line README that lays out only the rows in view.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;

use cv_testkit::{Clicks, poll_until};
use egui::accesskit::Role;
use egui::epaint::text::ByteRangeExt as _;
use egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, Rect, TouchPhase,
    Vec2, pos2, vec2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use review_core::{DiffSide, ProviderKind, ReviewThread, ThreadAnchor, ThreadStatus};
use review_git::FULL_FILE_CONTEXT;
use review_store::{DiffLayout, RenderedWidth, ThemePreference};

mod past_end;
mod placed_scroll;
mod width;

use super::heights::outward;
use super::model::{Look, Mode, Row};
use super::rows::{begin_frame, build};
use super::{MarkdownDiff, MarkdownModel, ROW_PAD, RenderedEvent, RenderedImages, RenderedInputs};
use cv_base::theme::{self, Palette};
use cv_pr_diff::diff_view::ChangePosition;
use cv_pr_threads::{ThreadsUi, samples};
use cv_testkit::frame_bench::{self, Bench, Report, SCALES};

/// What the last frame painted, for tests that look at the geometry.
#[derive(Debug, Default, Clone)]
pub(super) struct Probe {
    pub cells: Vec<ProbeCell>,
}

/// One lane of a row: a block (`unit`) or the hatched gap where its side has none.
#[derive(Debug, Clone, Copy)]
pub(super) struct ProbeCell {
    pub row: usize,
    pub side: DiffSide,
    pub unit: Option<usize>,
    pub rect: Rect,
}

const LONG_ADDED: &str = "A new paragraph only the new version has. It runs long on purpose, so that it wraps over \
several lines in its column and the old side needs a gap of the same height to keep the blocks below lined up.";
const EDITED_OLD: &str = "The quick brown fox jumps over the lazy dog.";
const EDITED_NEW: &str = "The quick red fox jumps over the sleepy dog.";
const REMOVED: &str = "Paragraph 12 is gone from the new version.";

fn same(index: usize) -> String {
    format!("Paragraph {index} reads the same in both versions.")
}

/// Forty paragraphs under a heading: one added (long), one removed, one edited.
fn sample() -> (String, String) {
    let mut old = vec!["# Release notes".to_owned()];
    let mut new = old.clone();
    for index in 0..40 {
        match index {
            5 => {
                new.push(LONG_ADDED.to_owned());
                old.push(same(index));
                new.push(same(index));
            }
            12 => old.push(REMOVED.to_owned()),
            20 => {
                old.push(EDITED_OLD.to_owned());
                new.push(EDITED_NEW.to_owned());
            }
            _ => {
                old.push(same(index));
                new.push(same(index));
            }
        }
    }
    (old.join("\n\n") + "\n", new.join("\n\n") + "\n")
}

/// What the view is shown: the two versions and the choices its inputs read.
struct Doc {
    old: Option<String>,
    new: Option<String>,
    layout: DiffLayout,
    context: u32,
    can_comment: bool,
    images: RenderedImages,
    threads: Vec<ReviewThread>,
    /// "Show comments in the diff".
    show_threads: bool,
    /// "Scroll past the end", which the strip's menu switches.
    past_end: bool,
}

impl Doc {
    fn key(&self) -> u64 {
        egui::epaint::util::hash((&self.old, &self.new))
    }

    /// The versions parsed and lined up, as the load builds them.
    fn model(&self) -> MarkdownModel {
        let (old, new) = (self.old.as_deref(), self.new.as_deref());
        MarkdownModel::new(self.key(), old, new, ProviderKind::GitHub, &self.images)
    }

    fn inputs<'a>(
        &'a self,
        model: &'a Arc<MarkdownModel>,
        threads: &'a [&'a ReviewThread],
    ) -> RenderedInputs<'a> {
        RenderedInputs {
            model,
            platform: ProviderKind::GitHub,
            layout: self.layout,
            context: self.context,
            scale: 1.0,
            can_comment: self.can_comment,
            threads,
            show_threads: self.show_threads,
            past_end: self.past_end,
        }
    }
}

struct Scene {
    doc: Doc,
    /// The model of `doc`'s versions, built again when they change.
    model: Option<Arc<MarkdownModel>>,
    view: MarkdownDiff,
    split: f32,
    theme: ThemePreference,
    events: Vec<RenderedEvent>,
    threads_ui: ThreadsUi,
    /// The reading column or full width.
    width: RenderedWidth,
}

impl Scene {
    fn new(old: &str, new: &str, layout: DiffLayout, context: u32) -> Self {
        Self {
            doc: Doc {
                old: Some(old.to_owned()),
                new: Some(new.to_owned()),
                layout,
                context,
                can_comment: true,
                images: RenderedImages::default(),
                threads: Vec::new(),
                show_threads: true,
                past_end: true,
            },
            model: None,
            view: MarkdownDiff::default(),
            split: 0.5,
            theme: ThemePreference::Light,
            events: Vec::new(),
            threads_ui: ThreadsUi::for_tests(ProviderKind::GitHub),
            width: RenderedWidth::default(),
        }
    }
}

fn show(ui: &mut egui::Ui, scene: &mut Scene) {
    theme::activate(ui.ctx(), scene.theme);
    let key = scene.doc.key();
    if scene.model.as_ref().is_none_or(|model| model.key != key) {
        scene.model = Some(Arc::new(scene.doc.model()));
    }
    let threads: Vec<&ReviewThread> = scene.doc.threads.iter().collect();
    let model = scene.model.as_ref().expect("built above");
    let inputs = scene.doc.inputs(model, &threads);
    if let Some(event) =
        scene.view.show(ui, &inputs, &mut scene.split, &mut scene.width, &mut scene.threads_ui)
    {
        if let RenderedEvent::SwitchPastEnd(is_on) = event {
            scene.doc.past_end = is_on;
        }
        scene.events.push(event);
    }
}

/// Frames until the view settles once its rows are measured.
const MAX_STEPS: u64 = 1_000;

fn harness(scene: Scene, size: [f32; 2]) -> Harness<'static, Scene> {
    Harness::builder()
        .with_size(size)
        .with_max_steps(MAX_STEPS)
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(show, scene)
}

/// Scrolls by `delta` (a wheel, a trackpad or Shift+wheel sideways) with the pointer at `pointer`.
fn wheel(harness: &mut Harness<'_, Scene>, pointer: Pos2, delta: Vec2) {
    let event = |delta: Vec2, phase: TouchPhase| Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta,
        phase,
        modifiers: Modifiers::NONE,
        source: MouseWheelSource::Wheel,
    };
    harness.hover_at(pointer);
    harness.input_mut().events.push(event(Vec2::ZERO, TouchPhase::Start));
    harness.step();
    harness.input_mut().events.push(event(delta, TouchPhase::Move));
    harness.step();
    harness.input_mut().events.push(event(Vec2::ZERO, TouchPhase::End));
    harness.run();
}

/// The primary button going down (`pressed`) or up at `pos`.
fn press(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// Hovers `at` for a frame, then clicks there.
fn hover_and_click(harness: &mut Harness<'_, Scene>, at: Pos2) {
    harness.hover_at(at);
    harness.step();
    harness.click_point(at, PointerButton::Primary);
    harness.run();
}

fn tops(harness: &Harness<'_, Scene>, text: &str) -> Vec<f32> {
    harness.query_all_by_label(text).map(|node| node.rect().top()).collect()
}

/// Every unchanged paragraph on screen has its old and its new version at the same height, and
/// the edited paragraph sits next to its old version.
fn check_aligned(harness: &Harness<'_, Scene>, when: &str) {
    let mut compared = 0;
    for index in 0..40 {
        let found = tops(harness, &same(index));
        if found.len() == 2 {
            assert!((found[0] - found[1]).abs() < 0.5, "{when}: paragraph {index} at {found:?}");
            compared += 1;
        }
    }
    if let (Some(old), Some(new)) =
        (tops(harness, EDITED_OLD).first(), tops(harness, EDITED_NEW).first())
    {
        assert!((old - new).abs() < 0.5, "{when}: the edited paragraph at {old} and {new}");
    }
    assert!(compared >= 3, "{when}: only {compared} pairs on screen");
}

#[test]
fn blocks_line_up_whatever_scrolls_them() {
    let (old, new) = sample();
    let mut harness =
        harness(Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT), [900.0, 600.0]);
    harness.run();
    check_aligned(&harness, "at the top");
    let mut last_top = 0.0;
    for (pointer, what) in [
        (pos2(200.0, 300.0), "the wheel over Before"),
        (pos2(650.0, 300.0), "the wheel over After"),
    ] {
        wheel(&mut harness, pointer, vec2(0.0, -400.0));
        let top = harness.state().view.viewport.0.top();
        assert!(top > last_top + 100.0, "{what} scrolled from {last_top} to {top}");
        last_top = top;
        check_aligned(&harness, what);
    }
    harness.key_press(Key::PageDown);
    harness.run();
    assert!(harness.state().view.viewport.0.top() > last_top, "Page Down scrolled");
    check_aligned(&harness, "after Page Down");
    harness.key_press(Key::End);
    harness.run();
    check_aligned(&harness, "at the end");
}

/// Regression: a reload of the same versions (moved commits) hands in a new model with the same
/// key; the frame painted that one while the diff kept the one it held.
#[test]
fn a_reload_of_the_same_versions_paints_the_held_model() {
    let (old, new) = sample();
    let scene = Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT);
    let (held, reloaded) = (Arc::new(scene.doc.model()), Arc::new(scene.doc.model()));
    let (ctx, mut view) = (egui::Context::default(), MarkdownDiff::default());
    let (first, ..) = begin_frame(&mut view, &ctx, &scene.doc.inputs(&held, &[]));
    assert!(Arc::ptr_eq(&first, &held), "the first load is taken in");
    let (painted, ..) = begin_frame(&mut view, &ctx, &scene.doc.inputs(&reloaded, &[]));
    assert!(Arc::ptr_eq(&painted, &held), "the held model is painted, not the reloaded one");
}

#[test]
fn a_missing_block_leaves_a_gap_as_tall_as_the_block_across() {
    let (old, new) = sample();
    let mut harness =
        harness(Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT), [900.0, 700.0]);
    harness.run();
    let probe = harness.state().view.probe.clone();
    let gap = probe
        .cells
        .iter()
        .find(|cell| cell.side == DiffSide::Old && cell.unit.is_none())
        .expect("a gap beside the added paragraph");
    let block = probe
        .cells
        .iter()
        .find(|cell| cell.row == gap.row && cell.side == DiffSide::New)
        .expect("the added paragraph");
    assert_eq!(gap.rect.y_range(), block.rect.y_range(), "the gap spans the added paragraph's row");
    let label = harness.get_by_label(LONG_ADDED).rect();
    assert!(label.height() > 40.0, "the added paragraph wraps: {label:?}");
    let room = block.rect.height() - label.height();
    assert!(
        (room - 2.0 * ROW_PAD).abs() < 1.0,
        "the row is the paragraph and its tint's room: {} vs {}",
        block.rect.height(),
        label.height()
    );
    // The paragraph after it starts below the gap on both sides.
    let below = tops(&harness, &same(5));
    assert!(below.iter().all(|top| *top >= gap.rect.bottom()), "{below:?} below {:?}", gap.rect);

    // The removed paragraph leaves the gap on the new side.
    wheel(&mut harness, pos2(450.0, 300.0), vec2(0.0, -300.0));
    let probe = harness.state().view.probe.clone();
    let removed = harness.get_by_label(REMOVED).rect();
    let gap = probe
        .cells
        .iter()
        .find(|cell| cell.side == DiffSide::New && cell.unit.is_none())
        .expect("a gap beside the removed paragraph");
    assert!(
        gap.rect
            .contains_rect(removed.translate(vec2(gap.rect.left() - removed.left() + 30.0, 0.0))),
        "{gap:?} spans {removed:?}"
    );
}

fn model_of(old: &str, new: &str) -> MarkdownModel {
    Scene::new(old, new, DiffLayout::SideBySide, FULL_FILE_CONTEXT).doc.model()
}

fn looks(rows: &[Row]) -> Vec<[Option<(DiffSide, Look)>; 2]> {
    rows.iter().map(|row| row.cells().map(|cell| cell.map(|cell| (cell.side, cell.look)))).collect()
}

#[test]
#[expect(
    clippy::single_range_in_vec_init,
    reason = "one change: a list of one row range, not a range of indices"
)]
fn each_layout_marks_added_removed_and_edited_blocks() {
    let old = "# Title\n\nKept.\n\nDropped block here.\n\nThe quick brown fox.\n";
    let new = "# Title\n\nKept.\n\nThe quick red fox.\n\nA new block.\n";
    let model = model_of(old, new);
    let none = HashSet::new();
    let added = Some((DiffSide::New, Look::Added));
    let removed = Some((DiffSide::Old, Look::Removed));
    let plain = |side| Some((side, Look::Plain));

    let columns = build(&model, Mode::Columns, FULL_FILE_CONTEXT, &none, &[]);
    assert_eq!(
        looks(&columns.rows),
        [
            [plain(DiffSide::Old), plain(DiffSide::New)],
            [plain(DiffSide::Old), plain(DiffSide::New)],
            [removed, None],
            [removed, added],
            [None, added],
        ]
    );
    assert_eq!(columns.hunks, [2..5], "consecutive changed blocks are one change");

    let unified = build(&model, Mode::Unified, FULL_FILE_CONTEXT, &none, &[]);
    assert_eq!(
        looks(&unified.rows),
        [
            [plain(DiffSide::New), None],
            [plain(DiffSide::New), None],
            [removed, None],
            [removed, None],
            [added, None],
            [added, None]
        ],
        "the old version of the edited block right above the new one"
    );

    let new_only = build(&model, Mode::NewOnly, FULL_FILE_CONTEXT, &none, &[]);
    assert!(
        matches!(new_only.rows[2], Row::Removal { .. }),
        "the removed block is a wedge: {:?}",
        new_only.rows
    );
    assert_eq!(
        looks(&new_only.rows[3..]),
        [[Some((DiffSide::New, Look::Focus)), None], [Some((DiffSide::New, Look::Focus)), None]]
    );
    assert!(new_only.overview.is_focus && new_only.overview.removals == [2]);

    let whole = Doc { old: None, ..Scene::new(old, new, DiffLayout::SideBySide, 3).doc }.model();
    let whole_rows = build(&whole, Mode::Whole(DiffSide::New), 3, &none, &[]);
    assert!(
        looks(&whole_rows.rows).iter().all(|cells| cells[0] == plain(DiffSide::New)),
        "an added file shows whole, unmarked"
    );
    assert_eq!(whole_rows.hunks, [0..whole_rows.rows.len()]);
}

/// An image file replaced under the same name changes no text, yet its block pairs as edited:
/// the old image left, the new one right, both marked.
#[test]
fn a_replaced_image_shows_side_by_side_as_a_change() {
    let text = "# Screens\n\n![Login](./img/login.png)\n\n![Logo](./img/logo.png)\n";
    let mut scene = Scene::new(text, text, DiffLayout::SideBySide, FULL_FILE_CONTEXT);
    let none = HashSet::new();
    let unchanged = scene.doc.model();
    assert!(
        build(&unchanged, Mode::Columns, FULL_FILE_CONTEXT, &none, &[]).hunks.is_empty(),
        "same text, same images"
    );

    scene.doc.images.content = [
        [("./img/login.png".to_owned(), "aaa".to_owned())].into_iter().collect(),
        [("./img/login.png".to_owned(), "bbb".to_owned())].into_iter().collect(),
    ];
    let model = scene.doc.model();
    let rows = build(&model, Mode::Columns, FULL_FILE_CONTEXT, &none, &[]);
    let plain = |side| Some((side, Look::Plain));
    assert_eq!(
        looks(&rows.rows),
        [
            [plain(DiffSide::Old), plain(DiffSide::New)],
            [Some((DiffSide::Old, Look::Removed)), Some((DiffSide::New, Look::Added))],
            [plain(DiffSide::Old), plain(DiffSide::New)],
        ],
        "only the block of the replaced image changed"
    );
}

#[test]
fn unchanged_runs_fold_around_the_changes() {
    let (old, new) = sample();
    let model = model_of(&old, &new);
    let rows = build(&model, Mode::Columns, 2, &HashSet::new(), &[]).rows;
    let folds: Vec<std::ops::Range<usize>> = rows
        .iter()
        .filter_map(|row| match row {
            Row::Folded { pairs } => Some(pairs.clone()),
            _ => None,
        })
        .collect();
    // Pairs: heading, paragraphs 0–4, the added one (6), 5–11, removed 12, 13–19, edited 20, 21–39.
    assert_eq!(
        folds,
        [0..4, 9..12, 17..20, 25..42],
        "two blocks kept on each side of a change, none at the ends"
    );
    let opened = build(&model, Mode::Columns, 2, &HashSet::from([9]), &[]).rows;
    assert_eq!(opened.len(), rows.len() + 2, "the opened fold shows its three blocks");
}

#[test]
fn a_fold_opens_on_click_and_keeps_the_place() {
    let (old, new) = sample();
    let mut harness = harness(Scene::new(&old, &new, DiffLayout::SideBySide, 2), [900.0, 1400.0]);
    harness.run();
    assert!(tops(&harness, &same(0)).is_empty(), "the fold holds paragraph 0");
    let fold = harness
        .query_all_by_label_contains("unchanged blocks")
        .find(|node| node.accesskit_node().role() == Role::Button)
        .expect("a fold bar");
    let label = fold.accesskit_node().label().unwrap_or_default();
    assert!(label.starts_with("Show 4 unchanged blocks (lines "), "{label}");
    // On its label: the middle of the bar is the handle between Before and After.
    let rect = fold.rect();
    hover_and_click(&mut harness, pos2(rect.left() + 60.0, rect.center().y));
    assert_eq!(tops(&harness, &same(0)).len(), 2, "the fold opened on paragraph 0");
    check_aligned(&harness, "after opening a fold");
}

#[test]
fn edited_words_are_filled_in_their_side_color() {
    let (old, new) = sample();
    let mut harness = harness(Scene::new(&old, &new, DiffLayout::SideBySide, 2), [900.0, 1400.0]);
    harness.run();
    let palette = Palette::LIGHT;
    let marked = |text: &str, color: egui::Color32| {
        let mut jobs = Vec::new();
        for clipped in &harness.output().shapes {
            walk(&clipped.shape, &mut |galley: &egui::Galley| {
                if galley.text() == text {
                    jobs.push(Arc::clone(&galley.job));
                }
            });
        }
        jobs.iter()
            .flat_map(|job| {
                job.sections
                    .iter()
                    .filter(|section| section.format.background == color)
                    .map(|section| job.text[section.byte_range.as_usize()].to_owned())
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(marked(EDITED_OLD, palette.diff_deleted_word).concat(), "brownlazy");
    assert_eq!(marked(EDITED_NEW, palette.diff_added_word).concat(), "redsleepy");
}

fn walk(shape: &egui::Shape, visit: &mut impl FnMut(&egui::Galley)) {
    match shape {
        egui::Shape::Text(text) => visit(&text.galley),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, visit)),
        _ => {}
    }
}

const CODE_OLD: &str =
    "Intro.\n\n```rust\nfn main() { let answer = 41; }\n```\n\nAfter the code.\n";
const CODE_NEW: &str =
    "Intro.\n\n```rust\nfn main() { let answer = 42; }\n```\n\nAfter the code.\n";

/// Hovering a block shows its comment button in the gutter and moves nothing, frame after frame;
/// neither does hovering the button or a code block's copy button. The button comments on the
/// block's source lines.
#[test]
fn hovering_a_block_moves_nothing() {
    let mut harness = harness(
        Scene::new(CODE_OLD, CODE_NEW, DiffLayout::SideBySide, FULL_FILE_CONTEXT),
        [900.0, 400.0],
    );
    harness.run();
    let code_line = "fn main() { let answer = 42; }";
    let rects = |harness: &Harness<'_, Scene>| {
        let mut rects: Vec<Rect> =
            harness.query_all_by_label("After the code.").map(|node| node.rect()).collect();
        rects.extend(harness.query_all_by_label(code_line).map(|node| node.rect()));
        rects
    };
    let resting = rects(&harness);
    assert_eq!(resting.len(), 3);
    harness.hover_at(harness.get_by_label(code_line).rect().center());
    for frame in 0..8 {
        harness.step();
        assert_eq!(rects(&harness), resting, "frame {frame} over the code");
    }
    let button = harness.get_by_role_and_label(Role::Button, "Comment on lines 3\u{2013}5").rect();
    harness.hover_at(button.center());
    harness.run_steps(3);
    assert_eq!(rects(&harness), resting, "over the comment button");
    let copy = harness
        .query_all_by_role_and_label(Role::Button, "Copy code")
        .map(|node| node.rect())
        .max_by(|a, b| a.left().total_cmp(&b.left()))
        .expect("the new code block's copy button");
    harness.hover_at(copy.center());
    harness.run_steps(3);
    assert_eq!(rects(&harness), resting, "over the copy button");
    harness.hover_at(button.center());
    harness.run_steps(2);
    harness.click_at(
        harness.get_by_role_and_label(Role::Button, "Comment on lines 3\u{2013}5").rect(),
    );
    harness.run();
    assert_eq!(
        harness.state().events,
        [RenderedEvent::Comment { side: DiffSide::New, lines: 3..6 }]
    );
}

#[test]
fn n_and_p_step_through_the_changes() {
    let (old, new) = sample();
    let mut harness =
        harness(Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT), [900.0, 400.0]);
    harness.run();
    let key = harness.state().doc.key();
    let at = |current| Some(ChangePosition { current, total: 3 });
    assert_eq!(harness.state().view.change_position(key), at(None));
    let area = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 400.0));
    for (index, block) in (0..3).zip([LONG_ADDED, REMOVED, EDITED_NEW]) {
        harness.key_press(Key::N);
        harness.run();
        assert_eq!(harness.state().view.change_position(key), at(Some(index)));
        assert!(
            harness.query_all_by_label(block).any(|node| area.contains(node.rect().center())),
            "change {index}, {block:?}, is in view"
        );
    }
    harness.key_press(Key::P);
    harness.run();
    assert_eq!(harness.state().view.change_position(key), at(Some(1)));
}

#[test]
fn the_place_carries_over_between_the_text_and_the_rendered_diff() {
    let (old, new) = sample();
    let line = new.lines().position(|line| line == same(16)).expect("paragraph 16") as u32 + 1;
    let mut harness =
        harness(Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT), [900.0, 500.0]);
    harness.run();
    harness.state_mut().view.reveal_line(Some((DiffSide::New, line)));
    harness.run();
    assert_eq!(
        harness.state().view.top_line(),
        Some((DiffSide::New, line)),
        "the text diff's line opens at the top"
    );
    let top = harness.state().view.viewport.0.top();
    assert!(top > 0.0, "the line is below the top of the document");
    let paragraph = tops(&harness, &same(16));
    assert_eq!(paragraph.len(), 2, "paragraph 16 is on screen on both sides");

    // Folded away, the line's fold opens first.
    let mut harness =
        self::harness(Scene::new(&old, &new, DiffLayout::SideBySide, 1), [900.0, 500.0]);
    harness.run();
    assert!(tops(&harness, &same(16)).is_empty(), "paragraph 16 starts folded away");
    harness.state_mut().view.reveal_line(Some((DiffSide::New, line)));
    harness.run();
    assert_eq!(tops(&harness, &same(16)).len(), 2, "paragraph 16 is on screen on both sides");
}

/// New only: the wedge of a removed block is a button that shows it side by side, in place.
#[test]
fn a_removal_wedge_opens_side_by_side() {
    let (old, new) = sample();
    let mut harness =
        harness(Scene::new(&old, &new, DiffLayout::NewOnly, FULL_FILE_CONTEXT), [1100.0, 900.0]);
    harness.run();
    assert!(tops(&harness, REMOVED).is_empty(), "New only leaves the removed block out");
    harness.click_at(harness.get_by_label("Show the 1 removed block side by side").rect());
    harness.run();
    assert_eq!(harness.state().events, [RenderedEvent::ShowSideBySide]);
    harness.state_mut().doc.layout = DiffLayout::SideBySide;
    harness.run();
    assert_eq!(tops(&harness, REMOVED).len(), 1, "the removed block shows where the wedge was");
}

/// The paragraph reading `text` sits at the same height on both sides.
fn assert_lined_up(harness: &Harness<'_, Scene>, text: &str, what: &str) {
    let found = tops(harness, text);
    assert!(found.len() == 2 && (found[0] - found[1]).abs() < 0.5, "{what}: {text:?} at {found:?}");
}

/// A changed Mermaid diagram, as a fence or as an Azure DevOps wiki page's `::: mermaid`
/// container, pairs as an edited block: the old drawing left, the new one right, both marked; the
/// paragraph after them stays lined up once both have their size, and Show source turns each side
/// into its source.
#[test]
fn a_changed_diagram_pairs_draws_side_by_side_and_switches_to_source() {
    const FENCE_OLD: &str =
        "# Flow\n\n```mermaid\nflowchart LR\n  A[Request] --> B[Review]\n```\n\nClosing words.\n";
    const FENCE_NEW: &str = "# Flow\n\n```mermaid\nflowchart LR\n  A[Request] --> B[Review] --> C[Merge]\n```\n\nClosing words.\n";
    const CONTAINER_OLD: &str =
        "# Flow\n\n::: mermaid\ngraph LR;\n  A[Request] --> B[Review]\n:::\n\nClosing words.\n";
    const CONTAINER_NEW: &str = "# Flow\n\n:::mermaid\ngraph LR;\n  A[Request] --> B[Review] --> C[Merge]\n:::\n\nClosing words.\n";
    let none = HashSet::new();
    let diagrams = |harness: &Harness<'_, Scene>| {
        harness
            .query_all_by_role_and_label(Role::Image, "Mermaid diagram")
            .map(|node| node.rect())
            .collect::<Vec<Rect>>()
    };
    for (case, old, new) in
        [("fence", FENCE_OLD, FENCE_NEW), ("Azure DevOps container", CONTAINER_OLD, CONTAINER_NEW)]
    {
        let rows = build(&model_of(old, new), Mode::Columns, FULL_FILE_CONTEXT, &none, &[]).rows;
        let edited = [Some((DiffSide::Old, Look::Removed)), Some((DiffSide::New, Look::Added))];
        assert!(looks(&rows).contains(&edited), "{case}: the diagram block is the change");

        let mut harness = harness(
            Scene::new(old, new, DiffLayout::SideBySide, FULL_FILE_CONTEXT),
            [1100.0, 520.0],
        );
        // The diagrams are drawn on a worker thread, so frames alone do not bring them.
        poll_until(&format!("{case}: both versions drawn"), || {
            harness.step();
            diagrams(&harness).len() >= 2
        });
        harness.run_steps(4);
        let drawn = diagrams(&harness);
        assert!((drawn[0].top() - drawn[1].top()).abs() < 0.5, "{case}: side by side at {drawn:?}");
        assert_lined_up(&harness, "Closing words.", &format!("{case}: below the diagrams"));
        if case != "fence" {
            assert_eq!(
                harness.query_all_by_label_contains(":::").count(),
                0,
                "{case}: the markers are not text"
            );
        }

        poll_until(&format!("{case}: the diagrams' buttons"), || {
            harness.step();
            harness.query_all_by_role_and_label(Role::Button, "Show source").count() >= 2
        });
        for _ in 0..2 {
            let switch = harness
                .get_all_by_role_and_label(Role::Button, "Show source")
                .next()
                .expect("a diagram to switch")
                .rect();
            harness.click_at(switch);
            harness.run_steps(4);
        }
        assert!(diagrams(&harness).is_empty(), "{case}: no side draws its diagram");
        assert_eq!(
            harness.query_all_by_label_contains("A[Request] --> B[Review]").count(),
            2,
            "{case}: both sides show their source"
        );
        assert_lined_up(&harness, "Closing words.", &format!("{case}: below the sources"));
    }
}

const PATH: &str = "README.md";
const ON_NEW: &str = "A comment on the new version of paragraph 8.";
const ON_OLD: &str = "A comment on the removed paragraph.";
const IN_FOLD: &str = "A comment deep in the unchanged part.";

/// 1-based line of `needle` in `text`.
fn line_in(text: &str, needle: &str) -> u32 {
    text.lines()
        .position(|line| line == needle)
        .unwrap_or_else(|| panic!("{needle:?} in the sample")) as u32
        + 1
}

fn thread_at(id: &str, side: DiffSide, line: u32, body: &str) -> ReviewThread {
    let anchor = match side {
        DiffSide::New => samples::lines(PATH, line, line),
        DiffSide::Old => ThreadAnchor {
            path: PATH.into(),
            old_path: None,
            old_line: Some(line),
            new_line: None,
            range: None,
        },
    };
    samples::thread(
        id,
        ThreadStatus::Active,
        Some(anchor),
        vec![samples::comment(&format!("{id}-c"), &samples::jonas(), 5, body)],
    )
}

/// The sample with a thread on paragraph 8 of the new version and one on the removed paragraph.
fn commented(layout: DiffLayout, context: u32) -> Scene {
    let (old, new) = sample();
    let mut scene = Scene::new(&old, &new, layout, context);
    scene.doc.threads = vec![
        thread_at("on-new", DiffSide::New, line_in(&new, &same(8)), ON_NEW),
        thread_at("on-old", DiffSide::Old, line_in(&old, REMOVED), ON_OLD),
    ];
    scene
}

/// The indexes of the `rows` that `is_marked` picks, as the overview strip numbers them.
fn positions(rows: &[Row], is_marked: impl Fn(&Row) -> bool) -> Vec<u32> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| is_marked(row))
        .map(|(index, _)| index as u32)
        .collect()
}

/// The overview strip marks every comment card, and a fold holding comments instead of the cards
/// it hides.
#[test]
fn the_overview_marks_each_card_and_each_fold_holding_comments() {
    let none = HashSet::new();
    let built = |scene: &Scene| {
        let threads: Vec<&ReviewThread> = scene.doc.threads.iter().collect();
        build(&scene.doc.model(), Mode::Columns, scene.doc.context, &none, &threads)
    };
    let shown = built(&commented(DiffLayout::SideBySide, FULL_FILE_CONTEXT));
    let cards = positions(&shown.rows, |row| matches!(row, Row::Thread(_)));
    assert_eq!(cards.len(), 2, "a card for each thread");
    assert_eq!(shown.overview.threads, cards, "the strip marks both cards");

    let (old, new) = sample();
    let mut scene = Scene::new(&old, &new, DiffLayout::SideBySide, 2);
    scene.doc.threads = vec![thread_at("folded", DiffSide::New, line_in(&new, &same(30)), IN_FOLD)];
    let folded = built(&scene);
    let fold =
        positions(&folded.rows, |row| matches!(row, Row::Folded { pairs } if pairs.contains(&32)));
    assert_eq!(fold.len(), 1, "one fold holds paragraph 30");
    assert_eq!(folded.overview.threads, fold, "the strip marks that fold, not the others");
}

/// Where the comment `body` shows, if it does.
fn card(harness: &Harness<'_, Scene>, body: &str) -> Option<Rect> {
    harness.query_all_by_label_contains(body).next().map(|node| node.rect())
}

/// The blocks reading `text`, left to right.
fn blocks(harness: &Harness<'_, Scene>, text: &str) -> Vec<Rect> {
    let mut rects: Vec<Rect> = harness.query_all_by_label(text).map(|node| node.rect()).collect();
    rects.sort_by(|a, b| a.left().total_cmp(&b.left()));
    rects
}

fn assert_between(card: Option<Rect>, block: Rect, next: Rect, what: &str) {
    let card = card.unwrap_or_else(|| panic!("{what}: the card shows"));
    assert!(
        card.top() > block.bottom() && card.bottom() < next.top(),
        "{what}: the card {card:?} sits between its block {block:?} and the next {next:?}"
    );
}

/// A thread's card sits under the block of its line: the new version's block for the new side,
/// the old version's for removed lines, in its column side by side.
#[test]
fn threads_sit_under_the_block_of_their_line_side_by_side() {
    let mut harness =
        harness(commented(DiffLayout::SideBySide, FULL_FILE_CONTEXT), [1000.0, 1300.0]);
    harness.run();
    let (paragraph, next) = (blocks(&harness, &same(8)), blocks(&harness, &same(9)));
    assert_between(card(&harness, ON_NEW), paragraph[1], next[1], "side by side, new side");
    assert!(
        card(&harness, ON_NEW).is_some_and(|card| card.left() >= paragraph[1].left() - 1.0),
        "in the After column"
    );
    let removed = blocks(&harness, REMOVED)[0];
    assert_between(
        card(&harness, ON_OLD),
        removed,
        blocks(&harness, &same(13))[0],
        "side by side, old side",
    );
    assert!(
        card(&harness, ON_OLD).is_some_and(|card| card.right() < paragraph[1].left()),
        "in the Before column"
    );
}

/// In one column a card sits under the block of its line too. New only leaves the removed block
/// out, so its card goes under the next block and says where the comment is; an added file shows
/// its cards under its blocks.
#[test]
fn threads_sit_under_the_block_of_their_line_in_one_column() {
    let mut harness =
        self::harness(commented(DiffLayout::Inline, FULL_FILE_CONTEXT), [800.0, 1500.0]);
    harness.run();
    assert_between(
        card(&harness, ON_NEW),
        blocks(&harness, &same(8))[0],
        blocks(&harness, &same(9))[0],
        "inline, new side",
    );
    assert_between(
        card(&harness, ON_OLD),
        blocks(&harness, REMOVED)[0],
        blocks(&harness, &same(13))[0],
        "inline, old side",
    );

    let mut harness =
        self::harness(commented(DiffLayout::NewOnly, FULL_FILE_CONTEXT), [800.0, 1500.0]);
    harness.run();
    assert_between(
        card(&harness, ON_NEW),
        blocks(&harness, &same(8))[0],
        blocks(&harness, &same(9))[0],
        "new only, new side",
    );
    assert!(blocks(&harness, REMOVED).is_empty(), "New only leaves the removed block out");
    let (old, _) = sample();
    let note = card(
        &harness,
        &format!("On line {} of the old version, removed here", line_in(&old, REMOVED)),
    )
    .expect("the note says where the comment is");
    assert_between(
        card(&harness, ON_OLD),
        blocks(&harness, &same(13))[0],
        blocks(&harness, &same(14))[0],
        "new only, old side",
    );
    assert!(
        card(&harness, ON_OLD).is_some_and(|card| note.bottom() <= card.top()),
        "the note heads the card"
    );

    let (_, new) = sample();
    let mut added = Scene::new("", &new, DiffLayout::SideBySide, 3);
    added.doc.old = None;
    added.doc.threads = vec![thread_at("on-new", DiffSide::New, line_in(&new, &same(8)), ON_NEW)];
    let mut harness = self::harness(added, [800.0, 1500.0]);
    harness.run();
    assert_between(
        card(&harness, ON_NEW),
        blocks(&harness, &same(8))[0],
        blocks(&harness, &same(9))[0],
        "added file",
    );
}

/// A comment on a line the rendered text does not show (here a blank line) sits under the next
/// block, says which line it is on and opens the text diff that shows it.
#[test]
fn a_comment_on_an_unshown_line_says_so_and_opens_the_text_diff() {
    let (old, new) = sample();
    let mut scene = Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT);
    let body = "Why the blank line here?";
    scene.doc.threads = vec![thread_at("blank", DiffSide::New, 2, body)];
    let mut harness = harness(scene, [1000.0, 900.0]);
    harness.run();
    let note =
        card(&harness, "On line 2, which the rendered text does not show").expect("the note");
    assert_between(
        Some(note),
        blocks(&harness, &same(0))[1],
        blocks(&harness, &same(1))[1],
        "the note under the next block",
    );
    assert!(
        card(&harness, body).is_some_and(|card| note.bottom() <= card.top()),
        "the note heads the card"
    );
    let note_button = harness
        .query_all_by_label_contains("Show in text diff")
        .find(|node| node.accesskit_node().role() == Role::Button)
        .expect("the note's button")
        .rect();
    harness.click_at(note_button);
    harness.run();
    assert_eq!(
        harness.state().events,
        [RenderedEvent::ShowThreadSource { thread_id: "blank".into() }]
    );
}

/// A fold holding comments counts them on its bar; opened, it shows the first of them.
#[test]
fn a_fold_counts_its_comments_and_opens_to_them() {
    let (old, new) = sample();
    let mut scene = Scene::new(&old, &new, DiffLayout::SideBySide, 2);
    scene.doc.threads = vec![thread_at("folded", DiffSide::New, line_in(&new, &same(30)), IN_FOLD)];
    let mut harness = harness(scene, [1000.0, 1100.0]);
    harness.run();
    assert!(card(&harness, IN_FOLD).is_none(), "folded away");
    let bar = harness
        .query_all_by_label_contains("unchanged blocks")
        .find(|node| {
            node.accesskit_node().role() == Role::Button
                && node
                    .accesskit_node()
                    .label()
                    .is_some_and(|label| label.ends_with(" and 1 comment"))
        })
        .expect("the fold names its comment");
    let rect = bar.rect();
    hover_and_click(&mut harness, pos2(rect.left() + 60.0, rect.center().y));
    let shown = card(&harness, IN_FOLD).expect("the comment shows");
    assert!(shown.top() >= 0.0 && shown.bottom() <= 1100.0, "and is in view: {shown:?}");
    assert_between(
        Some(shown),
        blocks(&harness, &same(30))[1],
        blocks(&harness, &same(31))[1],
        "under its block",
    );
}

/// While comments are hidden, a block's count stands for its threads: a click shows them there
/// and hides them again. Switching comments on shows every card, and off again hides the ones
/// shown one by one.
#[test]
fn hidden_comments_show_one_by_one_from_their_count() {
    let mut scene = commented(DiffLayout::SideBySide, FULL_FILE_CONTEXT);
    scene.doc.show_threads = false;
    let mut harness = harness(scene, [1000.0, 1300.0]);
    harness.run();
    assert!(
        card(&harness, ON_NEW).is_none() && card(&harness, ON_OLD).is_none(),
        "both cards are hidden"
    );
    let hidden = "1 comment hidden here, click to show";
    assert_eq!(
        harness.query_all_by_role_and_label(Role::Button, hidden).count(),
        2,
        "a count on each commented block"
    );
    let after_count = harness
        .query_all_by_role_and_label(Role::Button, hidden)
        .max_by(|a, b| a.rect().left().total_cmp(&b.rect().left()))
        .expect("the After column's count")
        .rect();
    harness.click_at(after_count);
    harness.run();
    assert!(card(&harness, ON_NEW).is_some(), "the clicked block's card shows");
    assert!(card(&harness, ON_OLD).is_none(), "the other stays hidden");
    harness.click_at(
        harness.get_by_role_and_label(Role::Button, "1 comment shown here, click to hide").rect(),
    );
    harness.run();
    assert!(card(&harness, ON_NEW).is_none(), "a second click hides it again");

    harness.state_mut().doc.show_threads = true;
    harness.run();
    assert!(
        card(&harness, ON_NEW).is_some() && card(&harness, ON_OLD).is_some(),
        "switched on, every card shows"
    );
    assert!(harness.query_by_label_contains("comment hidden here").is_none(), "without counts");
    harness.state_mut().doc.show_threads = false;
    harness.run();
    assert!(
        card(&harness, ON_NEW).is_none() && card(&harness, ON_OLD).is_none(),
        "switched off again, all are hidden"
    );
}

/// A jump to a thread (the files tab, the thread list) opens its fold and shows it while
/// comments are hidden; the others stay hidden.
#[test]
fn jumping_to_a_hidden_comment_opens_its_fold_and_shows_it() {
    let (old, new) = sample();
    let mut scene = Scene::new(&old, &new, DiffLayout::SideBySide, 2);
    scene.doc.threads = vec![
        thread_at("folded", DiffSide::New, line_in(&new, &same(30)), IN_FOLD),
        thread_at("on-old", DiffSide::Old, line_in(&old, REMOVED), ON_OLD),
    ];
    scene.doc.show_threads = false;
    let mut harness = harness(scene, [1000.0, 900.0]);
    harness.run();
    harness.state_mut().view.reveal_thread("folded");
    harness.run();
    let shown = card(&harness, IN_FOLD).expect("the thread jumped to shows");
    assert!(shown.top() >= 0.0 && shown.bottom() <= 900.0, "in view: {shown:?}");
    assert!(card(&harness, ON_OLD).is_none(), "the others stay hidden");
}

/// Hiding and showing the comments keeps the blocks in view where they are, with cards above them.
#[test]
fn switching_comments_keeps_the_view_in_place() {
    let (old, new) = sample();
    let mut scene = Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT);
    let (first, second) = ("A first comment above the view.", "A second comment above the view.");
    scene.doc.threads = vec![
        thread_at("first", DiffSide::New, line_in(&new, &same(1)), first),
        thread_at("second", DiffSide::Old, line_in(&old, &same(2)), second),
    ];
    let mut harness = harness(scene, [1000.0, 500.0]);
    harness.run();
    assert!(card(&harness, first).is_some(), "the cards show at the top");
    wheel(&mut harness, pos2(500.0, 250.0), vec2(0.0, -900.0));
    assert!(
        card(&harness, first).is_none() && card(&harness, second).is_none(),
        "the cards are above the view"
    );
    let watched = (10..40)
        .find(|index| tops(&harness, &same(*index)).first().is_some_and(|top| *top > 150.0))
        .expect("a paragraph in view");
    let before = tops(&harness, &same(watched))[0];
    for show in [false, true] {
        harness.state_mut().doc.show_threads = show;
        harness.run();
        let after = tops(&harness, &same(watched));
        assert!(
            after.first().is_some_and(|after| (after - before).abs() < 0.5),
            "comments {show}: paragraph {watched} moved from {before} to {after:?}"
        );
    }
}

/// A README of about 5,000 lines: sections of a heading, paragraphs, a list, code and a table,
/// every seventh edited, every eleventh with an added paragraph, every thirteenth without a list item.
fn long_readme() -> (String, String) {
    let (mut old, mut new) = (String::new(), String::new());
    for section in 0..220 {
        let heading = format!("## Section {section}\n\n");
        let paragraph = format!(
            "Section {section} explains how the module handles its configuration, which defaults apply and where to change them when the deployment needs it.\n\n"
        );
        let edited = format!(
            "Section {section} explains how the service handles its settings, which defaults apply and where to override them when the deployment needs it.\n\n"
        );
        let list: Vec<String> = (0..5)
            .map(|item| format!("- Item {item} of section {section} with a short note\n"))
            .collect();
        let code = format!(
            "```rust\nfn section_{section}() -> u32 {{\n    let total = {section} * 2;\n    total + 1\n}}\n```\n\n"
        );
        let table = format!(
            "| Key | Value |\n| --- | --- |\n| name | section-{section} |\n| size | {section} |\n\n"
        );
        let closing = format!(
            "Closing words of section {section}, with a [link](https://example.com/{section}) and `inline code`.\n\n"
        );
        old.push_str(&heading);
        new.push_str(&heading);
        old.push_str(&paragraph);
        new.push_str(if section % 7 == 0 { &edited } else { &paragraph });
        if section % 11 == 0 {
            write!(new, "An added paragraph in section {section}.\n\n").expect("write to a String");
        }
        old.push_str(&list.concat());
        let kept: Vec<&String> = list
            .iter()
            .enumerate()
            .filter(|(item, _)| section % 13 != 0 || *item != 2)
            .map(|(_, item)| item)
            .collect();
        new.push_str(&kept.into_iter().cloned().collect::<String>());
        old.push('\n');
        new.push('\n');
        for text in [&code, &table, &closing] {
            old.push_str(text);
            new.push_str(text);
        }
    }
    (old, new)
}

/// The 5,000-line README scrolls with only the rows in view laid out, and the rest is measured a
/// few rows per frame until every row is. How fast that is, is `frame_budget_markdown_diff`'s
/// question (release build).
#[test]
fn a_five_thousand_line_readme_lays_out_only_the_rows_in_view() {
    let (old, new) = long_readme();
    assert!(new.lines().count() >= 5_000, "{} lines", new.lines().count());
    let mut harness =
        harness(Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT), [1280.0, 900.0]);
    poll_until("every row of the README measured", || {
        harness.step();
        harness.state().view.heights.is_all_measured()
    });
    harness.input_mut().events.push(Event::PointerMoved(pos2(640.0, 450.0)));
    let mut most_cells = 0;
    for frame in 0..120 {
        let direction = if frame < 60 { -1.0 } else { 1.0 };
        harness.input_mut().events.push(Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, 300.0 * direction),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
            source: MouseWheelSource::Wheel,
        });
        harness.step();
        most_cells = most_cells.max(harness.state().view.probe.cells.len());
    }
    let rows = harness.state().view.rows.len();
    assert!(most_cells < 80, "a frame lays out the rows in view, not {most_cells} of {rows}");
}

/// The 5,000-line README's rendered diff at 4K, scrolled at the standard patterns. `#[ignore]`d;
/// see `cv_testkit::frame_bench` for how to run it.
#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_markdown_diff() {
    let mut reports: Vec<Report> = Vec::new();
    for pixels_per_point in SCALES {
        for layout in [DiffLayout::SideBySide, DiffLayout::Inline] {
            let (old, new) = long_readme();
            let mut bench = Bench::new(
                pixels_per_point,
                Scene::new(&old, &new, layout, FULL_FILE_CONTEXT),
                show,
            );
            let pointer = bench.ctx.content_rect().center();
            bench.warm_up(pointer);
            // Measured in the background meanwhile, as after a few seconds of reading.
            for _ in 0..2_000 {
                if bench.state.view.heights.is_all_measured() {
                    break;
                }
                bench.warm_up(pointer);
            }
            let view = format!("markdown diff 5k {layout:?}");
            reports.extend(bench.scroll_patterns(&view, pointer, vec2(0.0, -1.0)));
        }
    }
    frame_bench::finish(&reports);
}

#[test]
fn spare_time_measures_the_rows_nearest_the_view_first() {
    let order = |center, rows| outward(center, rows).collect::<Vec<_>>();
    assert_eq!(order(2, 5), [1, 3, 0, 4], "one above, then one below; the center is not repeated");
    assert_eq!(order(0, 3), [1, 2], "at the top only rows below");
    assert_eq!(order(4, 5), [3, 2, 1, 0], "at the end only rows above");
    assert_eq!(order(0, 1), Vec::<usize>::new(), "a single row has no neighbours");
}
