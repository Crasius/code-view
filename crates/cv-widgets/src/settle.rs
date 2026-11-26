//! Heights that settle instead of jumping. When a block changes height because of a hand-off (a
//! comment composer turning into the posted comment, the posted comment turning into the one the
//! platform sent back), the space it takes eases from the old height to the new one over
//! [`DURATION`], so whatever sits below slides instead of jumping. Every other change (typing,
//! a window resize) is taken at once. Reduced motion takes every change at once.

use cv_base::theme::motion;
use egui::{Context, Id, Rect, Ui, UiBuilder, vec2};

/// How long a height takes to settle.
pub const DURATION: f32 = 0.15;

/// One block's height and the ease it is in, if any.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tween {
    /// The height the content has now.
    target: f32,
    /// The height the ease started from, and when.
    from: f32,
    start: f64,
}

impl Tween {
    /// A block whose content is `height` tall, shown at that height.
    pub fn at(height: f32) -> Self {
        Self { target: height, from: height, start: f64::NEG_INFINITY }
    }

    /// The height to give the block at `now`.
    pub fn shown(&self, now: f64, duration: f32) -> f32 {
        if duration <= 0.0 {
            return self.target;
        }
        let t = ((now - self.start) / f64::from(duration)) as f32;
        if t >= 1.0 {
            return self.target;
        }
        self.from + (self.target - self.from) * motion::ease_out_cubic(t)
    }

    /// Still easing at `now`.
    pub fn is_moving(&self, now: f64, duration: f32) -> bool {
        duration > 0.0
            && (self.target - self.from).abs() > 0.5
            && now - self.start < f64::from(duration)
    }

    /// The content is `height` tall from `now` on: with `animate` the block eases there from the
    /// height it is shown at, else it takes it at once. Returns true when an ease started.
    pub fn retarget(&mut self, height: f32, animate: bool, now: f64, duration: f32) -> bool {
        if (height - self.target).abs() <= 0.5 {
            return false;
        }
        let shown = self.shown(now, duration);
        let eases = animate && duration > 0.0 && (height - shown).abs() > 0.5;
        *self =
            if eases { Self { target: height, from: shown, start: now } } else { Self::at(height) };
        eases
    }
}

/// The duration of a settle in this context: zero under reduced motion.
pub fn duration(ctx: &Context) -> f32 {
    motion::duration(ctx, DURATION)
}

/// Lays `content` out at its own height in the full width of `ui`, and takes up the settled
/// height for it: a change while `animate` is true eases, any other change is taken at once.
/// While it eases the content is clipped to the settled height.
pub fn height<R>(ui: &mut Ui, id: Id, animate: bool, content: impl FnOnce(&mut Ui) -> R) -> R {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|input| input.time);
    let duration = duration(&ctx);
    let state_id = id.with("settle");
    let before: Option<Tween> = ctx.data(|data| data.get_temp(state_id));
    let top_left = ui.cursor().min;
    let width = ui.available_width();
    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt(id)
            .max_rect(Rect::from_min_size(top_left, vec2(width, f32::INFINITY)))
            .layout(*ui.layout()),
    );
    let clipped = before
        .filter(|tween| tween.is_moving(now, duration))
        .map(|tween| tween.shown(now, duration));
    if let Some(shown) = clipped {
        child.set_clip_rect(
            child.clip_rect().intersect(Rect::from_min_size(top_left, vec2(width, shown))),
        );
    }
    let inner = content(&mut child);
    let measured = child.min_rect().height();
    let mut tween = before.unwrap_or_else(|| Tween::at(measured));
    let started = tween.retarget(measured, animate, now, duration);
    let shown = tween.shown(now, duration);
    if tween.is_moving(now, duration) {
        ctx.request_repaint();
    }
    // An ease that starts this pass was painted unclipped: the next pass clips it.
    if started
        && clipped.is_none()
        && ctx.current_pass_index() + 1 < ctx.options(|options| options.max_passes.get())
    {
        ctx.request_discard("a settling height started to ease");
    }
    ctx.data_mut(|data| data.insert_temp(state_id, tween));
    ui.advance_cursor_after_rect(Rect::from_min_size(
        top_left,
        vec2(child.min_rect().width().max(0.0), shown),
    ));
    inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settle_eases_from_the_shown_height_and_lands_exactly() {
        let mut tween = Tween::at(200.0);
        assert!(tween.retarget(120.0, true, 10.0, DURATION), "a hand-off eases");
        assert_eq!(tween.shown(10.0, DURATION), 200.0, "it starts where the block was");
        let middle = tween.shown(10.075, DURATION);
        assert!(middle < 200.0 && middle > 120.0, "halfway it is between: {middle}");
        assert_eq!(tween.shown(10.15, DURATION), 120.0, "it lands on the content's height");
        assert!(!tween.is_moving(10.151, DURATION));
    }

    #[test]
    fn other_changes_and_reduced_motion_take_the_height_at_once() {
        let mut tween = Tween::at(200.0);
        assert!(!tween.retarget(240.0, false, 1.0, DURATION), "typing grows the block at once");
        assert_eq!(tween.shown(1.0, DURATION), 240.0);
        assert!(!tween.retarget(100.0, true, 2.0, 0.0), "reduced motion");
        assert_eq!(tween.shown(2.0, 0.0), 100.0);
    }

    #[test]
    fn a_new_target_mid_ease_continues_from_where_the_block_is() {
        let mut tween = Tween::at(200.0);
        tween.retarget(100.0, true, 0.0, DURATION);
        let shown = tween.shown(0.05, DURATION);
        tween.retarget(150.0, true, 0.05, DURATION);
        assert_eq!(tween.shown(0.05, DURATION), shown, "no jump when the target moves");
    }
}
