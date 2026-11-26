//! Frame-budget benchmark of the overview's discussion at 4K with a 500-thread review (one to
//! eight comments, Markdown, code excerpts, suggestions; resolved ones collapsed), scrolled in
//! the main column. `#[ignore]`d; see `cv_testkit::frame_bench` for how to run it.

use egui::{Margin, pos2, vec2};
use review_core::ReviewThread;

use super::cards;
use cv_pr_threads::{ThreadsUi, samples};
use cv_testkit::frame_bench::{Bench, PATTERNS, Report, SCALES};
use cv_widgets::scroll;

const THREADS: usize = 500;
/// The side column beside the main one, as the overview's default leaves it.
const SIDE_WIDTH: f32 = 420.0;

struct Scene {
    threads: Vec<ReviewThread>,
    ui: ThreadsUi,
}

fn bench_timeline(pixels_per_point: f32, reports: &mut Vec<Report>) {
    let threads = samples::review(THREADS);
    let scene = Scene { ui: samples::review_ui(&threads), threads };
    let mut bench = Bench::new(pixels_per_point, scene, |ui, scene: &mut Scene| {
        scene.ui.begin_frame_for_tests(ui.ctx(), &scene.threads, 1);
        let main_width = ui.available_width() - SIDE_WIDTH;
        scroll::page("overview", Margin::same(16)).show(ui, |ui| {
            ui.set_width(main_width);
            let visible: Vec<&ReviewThread> = scene.threads.iter().collect();
            cards(ui, &visible, &[], &mut scene.ui);
        });
    });
    let rect = bench.ctx.content_rect();
    let pointer = pos2(rect.left() + 400.0, rect.center().y);
    bench.warm_up(pointer);
    let scale = pixels_per_point * 100.0;
    for (pattern, speed) in PATTERNS {
        let name = format!("timeline 500 threads @{scale:.0}% {pattern}");
        reports.push(bench.scroll_best(&name, pointer, vec2(0.0, -speed)));
    }
}

#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_timeline() {
    let mut reports = Vec::new();
    for pixels_per_point in SCALES {
        bench_timeline(pixels_per_point, &mut reports);
    }
    for report in &reports {
        report.print();
    }
    for report in &reports {
        report.assert_within_budget();
    }
}
