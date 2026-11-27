//! Opening a file shows its rows colored in the very first frame they are on screen, while the
//! file versions are still on their way (here held back like a slow blob read until the rows
//! showed), and the file's own colors take over without changing what the rows look like.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{Color32, Shape};
use egui_kittest::Harness;
use review_core::{ProviderKind, ReviewThread};
use review_git::{DiffOptions, FileDiff, LineKind};
use review_store::{PaneSizes, ThemePreference};

use crate::diff_cache::DiffKey;
use crate::diff_view::{DiffInputs, DiffView};
use crate::find::FindInputs;
use crate::patch_colors::PatchColors;
use crate::testing::{NEW, OLD, PATH, diff_of, settle, side};
use cv_base::theme::syntax::{self, TokenKind};
use cv_base::theme::{self, Palette, motion};
use cv_code::contents::{Coloring, FileContents};
use cv_code::diff_rows::Layout;
use cv_code::line_source::{self, Pane};
use cv_pr_threads::ThreadsUi;
use cv_syntax::color_worker::observe_repaints;
use cv_syntax::grammar;
use cv_syntax::pool::Priority;
use cv_testkit::{WAIT_LIMIT, poll_until};

/// Frames the rows show before the held file versions arrive.
const FRAMES_BEFORE_CONTENTS: usize = 3;
/// One frame of [`late_colors_fade_in_instead_of_flashing`], at 60 Hz.
const FRAME: f32 = 1.0 / 60.0;
/// Frames the late colors' fade may take: its own plus the frame it starts in and one to land.
const FADE_FRAMES: u64 = (motion::FAST / FRAME) as u64 + 2;

fn key() -> DiffKey {
    key_for(PATH)
}

fn key_for(path: &str) -> DiffKey {
    DiffKey {
        path: path.into(),
        old_path: None,
        old_oid: "1".repeat(40),
        new_oid: "2".repeat(40),
        options: DiffOptions::context(3),
    }
}

fn fixture_diff() -> Arc<FileDiff> {
    diff_of(OLD, NEW)
}

fn file_contents() -> FileContents {
    FileContents {
        old: Some(side(OLD)),
        new: Some(side(NEW)),
        coloring: Coloring::Done,
        error: None,
    }
}

/// The file view's part of opening a file: the diff load from a background thread (which waits for
/// the diff's first colors, as the files tab's does), and the slower file versions.
struct Opening {
    key: DiffKey,
    view: DiffView,
    colors: PatchColors,
    diff: Option<Arc<FileDiff>>,
    diff_arrives: Receiver<Arc<FileDiff>>,
    contents: Option<FileContents>,
    contents_arrive: Receiver<FileContents>,
    threads_ui: ThreadsUi,
    split: f32,
    layout: Layout,
}

fn opening(layout: Layout) -> (Opening, Sender<Arc<FileDiff>>, Sender<FileContents>) {
    let (diff_sender, diff_arrives) = mpsc::channel();
    let (contents_sender, contents_arrive) = mpsc::channel();
    let opening = Opening {
        key: key(),
        view: DiffView::default(),
        // The wait for the first colors is what this checks, not its length: the coloring pool is
        // shared with every other test of the suite.
        colors: PatchColors::default().with_wait(WAIT_LIMIT),
        diff: None,
        diff_arrives,
        contents: None,
        contents_arrive,
        threads_ui: ThreadsUi::for_tests(ProviderKind::GitHub),
        split: PaneSizes::DEFAULT.diff_split,
        layout,
    };
    (opening, diff_sender, contents_sender)
}

/// Starts the load of `diff` under `key` in the background, as a click on the file does.
fn start(
    harness: &Harness<'_, Opening>,
    diff_sender: Sender<Arc<FileDiff>>,
    key: DiffKey,
    diff: Arc<FileDiff>,
) {
    let ctx = harness.ctx.clone();
    let colorer = harness.state().colors.colorer(&ctx);
    std::thread::spawn(move || {
        let runtime =
            tokio::runtime::Builder::new_current_thread().enable_time().build().expect("a runtime");
        runtime.block_on(colorer.color(key, diff.clone(), Priority::Urgent));
        // The harness keeps the receiver for the whole test.
        diff_sender.send(diff).expect("the view listens");
        ctx.request_repaint();
    });
}

fn show(ui: &mut egui::Ui, state: &mut Opening) {
    theme::activate(ui.ctx(), ThemePreference::Light);
    if let Ok(diff) = state.diff_arrives.try_recv() {
        state.diff = Some(diff);
    }
    // As the files tab: after the diff, so the colors its load waited for are taken in this frame.
    state.colors.poll();
    if let Ok(contents) = state.contents_arrive.try_recv() {
        state.contents = Some(contents);
    }
    let Some(diff) = state.diff.clone() else {
        return;
    };
    let threads: Vec<&ReviewThread> = Vec::new();
    let inputs = DiffInputs {
        diff: &diff,
        contents: state.contents.as_ref(),
        threads: &threads,
        layout: state.layout,
        font_size: 13.0,
        wrap: false,
        can_comment: true,
        can_comment_outside_hunks: true,
        path: &state.key.path,
        old_path: None,
        line_spans: state.colors.spans(&state.key),
        show_threads: true,
        find: FindInputs::default(),
        past_end: true,
    };
    state.view.show(ui, &inputs, &mut state.split, &mut state.threads_ui);
}

fn harness(layout: Layout) -> (Harness<'static, Opening>, Sender<FileContents>) {
    let (state, diff_sender, contents_sender) = opening(layout);
    let harness = Harness::builder()
        .with_size([1180.0, 760.0])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(show, state);
    start(&harness, diff_sender, key(), fixture_diff());
    (harness, contents_sender)
}

/// The colors of every glyph painted in the last frame.
fn glyph_colors(harness: &Harness<'_, Opening>) -> Vec<Color32> {
    fn collect(shape: &Shape, colors: &mut Vec<Color32>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, colors)),
            Shape::Text(text) => {
                for row in &text.galley.rows {
                    let glyphs = &row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()];
                    colors.extend(glyphs.iter().map(|vertex| vertex.color));
                }
            }
            _ => {}
        }
    }
    let mut colors = Vec::new();
    for clipped in &harness.output().shapes {
        collect(&clipped.shape, &mut colors);
    }
    colors
}

/// Frames until the file versions, sent once the rows showed for [`FRAMES_BEFORE_CONTENTS`]
/// frames, arrived and a few after; per frame whether code was painted and whether it showed
/// keyword and string colors.
fn run_until_contents(
    harness: &mut Harness<'_, Opening>,
    contents_sender: &Sender<FileContents>,
) -> Vec<(bool, bool)> {
    let (keyword, string) = (
        syntax::color(TokenKind::Keyword, &Palette::LIGHT),
        syntax::color(TokenKind::String, &Palette::LIGHT),
    );
    let mut frames: Vec<(bool, bool)> = Vec::new();
    let mut after_contents = 0;
    poll_until("three frames after the file versions arrived", || {
        harness.step();
        let colors = glyph_colors(harness);
        let has_code = harness.state().diff.is_some() && !colors.is_empty();
        frames.push((has_code, colors.contains(&keyword) && colors.contains(&string)));
        if has_code
            && frames.iter().filter(|(has_code, _)| *has_code).count() == FRAMES_BEFORE_CONTENTS
        {
            contents_sender.send(file_contents()).expect("the view listens");
        }
        if harness.state().contents.is_some() {
            after_contents += 1;
        }
        after_contents == 3
    });
    frames
}

fn assert_colored_from_the_first_frame(layout: Layout) {
    let (mut harness, contents_sender) = harness(layout);
    let frames = run_until_contents(&mut harness, &contents_sender);
    let shown: Vec<bool> =
        frames.iter().filter(|(has_code, _)| *has_code).map(|(_, colored)| *colored).collect();
    assert!(
        shown.len() > FRAMES_BEFORE_CONTENTS,
        "the rows showed before the file versions arrived: {frames:?}"
    );
    assert!(
        shown.iter().all(|colored| *colored),
        "{layout:?}: every frame with rows on screen is colored, the first one too: {shown:?}"
    );
}

#[test]
fn inline_rows_are_colored_in_their_first_frame() {
    assert_colored_from_the_first_frame(Layout::Inline);
}

#[test]
fn side_by_side_rows_are_colored_in_their_first_frame() {
    assert_colored_from_the_first_frame(Layout::SideBySide);
}

/// When the colors can only come after the rows showed (a grammar still compiling past the load's
/// wait), they fade in over a few frames instead of flashing on; at once under reduced motion.
#[test]
fn late_colors_fade_in_instead_of_flashing() {
    for is_reduced in [false, true] {
        late_colors_arrive(is_reduced);
    }
}

fn late_colors_arrive(is_reduced: bool) {
    let (state, diff_sender, _contents_sender) = opening(Layout::Inline);
    let mut harness = Harness::builder()
        .with_size([1180.0, 760.0])
        .with_step_dt(FRAME)
        .with_max_steps(FADE_FRAMES)
        .with_context_setup(move |ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, is_reduced);
        })
        .build_ui_state(show, state);
    let diff = fixture_diff();
    diff_sender.send(diff.clone()).expect("the view listens");
    poll_until("the rows show", || {
        harness.step();
        !glyph_colors(&harness).is_empty()
    });
    let keyword = syntax::color(TokenKind::Keyword, &Palette::LIGHT);
    assert!(!glyph_colors(&harness).contains(&keyword), "no colors yet");
    let ctx = harness.ctx.clone();
    let repaints = observe_repaints(&ctx);
    let colors = &mut harness.state_mut().colors;
    colors.ensure(&ctx, &key(), &diff);
    assert!(colors.wait_finished(&repaints, &key()).is_some(), "the diff was colored");
    harness.step();
    let first = glyph_colors(&harness);
    settle(&mut harness);
    assert!(glyph_colors(&harness).contains(&keyword), "the colors are fully there after the fade");
    if is_reduced {
        assert!(first.contains(&keyword), "reduced motion: at once");
    } else {
        assert!(
            !first.contains(&keyword),
            "the first colored frame is part of the fade, not the full color"
        );
        assert!(
            first
                .iter()
                .any(|color| *color != Palette::LIGHT.text && !colors_of_ui().contains(color)),
            "but already on its way"
        );
    }
}

const TS_PATH: &str = "web/cart.ts";
const TS_OLD: &str = r#"import { Injectable } from "@angular/core";

/** Keeps the cart's items. */
@Injectable({ providedIn: "root" })
export class Cart<T extends { price: number }> {
    private readonly items = new Map<string, T>();

    add(id: string, item: T): void {
        this.items.set(id, item);
    }

    total(): number {
        let sum = 0;
        for (const [, item] of this.items) {
            sum += item.price;
        }
        return sum;
    }
}
"#;
const TS_NEW: &str = r#"import { Injectable } from "@angular/core";

/** Keeps the cart's items. */
@Injectable({ providedIn: "root" })
export class Cart<T extends { price: number }> {
    private readonly items = new Map<string, T>();

    add(id: string, item: T): void {
        if (this.items.has(id)) {
            throw new Error(`duplicate item ${id}`);
        }
        this.items.set(id, item);
    }

    total(discount = 0): number {
        const sum = [...this.items.values()].reduce((sum, item) => sum + item.price, 0);
        return sum * (1 - discount);
    }
}
"#;

fn changed_file(path: &str) -> review_git::ChangedFile {
    review_git::ChangedFile {
        additions: Some(8),
        deletions: Some(5),
        ..review_git::ChangedFile::sample(path, review_git::ChangeKind::Modified)
    }
}

/// A TypeScript file (the slowest grammar to compile) opened right as the pull request opened:
/// the warm-up that started with the file list still compiles its grammar, and the diff load waits
/// for it instead of showing the rows plain, so they are colored in their first frame too.
#[test]
fn a_file_opened_right_after_the_pull_request_is_colored_in_its_first_frame() {
    let grammar = grammar::for_path(TS_PATH, "").expect("a TypeScript grammar");
    grammar::forget(grammar);
    let (mut state, diff_sender, _contents_sender) = opening(Layout::Inline);
    state.key = key_for(TS_PATH);
    // The wait for the warming grammar is what this checks, not its length: the whole suite
    // compiling in parallel can take the grammar longer than `WARMING_WAIT`.
    state.colors = PatchColors::default().with_warming_wait(WAIT_LIMIT);
    // The pull request opens: its changed files are known, the warm-up starts on the start file.
    state
        .colors
        .warm_up(&[changed_file("src/lib.rs"), changed_file(TS_PATH)], Some(TS_PATH.to_owned()));
    let started = Instant::now();
    while !grammar::is_warming(grammar) && started.elapsed() < Duration::from_secs(30) {
        std::thread::yield_now();
    }
    let mut harness = Harness::builder()
        .with_size([1180.0, 760.0])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(show, state);
    // The file opens at once.
    start(&harness, diff_sender, key_for(TS_PATH), diff_of(TS_OLD, TS_NEW));
    let (keyword, string) = (
        syntax::color(TokenKind::Keyword, &Palette::LIGHT),
        syntax::color(TokenKind::String, &Palette::LIGHT),
    );
    let mut shown = Vec::new();
    poll_until("five frames with the diff's rows", || {
        harness.step();
        let colors = glyph_colors(&harness);
        if harness.state().diff.is_some() && !colors.is_empty() {
            shown.push(colors.contains(&keyword) && colors.contains(&string));
        }
        shown.len() == 5
    });
    assert!(
        shown.iter().all(|colored| *colored),
        "every frame with rows on screen is colored, the first one too: {shown:?}"
    );
}

/// Colors the diff view paints that are not syntax colors (numbers, markers, plain text).
fn colors_of_ui() -> Vec<Color32> {
    let palette = Palette::LIGHT;
    vec![palette.text, palette.text_secondary, palette.diff_gutter, palette.accent_text]
}

/// The colors by diff line and the file's own colors agree on every line of the sample, so when
/// the file's arrive, no row changes color.
#[test]
fn the_files_own_colors_take_over_unchanged() {
    let diff = fixture_diff();
    let contents = file_contents();
    let mut colors = PatchColors::default();
    let ctx = egui::Context::default();
    let repaints = observe_repaints(&ctx);
    colors.ensure(&ctx, &key(), &diff);
    let spans = colors.wait_finished(&repaints, &key()).expect("the diff was colored");
    for (index, line) in diff.lines().iter().enumerate() {
        if line.kind == LineKind::Hunk {
            continue;
        }
        for pane in [Pane::Left, Pane::Right, Pane::Inline] {
            let is_hidden = matches!(
                (line.kind, pane),
                (LineKind::Added, Pane::Left) | (LineKind::Deleted, Pane::Right)
            );
            if is_hidden {
                continue;
            }
            let from_file = line_source::diff_line(&diff, Some(&contents), index, pane);
            assert!(from_file.has_file_spans, "line {index} has the file's colors");
            assert_eq!(
                spans.line(index),
                Some(from_file.spans),
                "{pane:?} line {index} {:?}",
                from_file.text
            );
        }
    }
}
