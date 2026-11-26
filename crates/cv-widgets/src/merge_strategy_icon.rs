//! Merge strategy glyphs, painted: commit graphs drawn like Phosphor's git icons (round caps,
//! Bold-weight strokes, commits as dots, the resulting commit as a ring), so they sit next to
//! `GIT_MERGE` and `GIT_COMMIT` without looking borrowed. Phosphor has no glyph per strategy.
//!
//! * Merge commit: two lanes joining at a ring.
//! * Squash: three commits collapsing into one ring.
//! * Rebase / fast-forward: commits replayed on one straight line, the new tip a ring.
//! * Semi-linear: a branch leaving the tip, its commits in line, rejoining at a merge ring.
//!
//! Every glyph is drawn on a 16-unit grid centred in a `size` square. The stroke follows the icon
//! weight rules of [`cv_base::theme::icons`]: Bold-like (1.5 units) below 18 px and in the dark
//! theme, Regular-like (1.1 units) above. A glyph is never the only clue: callers put the
//! strategy's name next to it.

use cv_base::theme::Palette;
use egui::epaint::CubicBezierShape;
use egui::{Color32, Painter, Pos2, Shape, Stroke, vec2};
use review_core::MergeStrategy;

/// Grid units of the drawing.
const GRID: f32 = 16.0;
/// Commits (filled) and the resulting commit (ring), in grid units.
const DOT: f32 = 1.45;
const RING: f32 = 2.2;

/// Paints `strategy`'s glyph in `color`, centred on `center` in a `size` square. `ground` is the
/// fill behind it: rings are punched out of the lines running into them with it.
pub fn paint(
    painter: &Painter,
    palette: &Palette,
    center: Pos2,
    size: f32,
    strategy: MergeStrategy,
    color: Color32,
    ground: Color32,
) {
    let scale = size / GRID;
    let at = |x: f32, y: f32| center + vec2(x - GRID / 2.0, y - GRID / 2.0) * scale;
    let width = if palette.is_dark || size < 18.0 { 1.5 } else { 1.1 } * scale;
    let stroke = Stroke::new(width, color);
    let line = |from: Pos2, to: Pos2| painter.line_segment([from, to], stroke);
    let curve = |from: Pos2, c1: Pos2, c2: Pos2, to: Pos2| {
        painter.add(Shape::CubicBezier(CubicBezierShape::from_points_stroke(
            [from, c1, c2, to],
            false,
            Color32::TRANSPARENT,
            stroke,
        )));
    };
    let dot = |at: Pos2| painter.circle_filled(at, DOT * scale, color);
    let ring = |at: Pos2| {
        painter.circle_filled(at, RING * scale + width / 2.0, ground);
        painter.circle_stroke(at, RING * scale, stroke);
    };
    match strategy {
        MergeStrategy::Merge => {
            line(at(4.0, 2.5), at(4.0, 15.0));
            curve(at(12.0, 2.5), at(12.0, 8.5), at(8.5, 12.0), at(4.0, 12.0));
            dot(at(4.0, 2.5));
            dot(at(12.0, 2.5));
            ring(at(4.0, 12.0));
        }
        MergeStrategy::Squash => {
            for y in [2.5, 8.0, 13.5] {
                curve(at(2.5, y), at(7.0, y), at(8.0, 8.0), at(12.5, 8.0));
            }
            for y in [2.5, 8.0, 13.5] {
                dot(at(2.5, y));
            }
            ring(at(12.5, 8.0));
        }
        MergeStrategy::Rebase => {
            line(at(1.0, 8.0), at(15.0, 8.0));
            dot(at(3.0, 8.0));
            dot(at(7.5, 8.0));
            ring(at(12.5, 8.0));
        }
        MergeStrategy::SemiLinear => {
            line(at(3.5, 1.0), at(3.5, 15.0));
            curve(at(3.5, 2.5), at(3.5, 5.0), at(12.0, 3.0), at(12.0, 6.0));
            line(at(12.0, 6.0), at(12.0, 9.0));
            curve(at(12.0, 9.0), at(12.0, 12.0), at(3.5, 10.5), at(3.5, 13.0));
            dot(at(3.5, 2.5));
            dot(at(12.0, 6.0));
            dot(at(12.0, 9.5));
            ring(at(3.5, 13.0));
        }
    }
}
