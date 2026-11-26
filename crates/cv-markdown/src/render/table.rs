//! Table geometry: how wide the columns of a table are given the room there is, and the lines
//! and header tint drawn around its cells.

use cv_base::theme::{Palette, style as tokens};
use egui::layers::ShapeIdx;
use egui::{Align, CornerRadius, Painter, Rect, Stroke};
use pulldown_cmark::Alignment;

use super::typeset::Look;
use crate::parse::Inline;

/// A table column's text narrows to this, or to its longest word, before the table scrolls
/// sideways (at scale 1); a column whose widest cell is narrower keeps that width.
const MIN_COLUMN_WIDTH: f32 = 96.0;
/// A column keeps its longest word whole up to this width (at scale 1); a longer word breaks.
const MAX_WORD_WIDTH: f32 = 240.0;

/// What a table's columns measure, in points at the renderer's scale.
pub(super) struct Columns {
    /// Each column's widest cell on one line.
    pub natural: Vec<f32>,
    /// Each column's longest word.
    pub words: Vec<f32>,
    /// The padding around the cells of all columns together.
    pub frames: f32,
}

/// How a table is laid out.
#[derive(Debug, PartialEq)]
pub(super) struct TablePlan {
    /// The table's width with its padding: the text width, or more for a table that breaks out.
    pub width: f32,
    /// The text width of each column.
    pub widths: Vec<f32>,
    /// The columns cannot shrink to fit: the table scrolls sideways.
    pub overflows: bool,
}

/// Plans a table of `columns` in `available` width. A top-level table may grow to `breakout`
/// (centered on the text), but only as far as its columns need to show on one line each. A
/// column narrows to its longest word (up to a cap) where the room allows, else to the least
/// width, breaking the word, before the table scrolls.
pub(super) fn plan(
    columns: &Columns,
    available: f32,
    breakout: Option<f32>,
    scale: f32,
) -> TablePlan {
    let Columns { natural, words, frames } = columns;
    let natural_width = natural.iter().sum::<f32>() + frames;
    let width = match breakout {
        Some(breakout) if natural_width > available => natural_width.min(breakout).max(available),
        _ => available,
    };
    let room = width - frames;
    let floor = MIN_COLUMN_WIDTH * scale;
    let whole_words: Vec<f32> = natural
        .iter()
        .zip(words)
        .map(|(width, word)| width.min(word.min(MAX_WORD_WIDTH * scale).max(floor)))
        .collect();
    let minimum = if whole_words.iter().sum::<f32>() <= room {
        whole_words
    } else {
        natural.iter().map(|width| width.min(floor)).collect()
    };
    let widths = column_widths(natural, &minimum, room);
    let overflows = widths.iter().sum::<f32>() > room + 0.5;
    TablePlan { width, widths, overflows }
}

/// A table measured for [`super::Renderer::grid`]: its rows (the header first) and the width of each
/// column's text.
#[derive(Clone, Copy)]
pub(super) struct Grid<'a> {
    pub alignments: &'a [Alignment],
    pub table: &'a [(&'a [Inline], Look)],
    pub widths: &'a [f32],
    /// Around each cell's text.
    pub padding: egui::Vec2,
    /// A row is at least one line tall.
    pub min_height: f32,
}

/// The text width of each column of a table with `room` for all of them: the `natural` widths
/// (each column's widest cell on one line) when they fit. Otherwise every column that fits in its
/// share keeps its natural width and the others share the rest in proportion to their natural
/// widths, none below its `minimum`; when even the minimums do not fit, the table takes them and
/// scrolls.
fn column_widths(natural: &[f32], minimum: &[f32], room: f32) -> Vec<f32> {
    if natural.iter().sum::<f32>() <= room {
        return natural.to_vec();
    }
    if minimum.iter().sum::<f32>() >= room {
        return minimum.to_vec();
    }
    // Columns settle at their natural or their least width until the rest share what is left.
    let mut settled: Vec<Option<f32>> = vec![None; natural.len()];
    loop {
        let left = room - settled.iter().flatten().sum::<f32>();
        let weight: f32 = natural
            .iter()
            .zip(&settled)
            .filter(|(_, settled)| settled.is_none())
            .map(|(width, _)| width.max(1.0))
            .sum();
        let mut is_settling = false;
        for (index, width) in natural.iter().enumerate() {
            if settled[index].is_some() {
                continue;
            }
            let share = left * width.max(1.0) / weight.max(1.0);
            if share >= *width || share <= minimum[index] {
                settled[index] = Some(if share >= *width { *width } else { minimum[index] });
                is_settling = true;
            }
        }
        if !is_settling {
            return natural
                .iter()
                .zip(&settled)
                .map(|(width, settled)| {
                    settled.unwrap_or_else(|| (left * width.max(1.0) / weight.max(1.0)).floor())
                })
                .collect();
        }
    }
}

/// How the cells of `column` align.
pub(super) fn cell_align(alignments: &[Alignment], column: usize) -> Align {
    match alignments.get(column) {
        Some(Alignment::Center) => Align::Center,
        Some(Alignment::Right) => Align::Max,
        Some(Alignment::Left | Alignment::None) | None => Align::Min,
    }
}

/// The header's tint (into the reserved `header_fill`), the row and column lines and the frame of
/// a table laid out in `rect`, its rows starting at `row_tops` (the last entry is the bottom).
pub(super) fn paint_lines(
    painter: &Painter,
    grid: &Grid<'_>,
    rect: Rect,
    row_tops: &[f32],
    header_fill: ShapeIdx,
    palette: &Palette,
) {
    let radius = CornerRadius::same(tokens::RADIUS_CONTROL);
    let line = Stroke::new(1.0, palette.border);
    if let [top, below, ..] = row_tops[..] {
        let header = Rect::from_x_y_ranges(rect.x_range(), egui::Rangef::new(top, below));
        painter.set(
            header_fill,
            egui::Shape::rect_filled(
                header,
                CornerRadius { sw: 0, se: 0, ..radius },
                palette.surface_subtle,
            ),
        );
    }
    let inner_tops = row_tops.get(1..row_tops.len().saturating_sub(1)).unwrap_or_default();
    for top in inner_tops {
        painter.hline(rect.x_range(), *top, line);
    }
    let mut x = rect.left();
    for column_width in &grid.widths[..grid.widths.len().saturating_sub(1)] {
        x += column_width + 2.0 * grid.padding.x;
        painter.vline(x, rect.y_range(), line);
    }
    painter.rect_stroke(rect, radius, line, egui::StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::{Columns, TablePlan, column_widths, plan};

    #[test]
    fn columns_share_the_room_in_proportion_and_keep_their_least_width() {
        assert_eq!(
            column_widths(&[50.0, 80.0], &[50.0, 80.0], 200.0),
            [50.0, 80.0],
            "a table that fits keeps its widths"
        );
        let shared = column_widths(&[60.0, 1000.0, 2000.0], &[60.0, 96.0, 96.0], 900.0);
        assert_eq!(shared[0], 60.0, "a narrow column keeps its width: {shared:?}");
        let total: f32 = shared.iter().sum();
        assert!(total <= 900.0 && total > 895.0, "the room is used: {shared:?}");
        assert!(
            (shared[2] / shared[1] - 2.0).abs() < 0.05,
            "the others share in proportion: {shared:?}"
        );
        assert_eq!(
            column_widths(&[500.0, 500.0], &[96.0, 96.0], 150.0),
            [96.0, 96.0],
            "too narrow even for the least widths: they scroll"
        );
    }

    fn columns(natural: &[f32], words: &[f32], frames: f32) -> Columns {
        Columns { natural: natural.to_vec(), words: words.to_vec(), frames }
    }

    #[test]
    fn a_top_level_table_breaks_out_only_as_far_as_its_columns_need() {
        let wide = columns(&[300.0, 300.0], &[50.0, 50.0], 40.0);
        assert_eq!(
            plan(&wide, 400.0, Some(800.0), 1.0),
            TablePlan { width: 640.0, widths: vec![300.0, 300.0], overflows: false },
            "grows to its natural width, short of the breakout"
        );
        let wider = columns(&[500.0, 500.0], &[50.0, 50.0], 40.0);
        assert_eq!(
            plan(&wider, 400.0, Some(700.0), 1.0),
            TablePlan { width: 700.0, widths: vec![330.0, 330.0], overflows: false },
            "stops at the breakout and wraps"
        );
        assert_eq!(
            plan(&wider, 400.0, None, 1.0),
            TablePlan { width: 400.0, widths: vec![180.0, 180.0], overflows: false },
            "a nested table keeps to the text width"
        );
        assert_eq!(
            plan(&columns(&[100.0], &[100.0], 20.0), 400.0, Some(800.0), 1.0).width,
            400.0,
            "a table that fits never narrows below the text width"
        );
    }

    #[test]
    fn long_words_stay_whole_until_the_room_runs_out() {
        // The identifier column keeps its 200 pt word; the prose column gives way.
        let words = columns(&[400.0, 400.0], &[200.0, 30.0], 0.0);
        assert_eq!(plan(&words, 380.0, None, 1.0).widths, [200.0, 180.0]);
        // No room for whole words: every column narrows towards the least width, then scrolls.
        let narrow = columns(&[500.0, 500.0, 500.0], &[400.0, 400.0, 400.0], 0.0);
        assert_eq!(
            plan(&narrow, 300.0, None, 1.0),
            TablePlan { width: 300.0, widths: vec![100.0, 100.0, 100.0], overflows: false }
        );
        assert_eq!(
            plan(&narrow, 200.0, None, 1.0),
            TablePlan { width: 200.0, widths: vec![96.0, 96.0, 96.0], overflows: true }
        );
    }
}
