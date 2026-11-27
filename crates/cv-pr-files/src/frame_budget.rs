//! Frame-budget benchmarks of the Files tab's worst cases at 4K: a 100k-line diff with syntax
//! colors and a 10k-file tree. `#[ignore]`d; see `cv_testkit::frame_bench` for how to run them.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use egui::{Pos2, pos2, vec2};
use review_core::{FileCategory, ProviderKind, ReviewThread, ThreadAnchor, ThreadStatus};
use review_git::{ChangeKind, ChangedFile, FileDiff};

use cv_base::theme::syntax::TokenKind;
use cv_code::contents::{Coloring, FileContents, SideContents};
use cv_code::diff_rows::Layout;
use cv_code::line_source::Pane;
use cv_pr_diff::diff_view::{DiffInputs, DiffView};
use cv_pr_diff::find::FindInputs;
use cv_pr_threads::{ThreadsUi, samples};
use cv_pr_tree::{FileTree, TreeInputs, side_panel};
use cv_syntax::blob_text::BlobText;
use cv_syntax::highlight::{LineSpans, Span};
use cv_testkit::frame_bench::{Bench, PATTERNS, Report, SCALES};

const DIFF_LINES: usize = 100_000;
const TREE_FILES: usize = 10_000;
/// A thread every this many diff blocks of ten lines, so a screen always shows a few cards.
const THREAD_EVERY_BLOCKS: usize = 12;

/// Open threads on context lines all through the huge diff: two or three comments with Markdown,
/// every fourth resolved (collapsed), every third with a suggestion.
fn bench_threads() -> Vec<ReviewThread> {
    let people = [samples::jonas(), samples::mara(), samples::erika()];
    (0..DIFF_LINES / 10)
        .step_by(THREAD_EVERY_BLOCKS)
        .map(|block| {
            let (old_line, new_line) = (block as u32 * 7 + 3, block as u32 * 8 + 3);
            let mut bodies = vec![
                format!("Should `value_{block}` be computed **once** here? It runs for every entry."),
                "Good catch, the loop calls it again. I'll move it up.".to_owned(),
            ];
            if block % 3 == 0 {
                bodies.push(format!("Something like this?\n```suggestion\nlet value_{block} = cached_total(&input);\n```"));
            }
            let status = if block % 4 == 0 { ThreadStatus::Resolved } else { ThreadStatus::Active };
            let anchor = ThreadAnchor {
                path: BENCH_PATH.into(),
                old_path: None,
                old_line: Some(old_line),
                new_line: Some(new_line),
                range: None,
            };
            let comments = bodies
                .iter()
                .enumerate()
                .map(|(index, body)| {
                    let (id, minutes) = (format!("{block}-{index}"), (block * 3 + index) as i64);
                    samples::comment(&id, &people[(block + index) % people.len()], minutes, body)
                })
                .collect();
            samples::thread(&format!("thread-{block}"), status, Some(anchor), comments)
        })
        .collect()
}

const BENCH_PATH: &str = "src/generated/huge.rs";

/// A synthetic Rust-like source line: indentation, keywords, calls, strings, numbers, comments,
/// 0 to ~150 columns.
fn source_line(index: usize, variant: usize) -> String {
    let indent = "    ".repeat(1 + index % 4);
    match (index + variant) % 9 {
        0 => format!(
            "{indent}let value_{index} = compute_total(&input[{index}..], \"label {variant}\", {index} * 3); // running total"
        ),
        1 => format!("{indent}if let Some(entry) = self.entries.get(&key_{index}) {{"),
        2 => format!("{indent}}}"),
        3 => format!(
            "{indent}return Err(Error::Invalid(format!(\"line {{}} of {{}} is out of range: {index}\", line, total)));"
        ),
        4 => format!(
            "{indent}for (position, item) in items.iter().enumerate().filter(|(_, item)| item.weight > {variant}) {{"
        ),
        5 => String::new(),
        6 => format!(
            "{indent}/// Documentation for item {index}, which explains what the next function does and why."
        ),
        7 => format!(
            "{indent}pub fn handle_event_{index}(&mut self, event: &Event, context: &mut Context<'_>) -> Result<(), Error> {{"
        ),
        _ => format!(
            "{indent}self.cache.insert(key_{index}.clone(), Arc::new(Value::from_parts({variant}, {index}, true)));"
        ),
    }
}

/// Cheap tokenizer standing in for the grammar: same span density as real highlighting.
fn spans_of(line: &str) -> Vec<Span> {
    const KEYWORDS: &[&str] =
        &["let", "if", "for", "in", "pub", "fn", "mut", "self", "true", "return"];
    let bytes = line.as_bytes();
    let mut spans: Vec<Span> = Vec::new();
    let mut push = |start: usize, kind: TokenKind| {
        if spans.last().map_or(TokenKind::Plain, |span| span.kind()) != kind {
            spans.push(Span::new(start, kind));
        }
    };
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let start = index;
        if line[index..].starts_with("//") {
            push(start, TokenKind::Comment);
            break;
        } else if byte == b'"' {
            index += 1;
            while index < bytes.len() && bytes[index] != b'"' {
                index += 1;
            }
            index += 1;
            push(start, TokenKind::String);
        } else if byte.is_ascii_digit() {
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            push(start, TokenKind::Number);
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            let word = &line[start..index];
            let kind = if KEYWORDS.contains(&word) {
                TokenKind::Keyword
            } else if bytes.get(index) == Some(&b'(') {
                TokenKind::Function
            } else if word.starts_with(|first: char| first.is_ascii_uppercase()) {
                TokenKind::Type
            } else {
                TokenKind::Variable
            };
            push(start, kind);
        } else {
            index += 1;
            push(start, TokenKind::Plain);
        }
    }
    spans
}

fn side(lines: &[String]) -> SideContents {
    let mut text = lines.join("\n");
    text.push('\n');
    let mut spans = LineSpans::default();
    for line in lines {
        spans.push_line(&spans_of(line));
    }
    SideContents { text: Arc::new(BlobText::from_bytes(text.into_bytes())), spans: Arc::new(spans) }
}

/// One hunk over the whole file: per block of ten diff lines, five unchanged, two deleted and
/// three added lines that differ from the deleted ones by a word or two.
fn huge_diff() -> (FileDiff, FileContents) {
    let mut old = Vec::new();
    let mut new = Vec::new();
    let mut body = String::new();
    for block in 0..DIFF_LINES / 10 {
        for offset in 0..5 {
            let line = source_line(block * 10 + offset, 0);
            writeln!(body, " {line}").unwrap();
            old.push(line.clone());
            new.push(line);
        }
        for offset in 5..7 {
            let line = source_line(block * 10 + offset, 0);
            writeln!(body, "-{line}").unwrap();
            old.push(line);
        }
        for offset in 5..8 {
            let line = source_line(block * 10 + offset, 1);
            writeln!(body, "+{line}").unwrap();
            new.push(line);
        }
    }
    let unified = format!("@@ -1,{} +1,{} @@\n{body}", old.len(), new.len());
    let diff = FileDiff::parse_unified(unified.as_bytes());
    let contents = FileContents {
        old: Some(side(&old)),
        new: Some(side(&new)),
        coloring: Coloring::Done,
        error: None,
    };
    (diff, contents)
}

struct DiffState {
    view: DiffView,
    diff: Arc<FileDiff>,
    /// `None` while the file versions are on their way: the colors by diff line stand in.
    contents: Option<FileContents>,
    line_spans: Option<LineSpans>,
    threads: Vec<ReviewThread>,
    threads_ui: ThreadsUi,
    split: f32,
    layout: Layout,
    wrap: bool,
}

/// `selected`: every line's text selected, so each visible line also paints its selection.
fn bench_diff(
    pixels_per_point: f32,
    layout: Layout,
    wrap: bool,
    selected: bool,
    reports: &mut Vec<Report>,
) {
    let (diff, contents) = huge_diff();
    assert!(diff.lines().len() >= DIFF_LINES, "the diff has {} lines", diff.lines().len());
    let state = DiffState {
        view: DiffView::default(),
        diff: Arc::new(diff),
        contents: Some(contents),
        line_spans: None,
        threads: bench_threads(),
        threads_ui: ThreadsUi::for_tests(ProviderKind::GitHub),
        split: 0.5,
        layout,
        wrap,
    };
    let mut bench = Bench::new(pixels_per_point, state, show_diff);
    let pointer = center_of(&bench);
    if selected {
        bench.state.view.select_all_text(if layout.is_single_pane() {
            Pane::Inline
        } else {
            Pane::Right
        });
    }
    bench.warm_up(pointer);
    let layout_name = match (layout, wrap) {
        (Layout::SideBySide, false) => "side by side",
        (Layout::SideBySide, true) => "side by side, wrapped",
        (Layout::Inline, _) => "inline",
        (Layout::NewOnly, _) => "new only",
        (Layout::WholeFile(_), _) => "whole file",
        (Layout::Compact(_), _) => "compact",
        (Layout::CompactSplit(_), _) => "compact side by side",
    };
    let layout_name =
        if selected { format!("{layout_name}, selected") } else { layout_name.to_owned() };
    for (pattern, speed) in PATTERNS {
        let name = format!("diff 100k {layout_name} @{:.0}% {pattern}", pixels_per_point * 100.0);
        reports.push(bench.scroll_best(&name, pointer, vec2(0.0, -speed)));
    }
}

/// Ctrl+Plus zooms from 100 % to 200 % (or the window moves between monitors of different
/// scale): every zoom step re-rasterizes all glyphs at the new scale and grows the font atlas.
fn bench_zoom(pixels_per_point: f32, reports: &mut Vec<Report>) {
    const FRAMES_PER_STEP: usize = 4;
    let (diff, contents) = huge_diff();
    let state = DiffState {
        view: DiffView::default(),
        diff: Arc::new(diff),
        contents: Some(contents),
        line_spans: None,
        threads: bench_threads(),
        threads_ui: ThreadsUi::for_tests(ProviderKind::GitHub),
        split: 0.5,
        layout: Layout::SideBySide,
        wrap: false,
    };
    let mut bench = Bench::new(pixels_per_point, state, show_diff);
    let pointer = center_of(&bench);
    bench.warm_up(pointer);
    let steps = 11;
    let events = vec![Vec::new(); steps * FRAMES_PER_STEP];
    let name = format!("diff zoom sweep 100..200% @{:.0}%", pixels_per_point * 100.0);
    let report = bench.frames(&name, events, |ctx, frame| {
        if frame % FRAMES_PER_STEP == 0 {
            ctx.set_zoom_factor(1.0 + (frame / FRAMES_PER_STEP) as f32 * 0.1);
        }
    });
    reports.push(report);
}

/// The huge diff before its file versions arrived: every row colored by diff line.
fn bench_patch_colors(pixels_per_point: f32, layout: Layout, reports: &mut Vec<Report>) {
    let (diff, _) = huge_diff();
    let mut line_spans = LineSpans::default();
    for line in diff.lines() {
        line_spans.push_line(&spans_of(diff.text(line)));
    }
    let state = DiffState {
        view: DiffView::default(),
        diff: Arc::new(diff),
        contents: None,
        line_spans: Some(line_spans),
        threads: bench_threads(),
        threads_ui: ThreadsUi::for_tests(ProviderKind::GitHub),
        split: 0.5,
        layout,
        wrap: false,
    };
    let mut bench = Bench::new(pixels_per_point, state, show_diff);
    let pointer = center_of(&bench);
    bench.warm_up(pointer);
    let layout_name = if layout == Layout::Inline { "inline" } else { "side by side" };
    for (pattern, speed) in PATTERNS {
        let name = format!(
            "diff 100k colored by diff line {layout_name} @{:.0}% {pattern}",
            pixels_per_point * 100.0
        );
        reports.push(bench.scroll_best(&name, pointer, vec2(0.0, -speed)));
    }
}

fn show_diff(ui: &mut egui::Ui, state: &mut DiffState) {
    let threads: Vec<&ReviewThread> = state.threads.iter().collect();
    let inputs = DiffInputs {
        diff: &state.diff,
        contents: state.contents.as_ref(),
        threads: &threads,
        layout: state.layout,
        font_size: 13.0,
        wrap: state.wrap,
        can_comment: true,
        can_comment_outside_hunks: true,
        path: BENCH_PATH,
        old_path: None,
        line_spans: state.line_spans.as_ref(),
        show_threads: true,
        find: FindInputs::default(),
    };
    state.view.show(ui, &inputs, &mut state.split, &mut state.threads_ui);
}

fn center_of<State>(bench: &Bench<'_, State>) -> Pos2 {
    bench.ctx.content_rect().center()
}

fn many_files() -> Vec<ChangedFile> {
    let categories = [
        FileCategory::Code,
        FileCategory::Code,
        FileCategory::Tests,
        FileCategory::Docs,
        FileCategory::Config,
    ];
    let kinds = [
        ChangeKind::Modified,
        ChangeKind::Added,
        ChangeKind::Modified,
        ChangeKind::Deleted,
        ChangeKind::Renamed,
    ];
    (0..TREE_FILES)
        .map(|index| {
            let path = format!(
                "src/area_{}/module_{}/feature_{}/component_with_a_longer_name_{index}.rs",
                index % 7,
                index / 7 % 13,
                index / 91 % 11
            );
            ChangedFile {
                path,
                old_path: (index % 5 == 4).then(|| format!("src/old/place_{index}.rs")),
                kind: kinds[index % kinds.len()],
                old_oid: format!("{index:040x}"),
                new_oid: format!("{:040x}", index + 1),
                additions: Some((index * 7 % 400) as u32),
                deletions: Some((index * 3 % 200) as u32),
                is_binary: false,
                category: categories[index % categories.len()],
                whitespace_only: false,
            }
        })
        .collect()
}

struct TreeState {
    tree: FileTree,
    files: Arc<Vec<ChangedFile>>,
    viewed: HashSet<String>,
    thread_counts: HashMap<String, usize>,
}

fn bench_tree(pixels_per_point: f32, reports: &mut Vec<Report>) {
    let files = Arc::new(many_files());
    let viewed = files.iter().step_by(3).map(|file| file.path.clone()).collect();
    let thread_counts = files.iter().step_by(17).map(|file| (file.path.clone(), 2)).collect();
    let state = TreeState { tree: FileTree::default(), files, viewed, thread_counts };
    let mut bench = Bench::new(pixels_per_point, state, |ui, state: &mut TreeState| {
        let viewed = &state.viewed;
        let is_viewed = |file: &ChangedFile| viewed.contains(&file.path);
        state.tree.refresh(&state.files, &is_viewed, 0);
        let inputs = TreeInputs {
            files: &state.files,
            selected: Some(5),
            selected_folder: None,
            is_viewed: &is_viewed,
            thread_counts: &state.thread_counts,
            vote: cv_pr_tree::VoteOffer::None,
        };
        side_panel::show(ui, 340.0, &mut state.tree, &inputs, false);
    });
    let rect = bench.ctx.content_rect();
    let pointer = pos2(rect.left() + 150.0, rect.center().y);
    bench.warm_up(pointer);
    for (pattern, speed) in PATTERNS {
        let name = format!("tree 10k files @{:.0}% {pattern}", pixels_per_point * 100.0);
        reports.push(bench.scroll_best(&name, pointer, vec2(0.0, -speed)));
    }
}

fn finish(reports: &[Report]) {
    for report in reports {
        report.print();
    }
    for report in reports {
        report.assert_within_budget();
    }
}

#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_diff() {
    let mut reports = Vec::new();
    for pixels_per_point in SCALES {
        bench_diff(pixels_per_point, Layout::SideBySide, false, false, &mut reports);
        bench_diff(pixels_per_point, Layout::SideBySide, true, false, &mut reports);
        bench_diff(pixels_per_point, Layout::Inline, false, false, &mut reports);
        bench_diff(pixels_per_point, Layout::Inline, false, true, &mut reports);
        bench_diff(pixels_per_point, Layout::SideBySide, true, true, &mut reports);
        bench_zoom(pixels_per_point, &mut reports);
    }
    finish(&reports);
}

#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_diff_patch_colors() {
    let mut reports = Vec::new();
    for pixels_per_point in SCALES {
        bench_patch_colors(pixels_per_point, Layout::SideBySide, &mut reports);
        bench_patch_colors(pixels_per_point, Layout::Inline, &mut reports);
    }
    finish(&reports);
}

#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_tree() {
    let mut reports = Vec::new();
    for pixels_per_point in SCALES {
        bench_tree(pixels_per_point, &mut reports);
    }
    finish(&reports);
}
