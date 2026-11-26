//! Motion tokens and helpers: short eased transitions for hover, press, selection and content
//! reveals. Every helper collapses to an instant change when the OS asks for reduced motion, and
//! repaints are only requested while something is actually moving, so an idle app stays idle.

use egui::{Color32, Context, Id};

use crate::platform;

/// Hover and press feedback.
pub const FAST: f32 = 0.12;
/// Selection changes, sliding indicators.
pub const MEDIUM: f32 = 0.2;
/// Content fading in after it loaded.
pub const REVEAL: f32 = 0.22;

const REDUCED_ID: &str = "review.motion.reduced";

/// Remembers the OS "show animations" preference for this context. Called once at start-up.
pub fn install(ctx: &Context) {
    let is_reduced = platform::prefers_reduced_motion();
    set_reduced(ctx, is_reduced);
}

/// Stores the reduced-motion preference; tests use it to stand in for the OS setting.
pub fn set_reduced(ctx: &Context, is_reduced: bool) {
    ctx.data_mut(|data| data.insert_temp(Id::unique(REDUCED_ID), is_reduced));
}

pub fn is_reduced(ctx: &Context) -> bool {
    ctx.data(|data| data.get_temp(Id::unique(REDUCED_ID))).unwrap_or(false)
}

/// The duration to use for `seconds`, or zero under reduced motion.
pub fn duration(ctx: &Context, seconds: f32) -> f32 {
    if is_reduced(ctx) { 0.0 } else { seconds }
}

/// Eased 0..1 progress towards `value`; 1 means fully on.
pub fn toggle(ctx: &Context, id: Id, value: bool, seconds: f32) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, value, duration(ctx, seconds), ease_out_cubic)
}

/// A value that glides to `target` instead of jumping (sliding indicators, positions).
pub fn glide(ctx: &Context, id: Id, target: f32, seconds: f32) -> f32 {
    ctx.animate_value_with_time(id, target, duration(ctx, seconds))
}

/// Opacity for content that replaces a placeholder: 0 while `is_ready` is false, then eases to 1.
/// Content that is ready the first time it is seen shows at once (nothing to replace).
pub fn reveal(ctx: &Context, id: Id, is_ready: bool) -> f32 {
    toggle(ctx, id.with("reveal"), is_ready, REVEAL)
}

/// Opacity of a page or tab that was entered `elapsed` seconds ago; requests repaints until done.
pub fn entrance(ctx: &Context, elapsed: f64) -> f32 {
    let total = duration(ctx, REVEAL);
    if total <= 0.0 || elapsed >= f64::from(total) {
        return 1.0;
    }
    ctx.request_repaint();
    ease_out_cubic(elapsed as f32 / total)
}

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Slow at both ends, as CSS `ease-in-out`: for a move that starts and stops on its own.
pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (2.0 - 2.0 * t).powi(3) / 2.0 }
}

/// Linear blend between two colors in gamma space, `t` = 0 gives `from`.
pub fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    from.lerp_to_gamma(to, t.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::ease_out_cubic;

    #[test]
    fn easing_starts_fast_and_lands_exactly() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert!(ease_out_cubic(0.5) > 0.8);
        assert_eq!(ease_out_cubic(2.0), 1.0);
    }
}
