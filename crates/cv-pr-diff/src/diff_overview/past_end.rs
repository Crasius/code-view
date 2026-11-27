//! Scrolling past the end of a file (the setting "Scroll past the end", on by default): the text
//! and the rendered diff hold room below their last row, so the wheel and the scroll keys can take
//! the view on down until that row is the only one left at its top. The room is plain diff
//! background. The overview strip, the diffs' scrollbar, measures the rows alone: its view box
//! rests at the bottom of the strip in the room, and dragging it, End and every jump (a change,
//! cited lines, a thread) go where they went without the room.
//!
//! The diffs get the setting in their inputs; the strip's context menu switches it, and the view
//! hands the switch on as an event for the settings' owner to save.

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
    /// `view_height` tall, with the room past the end when `past_end` is on. Rows shorter than the
    /// view get no room.
    pub fn of(tops: &[f32], view_height: f32, past_end: bool) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Three rows 20 tall, a removal of no height last.
    const TOPS: [f32; 5] = [0.0, 20.0, 40.0, 60.0, 60.0];

    #[test]
    fn the_room_lets_the_last_row_with_a_height_reach_the_top() {
        let ends = Ends::of(&TOPS, 50.0, true);
        assert_eq!(ends, Ends { end: 10.0, max: 40.0, height: 90.0 });
        let off = Ends::of(&TOPS, 50.0, false);
        assert_eq!(off, Ends { end: 10.0, max: 10.0, height: 60.0 }, "off: the rows alone");
    }

    #[test]
    fn rows_shorter_than_the_view_get_no_room() {
        for view_height in [60.0, 400.0] {
            let ends = Ends::of(&TOPS, view_height, true);
            assert_eq!(ends, Ends { end: 0.0, max: 0.0, height: 60.0 }, "{view_height}");
        }
        assert_eq!(Ends::of(&[0.0], 400.0, true), Ends { end: 0.0, max: 0.0, height: 0.0 });
    }
}
