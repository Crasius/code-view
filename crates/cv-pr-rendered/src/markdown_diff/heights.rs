//! Heights and tops of the rendered diff's rows. A block row is as tall as its taller block's
//! laid-out text with the room inside its tint (estimated until the block was measured), a fold
//! has its bar's height, a removal none and a comment card the height it was measured at.
//! Blocks are measured unseen ahead of the view ([`RowHeights::measure`]), so scrolling into them
//! moves nothing.

use std::collections::HashMap;

use egui::{Align, Id, Layout, Rect, Ui, UiBuilder, vec2};
use review_core::DiffSide;

use super::ROW_PAD;
use super::lanes::Lanes;
use super::model::{Cell, MarkdownModel, Row};
use super::threads::ThreadRow;
use cv_markdown::RenderOptions;

const TOP_PADDING: f32 = 12.0;
const BOTTOM_PADDING: f32 = 48.0;
const FOLD_HEIGHT: f32 = 30.0;
/// How far below the view blocks are laid out unseen to be measured.
const MEASURE_OFFSET: f32 = 100_000.0;

#[derive(Default)]
pub(super) struct RowHeights {
    /// Laid-out height of each block's text, for `key`.
    cells: HashMap<(DiffSide, usize), f32>,
    key: u64,
    /// Each row's height, and whether it was measured rather than estimated.
    heights: Vec<f32>,
    fresh: Vec<bool>,
    /// The top of each row, and the total height last.
    tops: Vec<f32>,
    is_dirty: bool,
}

impl RowHeights {
    /// `rows` rows were built anew: none is measured or placed yet.
    pub fn reset(&mut self, rows: usize) {
        self.heights = vec![0.0; rows];
        self.fresh = vec![false; rows];
        self.tops.clear();
        self.is_dirty = true;
    }

    /// What the blocks' heights depend on (the lanes' widths, the zoom, the images loaded): when
    /// it changes, every block is measured again.
    pub fn rekey(&mut self, key: u64) {
        if key != self.key {
            self.key = key;
            self.cells.clear();
            self.is_dirty = true;
        }
    }

    /// A row's height changed: the rows are placed again before the next paint.
    pub fn mark_dirty(&mut self) {
        self.is_dirty = true;
    }

    pub fn is_dirty(&self) -> bool {
        self.is_dirty
    }

    /// The rows were placed since they were last built.
    pub fn is_placed(&self) -> bool {
        self.tops.len() == self.heights.len() + 1
    }

    /// The top of each row, and the total height last.
    pub fn tops(&self) -> &[f32] {
        &self.tops
    }

    pub fn top(&self, row: usize) -> f32 {
        self.tops[row]
    }

    pub fn height(&self, row: usize) -> f32 {
        self.heights[row]
    }

    /// The rows' height with the padding above and below them.
    pub fn total(&self) -> f32 {
        self.tops.last().copied().unwrap_or(0.0)
    }

    /// The row was measured, not estimated.
    pub fn is_measured(&self, row: usize) -> bool {
        self.fresh[row]
    }

    pub fn is_all_measured(&self) -> bool {
        self.fresh.iter().all(|fresh| *fresh)
    }

    /// The row at content height `y`: the last one starting at or above it.
    pub fn row_at(&self, y: f32) -> usize {
        self.tops
            .partition_point(|top| *top <= y)
            .saturating_sub(1)
            .min(self.heights.len().saturating_sub(1))
    }

    /// The rows reaching from `top` to `bottom`.
    pub fn rows_between(&self, top: f32, bottom: f32) -> impl Iterator<Item = usize> + '_ {
        let first = self.tops.partition_point(|offset| *offset < top).saturating_sub(1);
        (first..self.heights.len()).take_while(move |row| self.tops[*row] <= bottom)
    }

    /// Block `cell` was laid out `height` tall where it shows; true when that differs from what
    /// was known (a font arriving), so the rows are placed again.
    pub fn note_shown(&mut self, cell: Cell, height: f32) -> bool {
        let known = self.cells.insert((cell.side, cell.unit), height);
        known.is_none_or(|known| (known - height).abs() > 0.5)
    }

    /// Lays the blocks of `cells` not measured yet out unseen, at their lane's width in `lanes`.
    pub fn measure(
        &mut self,
        ui: &mut Ui,
        model: &MarkdownModel,
        lanes: &Lanes,
        options: RenderOptions,
        cells: [Option<Cell>; 2],
        id: Id,
    ) {
        for cell in cells.into_iter().flatten() {
            if self.cells.contains_key(&(cell.side, cell.unit)) {
                continue;
            }
            // Far outside the view: unseen widgets still take hovers from the rows under them.
            let rect = Rect::from_min_size(
                ui.max_rect().left_bottom() + vec2(0.0, MEASURE_OFFSET),
                vec2(lanes.text_width(cell.side), f32::INFINITY),
            );
            let mut child = ui.new_child(
                UiBuilder::new()
                    .max_rect(rect)
                    .layout(Layout::top_down(Align::Min))
                    .id_salt(("markdown_diff_measure", cell.side, cell.unit))
                    .invisible(),
            );
            let options = lanes.options(model.options(options, cell.side, None), cell.side);
            model.units(cell.side).show(
                &mut child,
                id.with(("measure", cell.side, cell.unit)),
                cell.unit,
                options,
                [],
            );
            self.cells.insert((cell.side, cell.unit), child.min_rect().height());
        }
        self.is_dirty = true;
    }

    /// Places `rows` from the heights known, `spaces` (at zoom 1) above each, at zoom `scale`;
    /// `card` gives the height of a thread's card row.
    pub fn place(
        &mut self,
        rows: &[Row],
        spaces: &[f32],
        model: &MarkdownModel,
        lanes: &Lanes,
        scale: f32,
        card: impl Fn(&ThreadRow) -> f32,
    ) {
        self.tops.clear();
        self.tops.reserve(rows.len() + 1);
        let mut y = TOP_PADDING;
        for (index, row) in rows.iter().enumerate() {
            y += spaces[index] * scale;
            self.tops.push(y);
            let (height, fresh) = match row {
                Row::Blocks { .. } => self.blocks_height(row, model, lanes, scale),
                Row::Folded { .. } => (FOLD_HEIGHT, true),
                Row::Removal { .. } => (0.0, true),
                // Cards are measured where they are painted, not ahead.
                Row::Thread(thread) => (card(thread), true),
            };
            self.heights[index] = height;
            self.fresh[index] = fresh;
            y += height;
        }
        self.tops.push(y + BOTTOM_PADDING);
        self.is_dirty = false;
    }

    /// A block row's height: its taller block's, measured or estimated (then not fresh), with the
    /// room inside its tint.
    fn blocks_height(
        &self,
        row: &Row,
        model: &MarkdownModel,
        lanes: &Lanes,
        scale: f32,
    ) -> (f32, bool) {
        let mut fresh = true;
        let mut text = 0.0_f32;
        for cell in row.cells().into_iter().flatten() {
            let measured = self.cells.get(&(cell.side, cell.unit)).copied();
            fresh &= measured.is_some();
            let height = measured.unwrap_or_else(|| {
                let units = model.units(cell.side);
                units.estimate_height(cell.unit, lanes.text_width(cell.side), scale)
            });
            text = text.max(height);
        }
        (text + 2.0 * ROW_PAD * scale, fresh)
    }
}

/// The rows around `center` of `rows` rows, nearest first, the one above before the one below;
/// `center` itself is not among them.
pub(super) fn outward(center: usize, rows: usize) -> impl Iterator<Item = usize> {
    (1..=rows)
        .map(move |distance| {
            [center.checked_sub(distance), Some(center + distance).filter(|row| *row < rows)]
        })
        .take_while(|candidates| candidates.iter().any(Option::is_some))
        .flat_map(|candidates| candidates.into_iter().flatten())
}
