//! The stage canvas: cards in dependency columns joined by rounded connectors with a dot at each
//! end, on a canvas the user grabs, drags and zooms (see `pan_canvas`). Only cards, rows and
//! connectors inside the visible part are drawn, so large pipelines cost what fits on screen.

use std::collections::HashSet;

use cv_base::theme::{self, motion};
use egui::{Rect, Shape, Stroke, Ui};
use review_core::{PipelineJob, RunStatus};

use super::layout::{self as graph_layout, GraphLayout};
use super::pan_canvas;
use super::stage_card::{self, CardAction, CardContext};

/// Extra margin around the viewport that is still painted, so panning never shows a gap.
const OVERSCAN: f32 = 96.0;
const DOT_RADIUS: f32 = 3.0;

#[derive(Clone, Copy)]
pub(crate) struct GraphInput<'a> {
    pub stages: &'a [PipelineJob],
    pub expanded: &'a HashSet<String>,
    pub highlighted: Option<&'a str>,
    pub context: CardContext<'a>,
}

/// Returns the clicked stage's index and what was clicked.
pub(crate) fn show(ui: &mut Ui, input: GraphInput<'_>) -> Option<(usize, CardAction)> {
    let palette = theme::palette(ui.ctx());
    let openings: Vec<f32> = input
        .stages
        .iter()
        .map(|stage| {
            let id = ui.scope_id().with(("stage_open", &stage.id));
            motion::toggle(ui.ctx(), id, input.expanded.contains(&stage.id), motion::MEDIUM)
        })
        .collect();
    let layout = GraphLayout::new(input.stages, |index| {
        stage_card::height(&input.stages[index], &input.context, openings[index])
    });
    let mut clicked = None;
    let mut pan = pan_canvas::begin(ui, ui.scope_id().with("stage_graph"), layout.size);
    // The canvas is drawn in its own coordinates; the pan and zoom map it to the screen.
    let visible = pan.viewport().expand(OVERSCAN / pan.zoom());
    paint_connectors(&pan.ui, &layout, input.stages, visible, &palette);
    for (index, card) in layout.cards.iter().enumerate() {
        if !card.intersects(visible) {
            continue;
        }
        let stage = &input.stages[index];
        let is_highlighted = input.highlighted == Some(stage.id.as_str());
        if let Some(action) = stage_card::show(
            &mut pan.ui,
            *card,
            stage,
            openings[index],
            is_highlighted,
            visible,
            &input.context,
        ) {
            clicked = Some((index, action));
        }
    }
    pan.finish(ui);
    clicked
}

fn paint_connectors(
    ui: &Ui,
    layout: &GraphLayout,
    stages: &[PipelineJob],
    visible: Rect,
    palette: &theme::Palette,
) {
    let painter = ui.painter();
    for &(from, to) in &layout.edges {
        let (start, end) = (layout.cards[from], layout.cards[to]);
        let bounds = Rect::from_two_pos(start.right_top(), end.left_bottom())
            .union(Rect::from_two_pos(start.right_bottom(), end.left_top()));
        if !bounds.intersects(visible) {
            continue;
        }
        // Waiting edges (the earlier stage has not finished) are drawn quieter.
        let is_done =
            stages[from].status.is_finished() && stages[from].status != RunStatus::Skipped;
        let color =
            if is_done { palette.accent.gamma_multiply(0.75) } else { palette.border_strong };
        let points = graph_layout::connector(start, end);
        let (first, last) = (points[0], points[points.len() - 1]);
        painter.add(Shape::line(points, Stroke::new(1.5, color)));
        painter.circle_filled(first, DOT_RADIUS, color);
        painter.circle_filled(last, DOT_RADIUS, color);
    }
}

/// Stages to show expanded when a run opens: every stage with jobs, so each card lists them.
pub(crate) fn initially_expanded(stages: &[PipelineJob]) -> HashSet<String> {
    stages.iter().filter(|stage| !stage.children.is_empty()).map(|stage| stage.id.clone()).collect()
}
