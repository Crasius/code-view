//! Placement of stage cards in dependency columns, the way Azure DevOps lays out a run: a stage
//! sits one column right of the latest stage it depends on, parallel stages stack in their
//! column, and every row is as tall as its tallest card so rows line up across columns.

use std::collections::HashMap;

use egui::{Pos2, Rect, Vec2, pos2, vec2};
use review_core::PipelineJob;

pub(crate) const CARD_WIDTH: f32 = 240.0;
pub(crate) const COLUMN_GAP: f32 = 40.0;
pub(crate) const ROW_GAP: f32 = 24.0;
pub(crate) const PADDING: f32 = 24.0;
/// Connectors attach this far below a card's top, level with its header.
pub(crate) const ATTACH_Y: f32 = 34.0;

pub(crate) struct GraphLayout {
    /// Card rectangles in stage order, relative to the canvas origin.
    pub cards: Vec<Rect>,
    /// `(from, to)`: stage `to` depends on stage `from`.
    pub edges: Vec<(usize, usize)>,
    pub size: Vec2,
}

impl GraphLayout {
    /// `height(index)` is the current height of a stage's card (collapsed or expanded).
    pub(crate) fn new(stages: &[PipelineJob], height: impl Fn(usize) -> f32) -> Self {
        let index_of: HashMap<&str, usize> =
            stages.iter().enumerate().map(|(index, stage)| (stage.id.as_str(), index)).collect();
        let columns = columns(stages, &index_of);
        let column_count = columns.iter().max().map_or(0, |max| max + 1);
        let mut rows = vec![0usize; stages.len()];
        let mut next_row = vec![0usize; column_count];
        for (index, column) in columns.iter().enumerate() {
            rows[index] = next_row[*column];
            next_row[*column] += 1;
        }
        let row_count = next_row.iter().max().copied().unwrap_or(0);
        let mut row_heights = vec![0.0f32; row_count];
        for (index, row) in rows.iter().enumerate() {
            row_heights[*row] = row_heights[*row].max(height(index));
        }
        let mut row_tops = Vec::with_capacity(row_count);
        let mut top = PADDING;
        for row_height in &row_heights {
            row_tops.push(top);
            top += row_height + ROW_GAP;
        }
        let cards: Vec<Rect> = (0..stages.len())
            .map(|index| {
                let left = PADDING + columns[index] as f32 * (CARD_WIDTH + COLUMN_GAP);
                Rect::from_min_size(
                    pos2(left, row_tops[rows[index]]),
                    vec2(CARD_WIDTH, height(index)),
                )
            })
            .collect();
        let edges = stages
            .iter()
            .enumerate()
            .flat_map(|(to, stage)| {
                stage
                    .depends_on
                    .iter()
                    .filter_map(|id| index_of.get(id.as_str()).copied())
                    .map(move |from| (from, to))
            })
            .filter(|(from, to)| columns[*from] < columns[*to])
            .collect();
        let width = PADDING * 2.0 + column_count as f32 * (CARD_WIDTH + COLUMN_GAP) - COLUMN_GAP;
        let height = top - ROW_GAP + PADDING;
        Self { cards, edges, size: vec2(width.max(0.0), height.max(0.0)) }
    }
}

/// Column of every stage: 0 without dependencies, else one right of its deepest dependency.
/// Unknown ids and cycles are ignored, so a bad graph still draws.
fn columns(stages: &[PipelineJob], index_of: &HashMap<&str, usize>) -> Vec<usize> {
    let mut columns: Vec<Option<usize>> = vec![None; stages.len()];
    for start in 0..stages.len() {
        resolve(start, stages, index_of, &mut columns, &mut Vec::new());
    }
    columns.into_iter().map(Option::unwrap_or_default).collect()
}

fn resolve(
    index: usize,
    stages: &[PipelineJob],
    index_of: &HashMap<&str, usize>,
    columns: &mut [Option<usize>],
    visiting: &mut Vec<usize>,
) -> usize {
    if let Some(column) = columns[index] {
        return column;
    }
    visiting.push(index);
    let mut column = 0;
    for dependency in
        stages[index].depends_on.iter().filter_map(|id| index_of.get(id.as_str()).copied())
    {
        if visiting.contains(&dependency) {
            continue;
        }
        column = column.max(resolve(dependency, stages, index_of, columns, visiting) + 1);
    }
    visiting.pop();
    columns[index] = Some(column);
    column
}

/// The connector from the right edge of `from` to the left edge of `to`: horizontal, then a
/// rounded vertical run in the gap before `to`, then horizontal again.
pub(crate) fn connector(from: Rect, to: Rect) -> Vec<Pos2> {
    let start = pos2(from.right(), from.top() + ATTACH_Y);
    let end = pos2(to.left(), to.top() + ATTACH_Y);
    let bend_x = to.left() - COLUMN_GAP / 2.0;
    let rise = end.y - start.y;
    if rise.abs() < 1.0 {
        return vec![start, end];
    }
    let radius = 8.0f32.min(rise.abs() / 2.0).min(COLUMN_GAP / 2.0);
    let direction = rise.signum();
    let mut points = vec![start, pos2(bend_x - radius, start.y)];
    points.extend(corner(
        pos2(bend_x - radius, start.y),
        pos2(bend_x, start.y),
        pos2(bend_x, start.y + radius * direction),
    ));
    points.push(pos2(bend_x, end.y - radius * direction));
    points.extend(corner(
        pos2(bend_x, end.y - radius * direction),
        pos2(bend_x, end.y),
        pos2(bend_x + radius, end.y),
    ));
    points.push(end);
    points
}

/// Quadratic curve from `from` to `to` around `control`, without its first point.
fn corner(from: Pos2, control: Pos2, to: Pos2) -> impl Iterator<Item = Pos2> {
    const STEPS: usize = 6;
    (1..=STEPS).map(move |step| {
        let t = step as f32 / STEPS as f32;
        let a = from.lerp(control, t);
        let b = control.lerp(to, t);
        a.lerp(b, t)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(id: &str, depends_on: &[&str]) -> PipelineJob {
        PipelineJob {
            id: id.to_owned(),
            name: id.to_owned(),
            depends_on: depends_on.iter().map(|id| (*id).to_owned()).collect(),
            ..PipelineJob::default()
        }
    }

    fn column_of(layout: &GraphLayout, index: usize) -> usize {
        ((layout.cards[index].left() - PADDING) / (CARD_WIDTH + COLUMN_GAP)).round() as usize
    }

    /// Stages that each depend on the one before stand in one row, one column each.
    #[test]
    fn a_chain_of_stages_forms_one_row_of_columns() {
        let stages = vec![
            stage("test", &[]),
            stage("build", &["test"]),
            stage("release", &["build"]),
            stage("documentation", &["release"]),
        ];
        let layout = GraphLayout::new(&stages, |_| 72.0);
        assert_eq!(
            (0..stages.len()).map(|index| column_of(&layout, index)).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert!(layout.cards.iter().all(|card| card.top() == PADDING), "one row");
        assert_eq!(layout.edges, vec![(0, 1), (1, 2), (2, 3)]);
        assert_eq!(layout.size.y, PADDING * 2.0 + 72.0);
    }

    #[test]
    fn cycles_and_unknown_ids_still_lay_out() {
        let stages = vec![
            stage("a", &["b"]),
            stage("b", &["a"]),
            stage("self", &["self", "unknown"]),
            stage("after", &["a"]),
        ];
        let layout = GraphLayout::new(&stages, |_| 72.0);
        // The cycle is cut where it is entered: "b" resolves first while "a" is being visited.
        assert_eq!(
            (0..stages.len()).map(|index| column_of(&layout, index)).collect::<Vec<_>>(),
            vec![1, 0, 0, 2]
        );
        // Only edges that point right are drawn; the back edge of the cycle and the self edge are not.
        assert_eq!(layout.edges, vec![(1, 0), (0, 3)]);
        assert!(layout.size.x.is_finite() && layout.size.y.is_finite());
    }

    #[test]
    fn parallel_stages_share_a_column_and_rows_align() {
        let stages = vec![
            stage("analyze", &[]),
            stage("build", &["analyze"]),
            stage("infra", &["analyze"]),
            stage("apps", &["build", "infra"]),
            stage("cleanup", &["apps"]),
            stage("cycle", &["cycle", "unknown"]),
        ];
        let tall = |index: usize| if index == 1 { 200.0 } else { 72.0 };
        let layout = GraphLayout::new(&stages, tall);
        assert_eq!(
            (0..stages.len()).map(|index| column_of(&layout, index)).collect::<Vec<_>>(),
            vec![0, 1, 1, 2, 3, 0]
        );
        // Row 0 is as tall as the expanded "build" card; "infra" and "cycle" start below it.
        assert_eq!(layout.cards[2].top(), PADDING + 200.0 + ROW_GAP);
        assert_eq!(layout.cards[5].top(), layout.cards[2].top());
        assert_eq!(layout.edges, vec![(0, 1), (0, 2), (1, 3), (2, 3), (3, 4)]);
        assert_eq!(layout.size.x, PADDING * 2.0 + 4.0 * CARD_WIDTH + 3.0 * COLUMN_GAP);
    }

    #[test]
    fn connectors_run_from_right_edge_to_left_edge() {
        let from = Rect::from_min_size(pos2(0.0, 0.0), vec2(CARD_WIDTH, 72.0));
        let level = Rect::from_min_size(pos2(CARD_WIDTH + COLUMN_GAP, 0.0), vec2(CARD_WIDTH, 72.0));
        assert_eq!(connector(from, level).len(), 2);
        let below =
            Rect::from_min_size(pos2(CARD_WIDTH + COLUMN_GAP, 300.0), vec2(CARD_WIDTH, 72.0));
        let points = connector(from, below);
        assert_eq!(points.first(), Some(&pos2(CARD_WIDTH, ATTACH_Y)));
        assert_eq!(points.last(), Some(&pos2(below.left(), 300.0 + ATTACH_Y)));
        assert!(points.windows(2).all(|pair| pair[1].y >= pair[0].y), "descends monotonically");
    }
}
