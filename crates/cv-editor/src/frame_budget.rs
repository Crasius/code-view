//! Frame-budget benchmarks of the Markdown widgets at 4K: the comment editor with a
//! 5,000-line pasted comment (idle and typing) and a 2,000+ line description with tables and
//! code blocks in the renderer. `#[ignore]`d; see `cv_testkit::frame_bench` for how to run
//! them.

use std::fmt::Write as _;

use cv_markdown::{self as markdown, RenderOptions};
use cv_testkit::frame_bench::{Bench, PATTERNS, Report, SCALES};
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Event, Id, pos2, vec2};
use review_core::ProviderKind;
use review_store::{CommentEditor, CommentEditors};

use crate::markdown_editor::{self, Options};

const COMMENT_LINES: usize = 5_000;
const DESCRIPTION_LINES: usize = 2_000;
const EDITOR_FRAMES: usize = 240;

/// Lines of review-comment Markdown: headings, prose with inline marks, lists, tasks, quotes and
/// fenced code, repeating every 25 lines.
fn markdown_line(index: usize) -> String {
    match index % 25 {
        0 => format!("## Finding {index}: the cache is rebuilt on every frame"),
        1 | 12 => String::new(),
        2 => format!("The **renderer** calls `build_job` for line {index} even when *nothing* changed; see [the docs](https://example.test/docs/{index})."),
        3 => "- first point about allocation in the hot path".to_owned(),
        4 => "  - nested detail with `inline code` and a ~~wrong~~ right fix".to_owned(),
        5 => format!("- [ ] follow-up task {index}"),
        6 => format!("- [x] done task {index}"),
        7 => "> Quoted reviewer remark that spans a fairly long sentence so it wraps on narrow panes.".to_owned(),
        8 => "```rust".to_owned(),
        9 => format!("fn frame_{index}(ui: &mut Ui) -> Result<(), Error> {{"),
        10 => "    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));".to_owned(),
        11 => "```".to_owned(),
        13 => format!("1. Numbered step {index} with **bold** text"),
        14 => "2. Second step with a [link](https://example.test) and `code`".to_owned(),
        _ => format!("Plain prose line {index} explaining the change in detail, long enough to wrap at least once at the editor's width on a 4K monitor."),
    }
}

fn comment() -> String {
    (0..COMMENT_LINES).map(markdown_line).collect::<Vec<_>>().join("\n")
}

/// A long pull request description: the comment lines plus a 30-row table every 200 lines.
fn description() -> String {
    let mut text = String::new();
    for index in 0..DESCRIPTION_LINES {
        if index % 200 == 199 {
            text.push_str(
                "\n| Area | Before | After | Notes |\n|:-----|-------:|------:|:------|\n",
            );
            for row in 0..30 {
                writeln!(text, "| `module_{row}` | {} ms | {} ms | **faster** by [{row}%](https://example.test/{row}) |", row * 3 + 7, row + 2)
                    .expect("writing to a String cannot fail");
            }
            text.push('\n');
        }
        text.push_str(&markdown_line(index));
        text.push('\n');
    }
    text
}

struct EditorState {
    text: String,
    id: Id,
}

/// The editor idle and while typing. Without AccessKit (eframe's default until an assistive
/// client connects) the frames are gated; with it they are only reported, because egui's text
/// edit then hands every row of the whole text to AccessKit on every frame.
fn bench_editor(pixels_per_point: f32, has_accesskit: bool, reports: &mut Vec<Report>) {
    let state = EditorState { text: comment(), id: Id::unique("frame_budget_editor") };
    let id = state.id;
    let mut bench = Bench::new(pixels_per_point, state, |ui, state: &mut EditorState| {
        ui.set_width(ui.available_width().min(900.0));
        // The Markdown text edit is what this measures.
        let editors =
            CommentEditors { github: CommentEditor::Markdown, ..CommentEditors::default() };
        markdown_editor::publish_editors(ui.ctx(), editors);
        let options = Options { platform: Some(ProviderKind::GitHub), ..Options::default() };
        markdown_editor::show(ui, state.id, &mut state.text, &options);
    });
    if has_accesskit {
        bench = bench.with_accesskit();
    }
    // Focus the text with the caret in the middle of the comment, as after a click there.
    let edit_id = id.with("text");
    bench.ctx.memory_mut(|memory| memory.request_focus(edit_id));
    let middle = bench.state.text.chars().count() / 2;
    let mut edit_state = TextEditState::default();
    edit_state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(middle))));
    edit_state.store(&bench.ctx, edit_id);
    let pointer = pos2(200.0, 200.0);
    bench.warm_up(pointer);
    let name = |phase: &str| {
        let accesskit = if has_accesskit { " +AccessKit" } else { "" };
        format!("editor 5k lines @{:.0}% {phase}{accesskit}", pixels_per_point * 100.0)
    };
    let idle = bench.frames(&name("idle"), vec![Vec::new(); EDITOR_FRAMES], |_, _| {});
    let typing_events = (0..EDITOR_FRAMES).map(|_| vec![Event::Text("x".to_owned())]).collect();
    let typing = bench.frames(&name("typing"), typing_events, |_, _| {});
    assert!(
        bench.state.text.matches('x').count() >= EDITOR_FRAMES,
        "the keystrokes reached the editor"
    );
    reports.extend([idle, typing].map(|report| Report { is_gated: !has_accesskit, ..report }));
}

fn bench_description(pixels_per_point: f32, reports: &mut Vec<Report>) {
    let text = description();
    let mut bench = Bench::new(pixels_per_point, text, |ui, text: &mut String| {
        egui::ScrollArea::vertical().id_salt("description").auto_shrink(false).show(ui, |ui| {
            ui.set_max_width(1100.0);
            markdown::render(
                ui,
                "description",
                text,
                RenderOptions::for_platform(ProviderKind::GitHub),
            );
        });
    });
    let rect = bench.ctx.content_rect();
    let pointer = pos2(rect.left() + 300.0, rect.center().y);
    bench.warm_up(pointer);
    for (pattern, speed) in PATTERNS {
        let name = format!("description 2k lines @{:.0}% {pattern}", pixels_per_point * 100.0);
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
fn frame_budget_comment_editor() {
    let mut reports = Vec::new();
    for pixels_per_point in SCALES {
        for has_accesskit in [false, true] {
            bench_editor(pixels_per_point, has_accesskit, &mut reports);
        }
    }
    finish(&reports);
}

#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_description() {
    let mut reports = Vec::new();
    for pixels_per_point in SCALES {
        bench_description(pixels_per_point, &mut reports);
    }
    finish(&reports);
}
