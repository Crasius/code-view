//! Headless frame-budget benchmarks: drives a view frame by frame at a 4K physical size and
//! measures, per frame, the UI build (the view's code, timed inside the frame) and tessellation
//! (what eframe does with the shapes before handing them to the GPU), plus the vertex count and
//! the font atlas size the GPU receives. A scroll benchmark measures each view with
//! [`Bench::scroll_patterns`] and ends with [`finish`].
//!
//! The benchmarks themselves are `#[ignore]`d tests next to the views they measure. Run them in
//! release mode, single-threaded so they don't compete for cores, with output shown:
//!
//! ```text
//! cargo test --release --workspace frame_budget -- --ignored --nocapture --test-threads=1
//! ```

use std::time::{Duration, Instant};

use cv_base::frame::{BUDGET_165_HZ_MS, BUDGET_240_HZ_MS, Percentiles, atlas_size};
use cv_base::theme;
use egui::{
    Event, Modifiers, MouseWheelSource, MouseWheelUnit, Pos2, RawInput, Rect, TouchPhase, Ui, Vec2,
    ViewportId, vec2,
};
use review_store::ThemePreference;

/// The physical size of the target monitors' largest mode.
const PHYSICAL_4K: Vec2 = vec2(3840.0, 2160.0);
/// Typical Windows scaling on a 4K monitor: 150 % and 200 %.
pub const SCALES: [f32; 2] = [1.5, 2.0];
/// Targets for UI build + tessellation, leaving the rest of the 4.17 ms (240 Hz) frame to the GPU submit.
const TARGET_P95_MS: f32 = 3.0;
const TARGET_P99_MS: f32 = 4.0;
const STEP_SECONDS: f32 = 1.0 / 240.0;
const WARMUP_FRAMES: usize = 30;

/// The view a bench draws each frame.
type View<'a, State> = Box<dyn FnMut(&mut Ui, &mut State) + 'a>;

/// An egui context driven frame by frame, as eframe drives it, with the view timed inside the
/// frame. AccessKit is off, as in eframe until an assistive client (a screen reader or another
/// UI Automation client) connects; [`Bench::with_accesskit`] measures that case.
pub struct Bench<'a, State> {
    pub ctx: egui::Context,
    pub state: State,
    app: View<'a, State>,
    input: RawInput,
    pixels_per_point: f32,
    ui_time: Duration,
    time: f64,
}

/// What one frame cost.
#[derive(Debug, Clone, Copy)]
struct Sample {
    ui_ms: f32,
    tessellate_ms: f32,
    vertices: usize,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub name: String,
    pub frames: usize,
    pub ui: Percentiles,
    pub tessellate: Percentiles,
    /// UI build + tessellation, per frame.
    pub total: Percentiles,
    pub max_ms: f32,
    pub vertices_p50: usize,
    pub vertices_max: usize,
    pub atlas: [usize; 2],
    /// Scroll scenarios must meet the targets; one-off events (zoom steps) are only reported.
    pub is_gated: bool,
}

impl Report {
    #[expect(clippy::print_stdout, reason = "the benchmarks print their report under --nocapture")]
    pub fn print(&self) {
        let budget = |ms: f32| {
            let share = |budget: f32| ms / budget * 100.0;
            format!(
                "{:>3.0}% of 240 Hz, {:>3.0}% of 165 Hz",
                share(BUDGET_240_HZ_MS),
                share(BUDGET_165_HZ_MS)
            )
        };
        println!(
            "{:<44} {:>4} frames: total p50 {:>5.2}  p95 {:>5.2}  p99 {:>5.2}  max {:>6.2} ms | ui p95 {:>5.2}  tess p95 {:>5.2} | p95 = {} | vertices p50 {:>6}  max {:>6} | atlas {}x{}",
            self.name,
            self.frames,
            self.total.p50,
            self.total.p95,
            self.total.p99,
            self.max_ms,
            self.ui.p95,
            self.tessellate.p95,
            budget(self.total.p95),
            self.vertices_p50,
            self.vertices_max,
            self.atlas[0],
            self.atlas[1],
        );
    }

    /// Fails the benchmark when the frame does not leave the GPU its share of a 240 Hz frame.
    pub fn assert_within_budget(&self) {
        if !self.is_gated {
            return;
        }
        assert!(
            self.total.p95 <= TARGET_P95_MS && self.total.p99 <= TARGET_P99_MS,
            "{}: p95 {:.2} ms / p99 {:.2} ms exceed the {TARGET_P95_MS} / {TARGET_P99_MS} ms targets",
            self.name,
            self.total.p95,
            self.total.p99
        );
    }
}

impl<'a, State> Bench<'a, State> {
    /// A 3840×2160 physical window at `pixels_per_point`, with the app's fonts and light theme.
    pub fn new(
        pixels_per_point: f32,
        state: State,
        app: impl FnMut(&mut Ui, &mut State) + 'a,
    ) -> Self {
        let ctx = egui::Context::default();
        theme::fonts::install(&ctx);
        theme::activate(&ctx, ThemePreference::Light);
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, PHYSICAL_4K / pixels_per_point)),
            ..RawInput::default()
        };
        if let Some(viewport) = input.viewports.get_mut(&ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(pixels_per_point);
        }
        let mut bench = Self {
            ctx,
            state,
            app: Box::new(app),
            input,
            pixels_per_point,
            ui_time: Duration::ZERO,
            time: 0.0,
        };
        bench.step(Vec::new());
        bench
    }

    /// Builds the AccessKit tree from the next frame on, as while an assistive client is connected.
    #[must_use]
    pub fn with_accesskit(self) -> Self {
        self.ctx.enable_accesskit();
        self
    }

    /// Runs one frame with `events`, at the next 240 Hz tick.
    fn step(&mut self, events: Vec<Event>) -> Sample {
        let input = RawInput {
            time: Some(self.time),
            predicted_dt: STEP_SECONDS,
            events,
            ..self.input.clone()
        };
        self.time += f64::from(STEP_SECONDS);
        let Self { ctx, state, app, ui_time, .. } = self;
        let mut output = ctx.run_ui(input, |ui| {
            // The margin kittest's harness and eframe's central panel give a view.
            egui::Frame::central_panel(ui.style()).outer_margin(8.0).inner_margin(0.0).show(
                ui,
                |ui| {
                    let started = Instant::now();
                    app(ui, state);
                    *ui_time = started.elapsed();
                },
            );
        });
        let shapes = std::mem::take(&mut output.shapes);
        let started = Instant::now();
        let primitives = self.ctx.tessellate(shapes, output.pixels_per_point);
        let tessellate_ms = started.elapsed().as_secs_f32() * 1000.0;
        output.drop_without_applying_deltas();
        let vertices = primitives
            .iter()
            .map(|primitive| match &primitive.primitive {
                egui::epaint::Primitive::Mesh(mesh) => mesh.vertices.len(),
                egui::epaint::Primitive::Callback(_) => 0,
            })
            .sum();
        Sample { ui_ms: self.ui_time.as_secs_f32() * 1000.0, tessellate_ms, vertices }
    }

    /// Lets fonts load and caches fill, with the pointer at `pointer`.
    pub fn warm_up(&mut self, pointer: Pos2) {
        self.step(vec![Event::PointerMoved(pointer)]);
        for _ in 0..WARMUP_FRAMES {
            self.step(Vec::new());
        }
    }

    /// `frames` frames, each scrolled by `delta` points (negative scrolls down) with the pointer at
    /// `pointer`. The direction flips every [`SWEEP_FRAMES`] frames, so even a fling through a
    /// short list keeps showing new rows instead of resting at its end.
    fn scroll(&mut self, name: &str, pointer: Pos2, delta: Vec2, frames: usize) -> Report {
        let wheel = |delta: Vec2, phase: TouchPhase| Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta,
            phase,
            modifiers: Modifiers::NONE,
            source: MouseWheelSource::Wheel,
        };
        self.step(vec![Event::PointerMoved(pointer), wheel(Vec2::ZERO, TouchPhase::Start)]);
        let samples: Vec<Sample> = (0..frames)
            .map(|frame| {
                let direction = if (frame / SWEEP_FRAMES).is_multiple_of(2) { 1.0 } else { -1.0 };
                self.step(vec![wheel(delta * direction, TouchPhase::Move)])
            })
            .collect();
        self.step(vec![wheel(Vec2::ZERO, TouchPhase::End)]);
        self.report(name, &samples)
    }

    /// `RUNS` scroll runs of `FRAMES` frames; keeps the run with the lowest p95. Other processes on the
    /// machine (compilers, indexers) only ever add time, so the best run is the closest to what
    /// the view itself costs.
    pub fn scroll_best(&mut self, name: &str, pointer: Pos2, delta: Vec2) -> Report {
        let mut runs: Vec<Report> =
            (0..RUNS).map(|_| self.scroll(name, pointer, delta, FRAMES)).collect();
        runs.sort_by(|a, b| a.total.p95.total_cmp(&b.total.p95));
        runs.swap_remove(0)
    }

    /// [`Self::scroll_best`] once per entry of [`PATTERNS`], named "{view} @{scale}% {pattern}";
    /// each frame scrolls by `direction * speed` (`vec2(0.0, -1.0)` scrolls down).
    pub fn scroll_patterns(&mut self, view: &str, pointer: Pos2, direction: Vec2) -> Vec<Report> {
        let scale = self.pixels_per_point * 100.0;
        PATTERNS
            .iter()
            .map(|(pattern, speed)| {
                let name = format!("{view} @{scale:.0}% {pattern}");
                self.scroll_best(&name, pointer, direction * *speed)
            })
            .collect()
    }

    /// One frame per entry of `events`, for one-off events; reported, not gated.
    pub fn frames(
        &mut self,
        name: &str,
        events: Vec<Vec<Event>>,
        mut before: impl FnMut(&egui::Context, usize),
    ) -> Report {
        let samples: Vec<Sample> = events
            .into_iter()
            .enumerate()
            .map(|(index, events)| {
                before(&self.ctx, index);
                self.step(events)
            })
            .collect();
        Report { is_gated: false, ..self.report(name, &samples) }
    }

    fn report(&self, name: &str, samples: &[Sample]) -> Report {
        let totals = samples.iter().map(|sample| sample.ui_ms + sample.tessellate_ms);
        let mut vertices: Vec<usize> = samples.iter().map(|sample| sample.vertices).collect();
        vertices.sort_unstable();
        Report {
            name: name.to_owned(),
            frames: samples.len(),
            ui: Percentiles::of(samples.iter().map(|sample| sample.ui_ms)),
            tessellate: Percentiles::of(samples.iter().map(|sample| sample.tessellate_ms)),
            total: Percentiles::of(totals.clone()),
            max_ms: totals.fold(0.0, f32::max),
            vertices_p50: vertices.get(vertices.len() / 2).copied().unwrap_or(0),
            vertices_max: vertices.last().copied().unwrap_or(0),
            atlas: atlas_size(&self.ctx),
            is_gated: true,
        }
    }
}

/// Prints every report, then fails on the first one over budget.
pub fn finish(reports: &[Report]) {
    for report in reports {
        report.print();
    }
    for report in reports {
        report.assert_within_budget();
    }
}

/// Frames per scroll run: two seconds at 240 Hz.
const FRAMES: usize = 480;
/// Runs per scroll pattern.
const RUNS: usize = 3;
/// Frames before a scroll run turns around.
const SWEEP_FRAMES: usize = 120;

/// The scroll patterns every view is measured with: a smooth-wheel glide (~25 px per 240 Hz
/// frame, about 6000 px/s) and a fling that shows entirely new rows on every frame.
pub const PATTERNS: [(&str, f32); 2] = [("glide", 25.0), ("fling", 1600.0)];
