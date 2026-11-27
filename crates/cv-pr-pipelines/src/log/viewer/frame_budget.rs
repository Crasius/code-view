//! Frame-budget benchmarks of the Pipelines tab at 4K: the log viewer with a 150k-line log
//! (timestamps, ANSI colors, markers), plain and with 100k lines selected, and the stage graph with 60 stages of 10 jobs while
//! panning at 100 % and 50 % zoom and while pinch-zooming. `#[ignore]`d; see `cv_testkit::frame_bench` for how to run them.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use cv_testkit::frame_bench::{self, Bench, Report, SCALES};
use cv_widgets::text_selection::{Caret, Selection};
use egui::{Event, pos2, vec2};
use review_core::{PipelineJob, ProviderKind, RunStatus};
use review_store::LogFile;

use super::LogViewer;
use super::fetch::LogFetch;
use super::selecting::Selecting;
use crate::graph::stage_card::CardContext;
use crate::graph::stage_graph::{self, GraphInput};
use crate::log::download::LoadedLog;

const LOG_LINES: usize = 150_000;
/// The selection the second log run scrolls through: from mid-line 1,000 to mid-line 101,000.
const SELECTED: (Caret, Caret) =
    (Caret { row: 1_000, column: 9 }, Caret { row: 101_000, column: 14 });

fn log_line(index: usize) -> String {
    let timestamp = format!(
        "2026-10-18T10:{:02}:{:02}.{:07}Z",
        index / 60_000 % 60,
        index / 1000 % 60,
        index % 10_000_000
    );
    let body = match index % 12 {
        0 => "##[group]Run cargo test --workspace --release".to_owned(),
        1 => format!(
            "\u{1b}[32m   Compiling\u{1b}[0m review-core v0.1.{index} (D:\\a\\review\\crates\\review-core)"
        ),
        2 => format!(
            "test features::pull_request::files::diff_view::tests::case_{index} ... \u{1b}[32mok\u{1b}[0m"
        ),
        3 => format!(
            "\u{1b}[33mwarning\u{1b}[0m: unused variable: `value_{index}` --> src/lib.rs:{}:9",
            index % 900
        ),
        4 => "##[error]Process completed with exit code 1.".to_owned(),
        5 => String::new(),
        6 => format!(
            "  Downloaded serde_json v1.0.{} from https://index.crates.io/ in {} ms",
            index % 200,
            index % 997
        ),
        _ => format!(
            "[{index:>6}] info  worker::queue  processed job {} in {}.{:03} s, {} items, {} retries, cache hit ratio {}.{}%",
            index * 31 % 100_000,
            index % 9,
            index % 1000,
            index % 5000,
            index % 3,
            index % 100,
            index % 10
        ),
    };
    format!("{timestamp} {body}")
}

fn write_log() -> PathBuf {
    let path = std::env::temp_dir().join(format!("review-frame-budget-{}.log", std::process::id()));
    let mut file = std::io::BufWriter::new(std::fs::File::create(&path).expect("create the log"));
    for index in 0..LOG_LINES {
        writeln!(file, "{}", log_line(index)).expect("write the log");
    }
    file.flush().expect("flush the log");
    path
}

struct LogState {
    viewer: LogViewer,
    log: Arc<LoadedLog>,
}

fn selection() -> Selection {
    let mut selection = Selection::caret((), SELECTED.0);
    selection.extend_to(SELECTED.1);
    selection
}

fn bench_log(
    pixels_per_point: f32,
    log: &Arc<LoadedLog>,
    selection: Option<Selection>,
    reports: &mut Vec<Report>,
) {
    let state = LogState {
        viewer: LogViewer {
            fetch: LogFetch::loaded_for_test(log.clone()),
            // The selected run starts well inside the selection; every painted line is selected.
            scroll_to: selection.map(|_| 20_000),
            selecting: Selecting { selection, ..Selecting::default() },
            ..LogViewer::default()
        },
        log: log.clone(),
    };
    let mut bench = Bench::new(pixels_per_point, state, |ui, state: &mut LogState| {
        let log = state.log.clone();
        state.viewer.lines(ui, &log, 13.0);
    });
    let rect = bench.ctx.content_rect();
    let pointer = pos2(rect.center().x, rect.center().y);
    bench.warm_up(pointer);
    let selected = if selection.is_some() { ", 100k selected" } else { "" };
    reports.extend(bench.scroll_patterns(
        &format!("log 150k lines{selected}"),
        pointer,
        vec2(0.0, -1.0),
    ));
}

#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_log() {
    let path = write_log();
    let file = LogFile::open(&path).expect("index the log");
    assert_eq!(file.line_count(), LOG_LINES);
    let log = Arc::new(LoadedLog { file, first_error: Some(4) });
    let mut reports = Vec::new();
    for pixels_per_point in SCALES {
        bench_log(pixels_per_point, &log, None, &mut reports);
        bench_log(pixels_per_point, &log, Some(selection()), &mut reports);
    }
    // "Copy log" and Ctrl+C build their text once, off the UI thread; this is how long the user waits.
    let started = std::time::Instant::now();
    let copied = super::raw_log::plain_text(&path).expect("read the log");
    let copy_ms = started.elapsed().as_secs_f32() * 1000.0;
    let started = std::time::Instant::now();
    let selected =
        super::raw_log::selected_text(&log.file, &selection()).expect("read the selection");
    let selected_ms = started.elapsed().as_secs_f32() * 1000.0;
    drop(log);
    // Best effort: a leftover file in the temp folder harms nothing.
    let _cleanup = std::fs::remove_file(&path);
    println!(
        "copy log: {LOG_LINES} lines, {:.1} MB of plain text in {copy_ms:.0} ms (background thread)",
        copied.len() as f32 / 1e6
    );
    println!(
        "copy selection: 100k lines, {:.1} MB in {selected_ms:.0} ms (background thread)",
        selected.len() as f32 / 1e6
    );
    frame_bench::finish(&reports);
}

const STAGES: usize = 60;
const JOBS_PER_STAGE: usize = 10;

/// Stages in dependency chains of three (each waits for one or two earlier stages), with every
/// status, and ten jobs each.
fn stages() -> Vec<PipelineJob> {
    let statuses = [
        RunStatus::Succeeded,
        RunStatus::Failed,
        RunStatus::Running,
        RunStatus::Queued,
        RunStatus::Skipped,
        RunStatus::Canceled,
    ];
    (0..STAGES)
        .map(|index| {
            let mut depends_on = Vec::new();
            if index >= 3 {
                depends_on.push(format!("stage-{}", index - 3));
            }
            if index >= 4 && index % 2 == 0 {
                depends_on.push(format!("stage-{}", index - 4));
            }
            PipelineJob {
                id: format!("stage-{index}"),
                name: format!(
                    "Stage {index}: build and test the {} component",
                    ["core", "store", "git", "app"][index % 4]
                ),
                status: statuses[index % statuses.len()],
                depends_on,
                children: (0..JOBS_PER_STAGE)
                    .map(|job| PipelineJob {
                        id: format!("stage-{index}-job-{job}"),
                        name: format!(
                            "Job {job} ({} on windows-latest)",
                            ["unit tests", "lint", "package"][job % 3]
                        ),
                        status: statuses[(index + job) % statuses.len()],
                        log_id: Some(format!("log-{index}-{job}")),
                        ..PipelineJob::default()
                    })
                    .collect(),
                ..PipelineJob::default()
            }
        })
        .collect()
}

struct GraphState {
    stages: Vec<PipelineJob>,
    expanded: HashSet<String>,
}

fn bench_stage_graph(pixels_per_point: f32, reports: &mut Vec<Report>) {
    let stages = stages();
    // Every other stage open, so the canvas holds cards with all ten job rows.
    let expanded = stages.iter().step_by(2).map(|stage| stage.id.clone()).collect();
    let mut bench = Bench::new(
        pixels_per_point,
        GraphState { stages, expanded },
        |ui, state: &mut GraphState| {
            let input = GraphInput {
                stages: &state.stages,
                expanded: &state.expanded,
                highlighted: Some("stage-7"),
                scroll_to: None,
                context: CardContext {
                    platform: ProviderKind::AzureDevOps,
                    has_token: true,
                    is_busy: false,
                    stages: &state.stages,
                    known: crate::attempts::Known::default(),
                },
            };
            stage_graph::show(ui, input);
        },
    );
    let pointer = bench.ctx.content_rect().center();
    bench.warm_up(pointer);
    let scale = pixels_per_point * 100.0;
    // Diagonal: across the dependency columns and down the stacked cards.
    let diagonal = vec2(-1.0, -0.25);
    reports.extend(bench.scroll_patterns("stage graph 60x10 pan", pointer, diagonal));
    // A pinch in and out between 50 % and 100 %: new text sizes while moving, exact ones at rest.
    let pinch: Vec<Vec<Event>> = (0..ZOOM_FRAMES)
        .map(|frame| vec![Event::Zoom(if (frame / 60) % 2 == 0 { 0.99 } else { 1.0 / 0.99 })])
        .collect();
    reports.push(bench.frames(
        &format!("stage graph 60x10 @{scale:.0}% pinch zoom"),
        pinch,
        |_, _| {},
    ));
    // Zoomed out to 50 %, four times as many cards are on screen, their text laid out for it.
    bench.frames("zoom to 50 %", vec![vec![Event::Zoom(0.5)]], |_, _| {});
    bench.warm_up(pointer);
    reports.extend(bench.scroll_patterns("stage graph 60x10 zoomed 50% pan", pointer, diagonal));
}

/// Frames of the pinch-zoom run: four sweeps of a quarter second at 240 Hz.
const ZOOM_FRAMES: usize = 240;

#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_stage_graph() {
    let mut reports = Vec::new();
    for pixels_per_point in SCALES {
        bench_stage_graph(pixels_per_point, &mut reports);
    }
    frame_bench::finish(&reports);
}
