//! Scrolling past the end of a file (the setting "Scroll past the end", on by default): the text
//! and the rendered diff hold room below their last row, so the wheel and the scroll keys can take
//! the view on down until that row is the only one left at its top. The room is plain diff
//! background. The overview strip, the diffs' scrollbar, measures the rows alone: its view box
//! rests at the bottom of the strip in the room, and dragging it, End and every jump (a change,
//! cited lines, a thread) go where they went without the room.
//!
//! The Files tab hands the setting to the diffs once a frame ([`sync`]); the strip's context menu
//! switches it ([`switch`]), and the next frame saves it.

use egui::{Context, Id};

use cv_services::Services;

/// The setting the diffs follow this frame, and a switch made on a strip not saved yet.
#[derive(Debug, Clone, Copy)]
struct State {
    is_on: bool,
    switch: Option<bool>,
}

/// On, as the setting's default, until [`sync`] hands the diffs the saved one.
impl Default for State {
    fn default() -> Self {
        Self { is_on: true, switch: None }
    }
}

fn state_id() -> Id {
    Id::unique("scroll_past_end")
}

fn state(ctx: &Context) -> State {
    ctx.data(|data| data.get_temp(state_id())).unwrap_or_default()
}

/// Once a frame, before the diffs show: a switch made on a strip is saved, and the diffs get the
/// setting.
pub fn sync(ctx: &Context, services: &mut Services) {
    let mut state = state(ctx);
    if let Some(is_on) = state.switch.take() {
        services.settings.scroll_past_end = is_on;
        services.save_settings();
    }
    state.is_on = services.settings.scroll_past_end;
    ctx.data_mut(|data| data.insert_temp(state_id(), state));
}

/// "Scroll past the end" is on, as [`sync`] handed it (or a strip switched it since).
pub fn is_on(ctx: &Context) -> bool {
    state(ctx).is_on
}

/// Switches "Scroll past the end" from a strip's menu: the diffs follow at once, the next frame's
/// [`sync`] saves it.
pub fn switch(ctx: &Context, is_on: bool) {
    ctx.data_mut(|data| data.insert_temp(state_id(), State { is_on, switch: Some(is_on) }));
    ctx.request_repaint();
}

/// The switch a strip made that [`sync`] has not saved yet.
#[cfg(test)]
pub fn pending_switch(ctx: &Context) -> Option<bool> {
    state(ctx).switch
}

/// How far a view can scroll over its rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ends {
    /// The view's top when the rows' bottom meets the view's: where the strip, End and the jumps
    /// stop.
    pub end: f32,
    /// How far the view scrolls: `end`, or with the room past the end, until the last row with a
    /// height is the only one left at the top.
    pub max: f32,
    /// The height the scroll area holds for that: the rows, and the room.
    pub height: f32,
}

impl Ends {
    /// For rows whose tops are `tops` (the last entry is the rows' height) in a view
    /// `view_height` tall, with the setting the diffs follow this frame.
    pub fn of(tops: &[f32], view_height: f32, ctx: &Context) -> Self {
        Self::with(tops, view_height, is_on(ctx))
    }

    /// As [`Self::of`], with `past_end` for the setting. Rows shorter than the view get no room.
    fn with(tops: &[f32], view_height: f32, past_end: bool) -> Self {
        let total = tops.last().copied().unwrap_or(0.0);
        let end = (total - view_height).max(0.0);
        if !past_end || end <= 0.0 {
            return Self { end, max: end, height: total };
        }
        let last = tops.windows(2).rev().find(|pair| pair[1] > pair[0]).map_or(0.0, |pair| pair[0]);
        let max = last.max(end);
        Self { end, max, height: max + view_height }
    }
}

/// Sets "Scroll past the end" for a test's diffs, which no Files tab syncs.
#[cfg(any(test, feature = "test-support"))]
pub fn set_for_test(ctx: &Context, is_on: bool) {
    ctx.data_mut(|data| data.insert_temp(state_id(), State { is_on, switch: None }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three rows 20 tall, a removal of no height last.
    const TOPS: [f32; 5] = [0.0, 20.0, 40.0, 60.0, 60.0];

    #[test]
    fn the_room_lets_the_last_row_with_a_height_reach_the_top() {
        let ends = Ends::with(&TOPS, 50.0, true);
        assert_eq!(ends, Ends { end: 10.0, max: 40.0, height: 90.0 });
        let off = Ends::with(&TOPS, 50.0, false);
        assert_eq!(off, Ends { end: 10.0, max: 10.0, height: 60.0 }, "off: the rows alone");
    }

    #[test]
    fn rows_shorter_than_the_view_get_no_room() {
        for view_height in [60.0, 400.0] {
            let ends = Ends::with(&TOPS, view_height, true);
            assert_eq!(ends, Ends { end: 0.0, max: 0.0, height: 60.0 }, "{view_height}");
        }
        assert_eq!(Ends::with(&[0.0], 400.0, true), Ends { end: 0.0, max: 0.0, height: 0.0 });
    }

    #[test]
    fn a_switch_takes_effect_at_once_and_waits_to_be_saved() {
        let ctx = Context::default();
        assert!(is_on(&ctx), "on by default");
        switch(&ctx, false);
        assert!(!is_on(&ctx));
        assert_eq!(pending_switch(&ctx), Some(false));
    }
}
