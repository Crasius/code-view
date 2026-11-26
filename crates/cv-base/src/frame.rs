//! Frame metrics shared by the frame stats overlay (`cv_shell::frame_stats`) and the headless
//! frame-budget benchmarks (`cv_testkit::frame_bench`): the whole-frame budgets of the target
//! monitors, percentiles of frame times and the font atlas's size. They live in the base crate
//! because the benchmarks' crate sits below the shell and cannot reach the overlay.

/// Whole-frame budgets of the target monitors, in milliseconds.
pub const BUDGET_240_HZ_MS: f32 = 1000.0 / 240.0;
pub const BUDGET_165_HZ_MS: f32 = 1000.0 / 165.0;

/// p50, p95 and p99 of a set of samples.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Percentiles {
    pub p50: f32,
    pub p95: f32,
    pub p99: f32,
}

impl Percentiles {
    pub fn of(samples: impl IntoIterator<Item = f32>) -> Self {
        let mut sorted: Vec<f32> =
            samples.into_iter().filter(|sample| sample.is_finite()).collect();
        if sorted.is_empty() {
            return Self::default();
        }
        sorted.sort_unstable_by(f32::total_cmp);
        let at = |quantile: f32| sorted[((sorted.len() - 1) as f32 * quantile).round() as usize];
        Self { p50: at(0.50), p95: at(0.95), p99: at(0.99) }
    }
}

/// The font atlas's size in texels, for the statistics and the frame benchmarks.
#[expect(
    clippy::redundant_closure_for_method_calls,
    reason = "the suggested method path does not satisfy the closure's higher-ranked lifetime"
)]
pub fn atlas_size(ctx: &egui::Context) -> [usize; 2] {
    ctx.fonts(|fonts| fonts.font_image_size())
}

#[cfg(test)]
mod tests {
    use super::Percentiles;

    #[test]
    fn percentiles_pick_nearest_ranks() {
        let samples = (1..=100).map(|value| value as f32);
        let percentiles = Percentiles::of(samples);
        assert_eq!((percentiles.p50, percentiles.p95, percentiles.p99), (51.0, 95.0, 99.0));
        assert_eq!(Percentiles::of([]), Percentiles::default());
    }
}
