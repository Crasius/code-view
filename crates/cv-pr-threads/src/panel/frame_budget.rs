//! Frame-budget benchmark of the all-threads panel at 4K with a 500-thread review (one to eight
//! comments, Markdown, code, suggestions; every filter off, so all 500 rows are listed): open and
//! idle, then scrolled. `#[ignore]`d; see `cv_testkit::frame_bench` for how to run it.

use cv_testkit::frame_bench::{self, Bench, Report, SCALES};
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
    let files = (0..samples::REVIEW_FILES).map(samples::review_path).collect();
    let scene =
        Scene { ui: samples::review_ui(&threads), threads, files, filters: ThreadFilters::ALL };
    let mut bench = Bench::new(pixels_per_point, scene, |ui, scene: &mut Scene| {
        scene.ui.begin_frame_for_tests(ui.ctx(), &scene.threads, 1);
        let inputs = PanelInputs {
            threads: &scene.threads,
            file_order: &mut scene.files.iter().map(String::as_str),
            file_order_version: 0,
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
    reports.extend(bench.scroll_patterns("threads panel 500", pointer, vec2(0.0, -1.0)));
}

#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_threads_panel() {
    let mut reports = Vec::new();
    for pixels_per_point in SCALES {
        bench_panel(pixels_per_point, &mut reports);
    }
    frame_bench::finish(&reports);
}
