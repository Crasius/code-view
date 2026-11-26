//! Frame stats overlay (Ctrl+Shift+F): frame rate, CPU frame and UI-build time, percentiles of the
//! last 240 frames as a sparkline against the 240 Hz and 165 Hz budgets, the monitor's refresh
//! rate, the renderer and vsync, and the memory: the heap's live bytes and the largest caches
//! ([`cv_runtime::cache::memory`]). Samples are recorded on every frame, shown or not, so the
//! numbers are there the moment the overlay opens; recording costs two ring-buffer pushes.
//!
//! egui is reactive: an idle app renders no frames, so the frame rate is "frames in the last
//! second", which drops to the overlay's own slow refresh while nothing moves.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cv_base::frame::{BUDGET_165_HZ_MS, BUDGET_240_HZ_MS, Percentiles, atlas_size};
use cv_base::{platform, theme};
use cv_runtime::cache::memory;
use cv_services::Services;
use egui::{Align2, FontId, Key, Modifiers, Order, Rect, Sense, Stroke, Ui, pos2, vec2};

/// Present with vsync (swap interval 1): at most one frame per monitor refresh, so frames never
/// tear. The frame rate limit ([`crate::frame_pacing`]) holds continuous repaints below that.
pub const VSYNC: bool = true;
/// Frames kept for the percentiles and the sparkline.
pub const HISTORY: usize = 240;
/// The open overlay refreshes this often while the app is otherwise idle.
const IDLE_REFRESH: Duration = Duration::from_millis(250);
/// Re-read the monitor's refresh rate this often (the window may move to another monitor).
const MONITOR_POLL_SECONDS: f64 = 0.5;
const SPARK_HEIGHT: f32 = 40.0;
/// Caches the memory line names, the largest first.
const SHOWN_HOLDERS: usize = 4;
const SPARK_SCALE_MS: f32 = 2.0 * BUDGET_240_HZ_MS;

pub struct FrameStats {
    renderer: Renderer,
    /// eframe's CPU time per frame (UI, tessellation and paint, not the vsync wait), in ms.
    frame_ms: VecDeque<f32>,
    /// Time spent in the app's `ui()`, in ms.
    ui_ms: VecDeque<f32>,
    /// Input time of the recent frames, in seconds.
    times: VecDeque<f64>,
    monitor: Monitor,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Renderer {
    /// "OpenGL" plus the driver's version string.
    pub api: String,
    /// `GL_RENDERER`: the GPU (or software rasterizer) that draws.
    pub device: String,
    pub vendor: String,
    pub is_software: bool,
}

#[derive(Default)]
struct Monitor {
    refresh_hz: Option<u32>,
    checked_at: Option<f64>,
}

impl FrameStats {
    pub fn new(creation: &eframe::CreationContext<'_>) -> Self {
        Self {
            renderer: creation.gl.as_deref().map(Renderer::from_gl).unwrap_or_default(),
            frame_ms: VecDeque::with_capacity(HISTORY),
            ui_ms: VecDeque::with_capacity(HISTORY),
            times: VecDeque::with_capacity(HISTORY),
            monitor: Monitor::default(),
        }
    }

    /// Records this frame (its UI build started at `ui_started`; the frame rate limit held the
    /// previous frame back for `held`), handles Ctrl+Shift+F and paints the overlay when it is on.
    /// Call at the very end of the app's `ui()`.
    pub fn finish(
        &mut self,
        ui: &mut Ui,
        frame: &eframe::Frame,
        ui_started: Instant,
        held: Duration,
        services: &mut Services,
    ) {
        let ui_ms = ui_started.elapsed().as_secs_f32() * 1000.0;
        let ctx = ui.ctx().clone();
        let now = ctx.input(|input| input.time);
        push(&mut self.ui_ms, ui_ms);
        push(&mut self.times, now);
        if let Some(seconds) = frame.info().cpu_usage {
            // The previous frame's whole pass, the frame rate limit's sleep included.
            push(&mut self.frame_ms, (seconds - held.as_secs_f32()).max(0.0) * 1000.0);
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::F)) {
            services.settings.show_frame_stats = !services.settings.show_frame_stats;
            services.save_settings();
        }
        if !services.settings.show_frame_stats {
            return;
        }
        let is_due =
            self.monitor.checked_at.is_none_or(|checked| now - checked >= MONITOR_POLL_SECONDS);
        if is_due {
            self.monitor.refresh_hz = platform::monitor_refresh_hz(frame);
            self.monitor.checked_at = Some(now);
        }
        self.show(&ctx, now, services.settings.frame_rate);
        ctx.request_repaint_after(IDLE_REFRESH);
    }

    fn show(&self, ctx: &egui::Context, now: f64, limit: review_store::FrameRateLimit) {
        let palette = theme::palette(ctx);
        let fps = self.times.iter().filter(|time| now - **time <= 1.0).count();
        let interval_ms = self
            .times
            .iter()
            .rev()
            .take(2)
            .copied()
            .reduce(|newer, older| newer - older)
            .map(|seconds| seconds as f32 * 1000.0);
        let dt_ms = ctx.input(|input| input.unstable_dt) * 1000.0;
        let frame = Percentiles::of(self.frame_ms.iter().copied());
        let ui = Percentiles::of(self.ui_ms.iter().copied());
        let ppp = ctx.pixels_per_point();
        let atlas = atlas_size(ctx);
        let refresh = self.monitor.refresh_hz.map_or("unknown".to_owned(), |hz| format!("{hz} Hz"));
        let vsync = if VSYNC { "vsync on" } else { "vsync off" };
        let limit =
            limit.cap().map_or("limit: display".to_owned(), |fps| format!("limit {fps} fps"));
        let lines = [
            format!(
                "{fps:>3} fps   last interval {}   dt {dt_ms:.2} ms",
                interval_ms.map_or("–".to_owned(), |ms| format!("{ms:.2} ms"))
            ),
            format!(
                "CPU frame  p50 {:.2}  p95 {:.2}  p99 {:.2} ms",
                frame.p50, frame.p95, frame.p99
            ),
            format!("UI build   p50 {:.2}  p95 {:.2}  p99 {:.2} ms", ui.p50, ui.p95, ui.p99),
            format!("budget {BUDGET_240_HZ_MS:.2} ms @240 Hz · {BUDGET_165_HZ_MS:.2} ms @165 Hz"),
            format!(
                "monitor {refresh} · scale {:.0} % · font atlas {}×{}",
                ppp * 100.0,
                atlas[0],
                atlas[1]
            ),
            format!("{} · {vsync} · {limit}", self.renderer.api),
            format!(
                "{} · {}{}",
                self.renderer.device,
                self.renderer.vendor,
                if self.renderer.is_software { "  (SOFTWARE)" } else { "" }
            ),
            format!("heap {} live", megabytes(platform::heap::live_bytes())),
            memory_line(&memory::holders()),
        ];
        egui::Area::new(egui::Id::unique("frame_stats_overlay"))
            .order(Order::Debug)
            .anchor(Align2::RIGHT_TOP, vec2(-12.0, 52.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(palette.surface.gamma_multiply(0.94))
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(6)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        let font = FontId::monospace(11.5);
                        for line in &lines {
                            ui.label(
                                egui::RichText::new(line).font(font.clone()).color(palette.text),
                            );
                        }
                        ui.add_space(6.0);
                        self.sparkline(ui, &palette);
                    });
            });
    }

    /// One column per frame (CPU frame time), with the 240 Hz and 165 Hz budgets as lines.
    fn sparkline(&self, ui: &mut Ui, palette: &theme::Palette) {
        let (rect, _) = ui.allocate_exact_size(vec2(HISTORY as f32, SPARK_HEIGHT), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2, palette.surface_subtle);
        let y_of = |ms: f32| rect.bottom() - (ms / SPARK_SCALE_MS).clamp(0.0, 1.0) * rect.height();
        let start = rect.left() + (HISTORY - self.frame_ms.len()) as f32;
        for (index, ms) in self.frame_ms.iter().enumerate() {
            let color = if *ms <= BUDGET_240_HZ_MS {
                palette.success
            } else if *ms <= BUDGET_165_HZ_MS {
                palette.warning
            } else {
                palette.error
            };
            let x = start + index as f32;
            painter.rect_filled(
                Rect::from_min_max(pos2(x, y_of(*ms)), pos2(x + 1.0, rect.bottom())),
                0,
                color,
            );
        }
        for (budget, color) in
            [(BUDGET_240_HZ_MS, palette.text_secondary), (BUDGET_165_HZ_MS, palette.warning)]
        {
            painter.hline(
                rect.x_range(),
                y_of(budget),
                Stroke::new(1.0, color.gamma_multiply(0.8)),
            );
        }
    }
}

impl Renderer {
    fn from_gl(gl: &eframe::glow::Context) -> Self {
        let platform::gl::DriverStrings { version, renderer: device, vendor } =
            platform::gl::driver_strings(gl);
        Self {
            api: format!("OpenGL {version}"),
            is_software: is_software_rasterizer(&device),
            device,
            vendor,
        }
    }
}

fn megabytes(bytes: usize) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

/// The largest caches and their bytes: "images 12.0 MB · markdown 1.5 MB · …".
fn memory_line(holders: &[(&str, usize)]) -> String {
    let shown: Vec<String> = holders
        .iter()
        .take(SHOWN_HOLDERS)
        .map(|(name, bytes)| format!("{name} {}", megabytes(*bytes)))
        .collect();
    shown.join(" · ")
}

/// Microsoft's GDI fallback, Mesa's `llvmpipe`/`softpipe` and SwiftShader draw on the CPU.
pub fn is_software_rasterizer(device: &str) -> bool {
    let device = device.to_ascii_lowercase();
    ["gdi generic", "llvmpipe", "softpipe", "swiftshader", "basic render"]
        .iter()
        .any(|software| device.contains(software))
}

fn push<T>(samples: &mut VecDeque<T>, sample: T) {
    if samples.len() == HISTORY {
        samples.pop_front();
    }
    samples.push_back(sample);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_last_frames_only() {
        let mut samples = VecDeque::new();
        for value in 0..HISTORY + 10 {
            push(&mut samples, value);
        }
        assert_eq!(samples.len(), HISTORY);
        assert_eq!(samples.front(), Some(&10));
    }

    #[test]
    fn recognizes_software_rasterizers() {
        assert!(is_software_rasterizer("GDI Generic"));
        assert!(is_software_rasterizer("llvmpipe (LLVM 17.0.6, 256 bits)"));
        assert!(!is_software_rasterizer("NVIDIA GeForce RTX 4070/PCIe/SSE2"));
    }
}
