//! Frame-budget benchmark of the all-threads panel at 4K with a 500-thread review (one to eight
//! comments, Markdown, code, suggestions; every filter off, so all 500 rows are listed): open and
//! idle, then scrolled. `#[ignore]`d; see `cv_testkit::frame_bench` for how to run it.

use cv_testkit::frame_bench::{Bench, PATTERNS, Report, SCALES};
use egui::{pos2, vec2};
use review_core::ReviewThread;
use review_store::ThreadFilters;

use super::{PanelInputs, show};
use crate::{ThreadsUi, samples};

const THREADS: usize = 500;
/// The panel's width beside the diff.
const WIDTH: f32 = 480.0;
const IDLE_FRAMES: usize = 240;

struct Scene {
    threads: Vec<ReviewThread>,
    files: Vec<String>,
    ui: ThreadsUi,
    filters: ThreadFilters,
}

fn bench_panel(pixels_per_point: f32, reports: &mut Vec<Report>) {
    let threads = samples::review(THREADS);
    let files = (0..samples::REVIEW_FILES).map(|index| samples::review_file(index).0).collect();
    let scene = Scene {
        ui: samples::review_ui(&threads),
        threads,
        files,
        filters: ThreadFilters { active: false, resolved: false, unread: false, mine: false },
    };
    let mut bench = Bench::new(pixels_per_point, scene, |ui, scene: &mut Scene| {
        scene.ui.begin_frame_for_tests(ui.ctx(), &scene.threads, 1);
        let file_order: Vec<&str> = scene.files.iter().map(String::as_str).collect();
        let inputs = PanelInputs {
            threads: &scene.threads,
            file_order: &file_order,
            filters: &mut scene.filters,
            width: WIDTH,
        };
        show(ui, &mut scene.ui, inputs);
    });
    let rect = bench.ctx.content_rect();
    let pointer = pos2(rect.right() - WIDTH / 2.0, rect.center().y);
    bench.warm_up(pointer);
    let scale = pixels_per_point * 100.0;
    let idle = format!("threads panel 500 @{scale:.0}% open");
    reports.push(bench.frames(&idle, vec![Vec::new(); IDLE_FRAMES], |_, _| {}));
    for (pattern, speed) in PATTERNS {
        let name = format!("threads panel 500 @{scale:.0}% {pattern}");
        reports.push(bench.scroll_best(&name, pointer, vec2(0.0, -speed)));
    }
}

#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_threads_panel() {
    let mut reports = Vec::new();
    for pixels_per_point in SCALES {
        bench_panel(pixels_per_point, &mut reports);
    }
    for report in &reports {
        report.print();
    }
    for report in &reports {
        report.assert_within_budget();
    }
}
