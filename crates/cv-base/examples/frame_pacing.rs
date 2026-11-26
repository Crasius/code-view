//! Frame pacing probe: opens a window that repaints continuously with a screenful of moving
//! monospace text (like a scrolling diff), records the interval between frames and eframe's CPU
//! frame time, prints the distribution and closes itself. Compares renderers on this machine's
//! monitor without looking at the screen:
//!
//! ```text
//! cargo run --release -p cv-base --example frame_pacing -- glow 6
//! cargo run --release -p cv-base --example frame_pacing -- wgpu 6
//! ```
//!
//! The window must stay visible (not minimized, not covered) for vsync to pace it.

#[path = "../src/platform/gl.rs"]
#[expect(
    dead_code,
    reason = "the probe prints version and renderer; the vendor is for the frame stats overlay"
)]
mod gl;

use std::time::{Duration, Instant};

use egui::{Align2, Color32, FontId, pos2};

const WARMUP: Duration = Duration::from_millis(1000);

fn main() -> eframe::Result<()> {
    let mut args = std::env::args().skip(1);
    let backend = args.next().unwrap_or_else(|| "glow".to_owned());
    let seconds: f64 = args.next().and_then(|value| value.parse().ok()).unwrap_or(6.0);
    let renderer = match backend.as_str() {
        "wgpu" => eframe::Renderer::Wgpu,
        _ => eframe::Renderer::Glow,
    };
    let options = eframe::NativeOptions {
        renderer,
        multisampling: 0,
        depth_buffer: 0,
        stencil_buffer: 0,
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("frame pacing probe ({backend})"))
            .with_inner_size([1600.0, 1000.0]),
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "frame pacing probe",
        options,
        Box::new(move |creation| Ok(Box::new(Probe::new(creation, backend, seconds)))),
    )
}

struct Probe {
    backend: String,
    adapter: String,
    started: Instant,
    seconds: f64,
    last_frame: Option<Instant>,
    intervals_ms: Vec<f32>,
    cpu_ms: Vec<f32>,
    is_done: bool,
}

impl Probe {
    fn new(creation: &eframe::CreationContext<'_>, backend: String, seconds: f64) -> Self {
        let adapter = if let Some(gl) = creation.gl.as_deref() {
            let gl::DriverStrings { version, renderer, .. } = gl::driver_strings(gl);
            format!("OpenGL {version} · {renderer}")
        } else if let Some(state) = &creation.wgpu_render_state {
            let info = state.adapter.get_info();
            format!("wgpu {:?} · {} · {:?}", info.backend, info.name, info.device_type)
        } else {
            "unknown".to_owned()
        };
        Self {
            backend,
            adapter,
            started: Instant::now(),
            seconds,
            last_frame: None,
            intervals_ms: Vec::new(),
            cpu_ms: Vec::new(),
            is_done: false,
        }
    }

    #[expect(clippy::print_stdout, reason = "the probe's result is its printed report")]
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "frame counts of a few seconds and a rounded, non-negative quantile index"
    )]
    fn report(&self) {
        let mut intervals = self.intervals_ms.clone();
        intervals.sort_unstable_by(f32::total_cmp);
        let mut cpu = self.cpu_ms.clone();
        cpu.sort_unstable_by(f32::total_cmp);
        let at = |sorted: &[f32], quantile: f32| {
            sorted
                .get(((sorted.len().max(1) - 1) as f32 * quantile).round() as usize)
                .copied()
                .unwrap_or(0.0)
        };
        let median = at(&intervals, 0.5);
        let mean = intervals.iter().sum::<f32>() / intervals.len().max(1) as f32;
        let deviation = (intervals.iter().map(|ms| (ms - mean).powi(2)).sum::<f32>()
            / intervals.len().max(1) as f32)
            .sqrt();
        let missed = intervals.iter().filter(|ms| **ms > median * 1.5).count();
        println!("backend   {}", self.backend);
        println!("adapter   {}", self.adapter);
        println!(
            "frames    {} in {:.1} s -> {:.1} fps (refresh ~{:.0} Hz from the median interval)",
            intervals.len(),
            self.seconds,
            1000.0 / mean,
            1000.0 / median
        );
        println!(
            "interval  p1 {:.2}  p50 {:.2}  p99 {:.2}  max {:.2} ms, stddev {:.3} ms, {} frames > 1.5x median",
            at(&intervals, 0.01),
            median,
            at(&intervals, 0.99),
            intervals.last().copied().unwrap_or(0.0),
            deviation,
            missed
        );
        println!(
            "cpu       p50 {:.2}  p95 {:.2}  p99 {:.2} ms",
            at(&cpu, 0.5),
            at(&cpu, 0.95),
            at(&cpu, 0.99)
        );
    }
}

impl eframe::App for Probe {
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "screen rows and line numbers: small, non-negative, cut to whole rows on purpose"
    )]
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let now = Instant::now();
        let elapsed = now - self.started;
        if elapsed > WARMUP {
            if let Some(last) = self.last_frame {
                self.intervals_ms.push((now - last).as_secs_f32() * 1000.0);
            }
            if let Some(cpu) = frame.info().cpu_usage {
                self.cpu_ms.push(cpu * 1000.0);
            }
        }
        self.last_frame = Some(now);
        let rect = ui.max_rect();
        let painter = ui.painter();
        painter.rect_filled(rect, 0, Color32::from_gray(250));
        let font = FontId::monospace(13.0);
        let line_height = 20.0;
        let scroll = (elapsed.as_secs_f32() * 600.0) % line_height;
        let first = (elapsed.as_secs_f32() * 600.0 / line_height) as usize;
        let rows = (rect.height() / line_height) as usize + 2;
        for row in 0..rows {
            let y = rect.top() + row as f32 * line_height - scroll;
            let index = first + row;
            let text = format!(
                "{index:>6}  let value_{index} = compute_total(&input[{index}..], \"label\", {index} * 3); // running total"
            );
            painter.text(
                pos2(rect.left() + 8.0, y),
                Align2::LEFT_TOP,
                text,
                font.clone(),
                Color32::from_gray(30),
            );
        }
        if !self.is_done && elapsed.as_secs_f64() > WARMUP.as_secs_f64() + self.seconds {
            self.is_done = true;
            self.report();
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui.ctx().request_repaint();
    }
}
