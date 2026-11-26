//! The find widget of the rendered Markdown diff (the search itself is the text diff's, see
//! [`cv_pr_diff::find`]). It reads the text of every block as it shows,
//! in the order of the pairs: an unchanged block once, an edited block's old and new version as
//! one entry, an added or removed block alone. A hit's numbers are the indices of its blocks in
//! the old and the new text. Side by side the widget searches the focused column only (Before or
//! After, see [`cv_pr_diff::find::bar`]); one column searches the whole
//! diff. The rendered text has no selection of the diff's own, so the widget has no Find in
//! selection here.
//!
//! A block with hits fills them in the find color in place of its changed words, the block of the
//! current hit gets an outline, and the overview strip a tick per hit at its row (a folded
//! block's at its fold). Going to a hit opens its fold and puts its block in the middle of the
//! view, unless the block is in view already.

use std::ops::Range;
use std::sync::Arc;

use egui::{Rect, Ui, pos2};

use super::align::Change;
use super::heights::RowHeights;
use super::model::{Cell, Mode, Model, Row, Side};
use cv_base::theme::Palette;
use cv_code::find_paint::{self as paint, HitSide};
use cv_markdown::Marks;
use cv_pr_diff::diff_overview::Overview;
use cv_pr_diff::find::bar::{FindBar, Panes};
use cv_pr_diff::find::{Entry, Found, Hit, search, spawn};
use cv_widgets::find_bar::Step;

/// What the rows were built from (the diff's `rows_key`).
type RowsKey = (Mode, u32, u64, u64);

/// The rendered diff's find widget and where its hits are among the rows.
#[derive(Default)]
pub(super) struct BlockFind {
    bar: FindBar,
    /// The columns this frame and the side searched, for the widget painted after the rows.
    focus: Option<(Panes, HitSide)>,
    /// The row of each hit (a folded block's fold), for the rows built from `placed_for`.
    hit_rows: Vec<Option<usize>>,
    placed_for: Option<RowsKey>,
}

/// What the widget asked for this frame, acted on once the rows are built ([`BlockFind::go`]).
pub(super) struct BarFrame {
    step: Option<Step>,
    /// Hits arrived: the first one in view becomes current.
    arrived: bool,
}

/// The rows the hits are placed among.
pub(super) struct FindRows<'a> {
    pub key: Option<RowsKey>,
    pub rows: &'a [Row],
    /// The row of each block of the old and of the new text.
    pub old_rows: &'a [Option<usize>],
    pub new_rows: &'a [Option<usize>],
    pub heights: &'a RowHeights,
    /// The part of the rows in view, in row coordinates.
    pub view: Rect,
}

/// The hits a block paints (the current one in a stronger layer of its own), and whether the
/// current hit is among them.
pub(super) struct BlockMarks {
    pub layers: Vec<Marks>,
    pub is_current: bool,
}

impl BlockFind {
    /// The widget's keys while the diff `owns_keys`, the focused one of `panes` and the search of
    /// `model` there; before the rows paint (the widget paints after them, [`Self::show_widget`]).
    pub fn prepare(
        &mut self,
        ui: &Ui,
        panes: Panes,
        owns_keys: bool,
        model: &Arc<Model>,
    ) -> BarFrame {
        let step = self.bar.keys(ui, owns_keys, || None);
        let side = self.bar.follow_focus(ui, &panes, None);
        self.focus = Some((panes, side));
        let source = egui::epaint::util::hash((model.key, Arc::as_ptr(model) as usize));
        let (ctx, model) = (ui.ctx().clone(), Arc::clone(model));
        self.bar.refresh(ui.ctx(), side, source, move |query| {
            spawn(&ctx, move || {
                let hits = search(entries(&model), &query);
                Found::new(query, source, hits)
            })
        });
        let arrived = self.bar.poll();
        BarFrame { step, arrived }
    }

    /// The widget over the focused column, after the rows painted, so it is on top of them and
    /// takes its own clicks.
    pub fn show_widget(&mut self, ui: &mut Ui) {
        if let Some((panes, side)) = self.focus {
            // Closing drops the field's keyboard, so the diff's keys work again.
            let _closed = self.bar.show(ui, &panes, side);
        }
    }

    /// Places the hits among `rows` when either is new, steps where the bar asked (or to the first
    /// hit in view when hits arrived) and ticks the strip; the block to go to.
    pub fn go(
        &mut self,
        frame: &BarFrame,
        rows: &FindRows<'_>,
        model: &Model,
        overview: &mut Overview,
    ) -> Option<(Side, usize)> {
        if frame.arrived || self.placed_for != rows.key {
            self.place(rows, model);
        }
        let jump = match frame.step {
            Some(step) => self.bar.step(step).is_some(),
            None if frame.arrived => {
                self.bar.set_current(self.first_in_view(rows));
                self.bar.current().is_some()
            }
            None => false,
        };
        self.tick(overview);
        jump.then(|| self.target(rows)).flatten()
    }

    /// The hits `cell` paints (side by side only those of the column searched), and whether the
    /// current one is among them.
    pub fn marks(&self, cell: Cell, mode: Mode, palette: &Palette) -> Option<BlockMarks> {
        let hits = self.bar.hits()?;
        let unit = cell.unit as u32;
        let is_column = mode == Mode::Columns;
        let found: Vec<(usize, &Hit)> = match cell.side {
            Side::Old => hits
                .on_old(unit)
                .filter(|(_, hit)| !is_column || hit.side != HitSide::New)
                .collect(),
            Side::New => hits
                .on_new(unit)
                .filter(|(_, hit)| !is_column || hit.side != HitSide::Old)
                .collect(),
        };
        if found.is_empty() {
            return None;
        }
        let current = self.bar.current();
        let (strong, others): (Vec<_>, Vec<_>) =
            found.iter().partition(|(index, _)| current == Some(*index));
        let layer = |hits: Vec<&(usize, &Hit)>, is_current: bool| {
            let ranges: Vec<Range<usize>> = hits.iter().map(|(_, hit)| hit.bytes.clone()).collect();
            Marks { ranges, color: paint::fill(palette, is_current) }
        };
        Some(BlockMarks {
            is_current: !strong.is_empty(),
            layers: vec![layer(others, false), layer(strong, true)],
        })
    }

    /// Works out the row of every hit: its block's row, or the fold hiding it (New only: the row
    /// after a removed block).
    fn place(&mut self, rows: &FindRows<'_>, model: &Model) {
        self.placed_for = rows.key;
        let Some(hits) = self.bar.hits() else {
            self.hit_rows.clear();
            return;
        };
        let row_of = |hit: &Hit| {
            let (side, unit) = block_of(hit)?;
            let unit_rows = match side {
                Side::Old => rows.old_rows,
                Side::New => rows.new_rows,
            };
            unit_rows.get(unit).copied().flatten().or_else(|| {
                let pair = model.pair_of(side, unit)?;
                rows.rows.iter().position(|row| row.pairs().end > pair)
            })
        };
        self.hit_rows = hits.list.iter().map(row_of).collect();
    }

    /// The first hit at or below the top of the view, else the first one.
    fn first_in_view(&self, rows: &FindRows<'_>) -> Option<usize> {
        let tops = rows.heights.tops();
        let below = self.hit_rows.iter().position(|row| {
            row.and_then(|row| tops.get(row)).is_some_and(|top| *top >= rows.view.top() - 0.5)
        });
        below.or_else(|| (!self.hit_rows.is_empty()).then_some(0))
    }

    /// The block of the current hit; nothing while it is in view already.
    fn target(&self, rows: &FindRows<'_>) -> Option<(Side, usize)> {
        let current = self.bar.current()?;
        let hit = self.bar.hits()?.list.get(current)?;
        let row = self.hit_rows.get(current).copied().flatten();
        let is_in_view = row.is_some_and(|row| {
            let Some(top) = rows.heights.tops().get(row).copied() else {
                return false;
            };
            let bottom = top + rows.heights.height(row);
            matches!(rows.rows.get(row), Some(Row::Blocks { .. }))
                && top >= rows.view.top() + self.bar.cover()
                && bottom <= rows.view.bottom()
        });
        if is_in_view { None } else { block_of(hit) }
    }

    /// The overview strip's ticks of the hits, the current one marked.
    fn tick(&self, overview: &mut Overview) {
        overview.finds.clear();
        overview.current_find = None;
        let Some(hits) = self.bar.hits() else {
            return;
        };
        for (index, (hit, row)) in hits.list.iter().zip(&self.hit_rows).enumerate() {
            let Some(row) = row.and_then(|row| u32::try_from(row).ok()) else {
                continue;
            };
            overview.finds.push((row, hit.side));
            if self.bar.current() == Some(index) {
                overview.current_find = Some(row);
            }
        }
    }
}

/// The columns of `content` the widget searches one at a time: Before `left_width` wide and After
/// side by side, else the one column.
pub(super) fn panes(content: Rect, left_width: f32, mode: Mode) -> Panes {
    if mode != Mode::Columns {
        return Panes::One(content);
    }
    let middle = content.left() + left_width;
    Panes::Two {
        left: Rect::from_min_max(content.min, pos2(middle, content.bottom())),
        right: Rect::from_min_max(pos2(middle, content.top()), content.max),
    }
}

/// The block a hit goes to: its new version where it has one.
fn block_of(hit: &Hit) -> Option<(Side, usize)> {
    match (hit.new, hit.old) {
        (Some(unit), _) => Some((Side::New, unit as usize)),
        (None, Some(unit)) => Some((Side::Old, unit as usize)),
        (None, None) => None,
    }
}

/// The blocks of `model` as the search reads them, in the order of the pairs.
fn entries(model: &Model) -> Vec<Entry<'_>> {
    let text = |side: Side, unit: Option<usize>| {
        let unit = unit?;
        let text = model.units(side).units.get(unit)?.text.as_str();
        Some((u32::try_from(unit).ok()?, text))
    };
    model
        .pairs
        .iter()
        .map(|pair| Entry {
            old: text(Side::Old, pair.old),
            new: text(Side::New, pair.new),
            is_unchanged: pair.change == Change::Equal,
        })
        .collect()
}
