//! Text and line selection in the diff: the shared [`text_selection`] model with the diff's
//! panes, plus what a comment on the selected lines needs. Side by side keeps a selection in one
//! pane, as GitHub and Azure DevOps do.

use cv_code::diff_rows::Row;
use cv_code::line_source::Pane;
use cv_widgets::text_selection::{self, Unit};

pub type Selection = text_selection::Selection<Pane>;

/// What the diff adds to the shared selection model; a trait because the model lives in
/// `cv-widgets`.
pub trait DiffSelection {
    /// First and last code row the selection covers, for a comment on those lines. A text
    /// selection that ends at the very start of a line does not include that line.
    fn line_rows(&self, rows: &[Row], has_line: impl FnMut(&Row) -> bool)
    -> Option<(usize, usize)>;

    /// 1-based character offsets of the first selected character and just past the last one,
    /// for a comment on `first..=last`; whole lines run from 1 to just past `last_len`.
    fn offsets(&self, first: usize, last: usize, last_len: usize) -> (u32, u32);

    /// Follows the selected rows into a rebuilt row list (a comment appeared, lines were
    /// revealed); `false` when a selected row is gone.
    fn remap(&mut self, old: &[Row], new: &[Row]) -> bool;
}

impl DiffSelection for Selection {
    fn line_rows(
        &self,
        rows: &[Row],
        mut has_line: impl FnMut(&Row) -> bool,
    ) -> Option<(usize, usize)> {
        if self.is_empty() {
            return None;
        }
        let (start, end) = self.bounds();
        let last = if self.unit == Unit::Text && end.column == 0 && end.row > start.row {
            end.row - 1
        } else {
            end.row
        };
        let last = last.min(rows.len().checked_sub(1)?);
        let first = (start.row..=last).find(|row| has_line(&rows[*row]))?;
        let last = (first..=last).rev().find(|row| has_line(&rows[*row]))?;
        Some((first, last))
    }

    fn offsets(&self, first: usize, last: usize, last_len: usize) -> (u32, u32) {
        let (start, end) = self.bounds();
        let whole_last = self.unit == Unit::Lines || end.row != last;
        let start_offset =
            if self.unit == Unit::Text && start.row == first { start.column + 1 } else { 1 };
        let end_offset = if whole_last { last_len + 1 } else { end.column.min(last_len) + 1 };
        (start_offset as u32, end_offset as u32)
    }

    fn remap(&mut self, old: &[Row], new: &[Row]) -> bool {
        self.map_rows(|row| {
            old.get(row).and_then(|wanted| new.iter().position(|candidate| candidate == wanted))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_code::diff_rows::{Layout, RowInputs, build};
    use cv_widgets::text_selection::{Caret, RowSelection};
    use review_git::FileDiff;

    fn rows() -> (FileDiff, Vec<Row>) {
        let diff = FileDiff::parse_unified(
            b"@@ -1,4 +1,4 @@\n alpha beta\n-gamma\n+delta\n epsilon\n zeta\n",
        );
        let rows = build(&RowInputs {
            diff: &diff,
            layout: Layout::Inline,
            gaps: &[],
            reveals: &[],
            threads: &[],
            composer: None,
        })
        .rows;
        (diff, rows)
    }

    #[test]
    fn copies_characters_across_lines_without_gutter() {
        let (diff, rows) = rows();
        let text_of = |row: usize| {
            rows.get(row)
                .and_then(|row| row.right.or(row.left))
                .map(|index| diff.text(&diff.lines()[index as usize]))
        };
        let mut selection = Selection::caret(Pane::Inline, Caret::new(0, 6));
        selection.extend_to(Caret::new(2, 3));
        assert_eq!(selection.text(text_of), "beta\ngamma\ndel");
        // Dragging back up over the start keeps working.
        selection.extend_to(Caret::new(0, 2));
        assert_eq!(selection.text(text_of), "pha ");
        let lines = Selection::lines(Pane::Inline, 1, 2);
        assert_eq!(lines.text(text_of), "gamma\ndelta\n");
    }

    #[test]
    fn selects_rows_in_its_pane_only() {
        let mut selection = Selection::caret(Pane::Left, Caret::new(1, 2));
        selection.extend_to(Caret::new(3, 1));
        assert_eq!(selection.on_row(2, Pane::Right, 5), None, "side by side keeps one pane");
        assert_eq!(
            selection.on_row(1, Pane::Left, 5),
            Some(RowSelection::Text { columns: 2..5, through_end: true })
        );
        assert_eq!(
            selection.on_row(2, Pane::Left, 5),
            Some(RowSelection::Text { columns: 0..5, through_end: true })
        );
        assert_eq!(
            selection.on_row(3, Pane::Left, 5),
            Some(RowSelection::Text { columns: 0..1, through_end: false })
        );
        assert_eq!(selection.on_row(4, Pane::Left, 5), None);
        assert_eq!(
            Selection::line(Pane::Left, 3).on_row(3, Pane::Left, 0),
            Some(RowSelection::Line)
        );
    }

    #[test]
    fn picks_the_commented_lines_and_their_offsets() {
        let (_, rows) = rows();
        let every = |_: &Row| true;
        let mut selection = Selection::caret(Pane::Inline, Caret::new(1, 2));
        selection.extend_to(Caret::new(3, 0));
        assert_eq!(
            selection.line_rows(&rows, every),
            Some((1, 2)),
            "ending at a line start leaves that line out"
        );
        assert_eq!(selection.offsets(1, 2, 5), (3, 6));
        selection.extend_to(Caret::new(3, 4));
        assert_eq!(selection.offsets(1, 3, 7), (3, 5));
        assert_eq!(
            Selection::caret(Pane::Inline, Caret::new(1, 1)).line_rows(&rows, every),
            None,
            "nothing selected"
        );
        assert_eq!(Selection::lines(Pane::Inline, 0, 2).offsets(0, 2, 5), (1, 6));
    }

    #[test]
    fn follows_its_rows_into_rebuilt_rows() {
        let (_, rows) = rows();
        let mut grown = rows.clone();
        grown.insert(1, rows[4]);
        let mut selection = Selection::lines(Pane::Inline, 1, 2);
        assert!(selection.remap(&rows, &grown));
        assert_eq!(selection.bounds().0.row, 2);
        assert!(!selection.remap(&rows, &rows[..2]), "a row that is gone ends the selection");
    }
}
