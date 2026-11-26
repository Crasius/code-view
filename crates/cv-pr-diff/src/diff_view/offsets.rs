//! Where each row of the diff starts. A code row is a line tall (a multiple of it when wrapped),
//! gap bars and separators have their bar's height, removals none, and comment cards the height
//! they were measured at (see [`super::cards`]). The tops are worked out again only after a height
//! changed.

use std::collections::HashMap;
use std::ops::Index;

use cv_code::diff_paint::Metrics;
use cv_code::diff_rows::{Row, RowKind};

/// What row heights depend on besides the rows.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(super) struct HeightKey {
    pub line_height: f32,
    pub gap_height: f32,
    /// Columns per visual row of each pane when wrapping.
    pub wrap_columns: Option<[usize; 2]>,
}

#[derive(Default)]
pub(super) struct RowOffsets {
    /// The top of each row, and the total height last.
    tops: Vec<f32>,
    is_dirty: bool,
    key: Option<HeightKey>,
    /// Visual rows of wrapped lines whose layout differed from the column estimate (wide glyphs).
    wrapped: HashMap<u32, u32>,
}

impl RowOffsets {
    /// The rows' height: code, comment cards and all.
    pub fn height(&self) -> f32 {
        self.tops.last().copied().unwrap_or(0.0)
    }

    pub fn get(&self, row: usize) -> Option<f32> {
        self.tops.get(row).copied()
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.tops
    }

    /// The tops are those of `rows` rows: they were placed since the rows were last built.
    pub fn is_placed(&self, rows: usize) -> bool {
        self.tops.len() == rows + 1
    }

    /// The row at content height `y`: the last one starting at or above it.
    pub fn row_at(&self, y: f32) -> usize {
        self.tops.partition_point(|top| *top <= y).saturating_sub(1)
    }

    /// The first row reaching below `y`.
    pub fn first_below(&self, y: f32) -> usize {
        self.tops.partition_point(|top| *top < y).saturating_sub(1)
    }

    /// The row of `rows` rows at content height `y` and how far into it `y` lies (0..1).
    pub fn anchor_at(&self, y: f32, rows: usize) -> Option<(usize, f32)> {
        if rows == 0 || !self.is_placed(rows) {
            return None;
        }
        let row = self.row_at(y).min(rows - 1);
        let height = (self.tops[row + 1] - self.tops[row]).max(1.0);
        Some((row, ((y - self.tops[row]) / height).clamp(0.0, 1.0)))
    }

    pub fn is_dirty(&self) -> bool {
        self.is_dirty
    }

    /// A row's height changed: the tops are worked out again before the next paint.
    pub fn mark_dirty(&mut self) {
        self.is_dirty = true;
    }

    /// The rows were built anew: wrapped lines are estimated again.
    pub fn rows_rebuilt(&mut self) {
        self.wrapped.clear();
        self.is_dirty = true;
    }

    /// Takes the heights of `key`; when they differ from the last ones every wrapped line is
    /// estimated again. Returns the heights `key` replaced (none the first time).
    pub fn rekey(&mut self, key: HeightKey) -> Option<HeightKey> {
        if self.key == Some(key) {
            return None;
        }
        self.wrapped.clear();
        self.is_dirty = true;
        self.key.replace(key)
    }

    /// Code row `row` wrapped to `rows` visual rows where `estimate` were counted; true when that
    /// moves the rows below.
    pub fn note_wrapped(&mut self, row: usize, rows: usize, estimate: usize) -> bool {
        if rows == estimate {
            return false;
        }
        self.wrapped.insert(row as u32, rows as u32);
        self.is_dirty = true;
        true
    }

    /// The tops of `rows`, code rows `row_columns` wide, with `metrics` and `wrap_columns`
    /// columns per visual row when wrapping; `card` gives the height of a thread's or the new
    /// comment's card row.
    pub fn recompute(
        &mut self,
        rows: &[Row],
        row_columns: &[[u32; 2]],
        metrics: &Metrics,
        wrap_columns: Option<[usize; 2]>,
        card: impl Fn(RowKind) -> f32,
    ) {
        self.tops.clear();
        self.tops.reserve(rows.len() + 1);
        let mut top = 0.0;
        for (index, row) in rows.iter().enumerate() {
            self.tops.push(top);
            top += match row.kind {
                RowKind::Line | RowKind::Expanded => {
                    let estimate = wrap_columns.map_or(1, |[left_columns, right_columns]| {
                        let [left, right] = row_columns.get(index).copied().unwrap_or_default();
                        left.div_ceil(left_columns as u32)
                            .max(right.div_ceil(right_columns as u32))
                            .max(1)
                    });
                    let rows = self.wrapped.get(&(index as u32)).copied().unwrap_or(estimate);
                    metrics.line_height * rows as f32
                }
                RowKind::Gap(_) => metrics.gap_height,
                RowKind::Removal => 0.0,
                RowKind::Separator => metrics.separator_height,
                RowKind::Thread(_) | RowKind::Composer => card(row.kind),
            };
        }
        self.tops.push(top);
        self.is_dirty = false;
    }
}

impl Index<usize> for RowOffsets {
    type Output = f32;

    fn index(&self, row: usize) -> &f32 {
        &self.tops[row]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offsets(tops: &[f32]) -> RowOffsets {
        RowOffsets { tops: tops.to_vec(), ..RowOffsets::default() }
    }

    #[test]
    fn rows_are_found_by_height_and_anchored_by_fraction() {
        let offsets = offsets(&[0.0, 20.0, 20.0, 60.0]);
        assert_eq!(offsets.row_at(0.0), 0);
        assert_eq!(offsets.row_at(20.0), 2, "a row without height is passed over");
        assert_eq!(offsets.first_below(20.0), 0, "the row ending at 20 still reaches it");
        assert_eq!(offsets.anchor_at(40.0, 3), Some((2, 0.5)));
        assert_eq!(offsets.anchor_at(500.0, 3), Some((2, 1.0)), "past the end: the last row");
        assert_eq!(offsets.anchor_at(10.0, 4), None, "tops of other rows");
        assert_eq!(offsets.height(), 60.0);
    }

    #[test]
    fn a_new_key_or_a_wrapped_line_asks_for_new_tops() {
        let key = HeightKey { line_height: 20.0, gap_height: 32.0, wrap_columns: None };
        let mut offsets = RowOffsets::default();
        assert_eq!(offsets.rekey(key), None, "the first heights replace none");
        assert!(offsets.is_dirty());
        offsets.is_dirty = false;
        assert_eq!(offsets.rekey(key), None, "the same heights");
        assert!(!offsets.is_dirty());
        assert!(!offsets.note_wrapped(3, 2, 2), "as estimated");
        assert!(offsets.note_wrapped(3, 3, 2) && offsets.is_dirty());
        let zoomed = HeightKey { line_height: 24.0, ..key };
        assert_eq!(offsets.rekey(zoomed), Some(key));
        assert!(offsets.wrapped.is_empty(), "wrapped lines are estimated again");
    }
}
