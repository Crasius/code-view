//! Text and line selection over rows of monospace text, shared by the diff and the pipeline log.
//!
//! A selection is two spans of (row, character), so selecting 100k lines costs nothing until the
//! visible rows paint or the text is copied. `P` tells panes apart (the two sides of a
//! side-by-side diff keep a selection in one of them, as GitHub and Azure DevOps do); a view with
//! a single pane uses `()`.
//!
//! The views own hit testing and painting; this module decides what a press, a drag, a
//! multi-click or a shortcut does, and where the selection fill goes on a laid-out line.

use std::ops::Range;

use egui::emath::GuiRounding as _;
use egui::epaint::text::CharIndex;
use egui::{Galley, InputState, Key, Modifiers, Rect};

/// How close to the top or bottom edge of the text a drag starts scrolling.
const SCROLL_MARGIN: f32 = 12.0;
/// Fastest scroll while dragging past the edge, in points per frame.
const MAX_SCROLL_STEP: f32 = 30.0;

/// A position in the text: a row and a character offset (characters, not bytes; a tab counts
/// once) in the line shown there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Caret {
    pub row: usize,
    pub column: usize,
}

impl Caret {
    pub fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Characters, or a word after a double-click.
    Text,
    /// Whole lines: picked in the gutter, by a triple-click or by Ctrl+A.
    Lines,
}

/// How one row of a pane is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowSelection {
    /// Characters `columns` of the line; `through_end` when the selection runs on into the next line.
    Text { columns: Range<usize>, through_end: bool },
    /// The whole line, gutter included.
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection<P = ()> {
    pub pane: P,
    pub unit: Unit,
    /// The span picked first (a caret, a word or a line); the selection always covers it.
    origin: (Caret, Caret),
    /// The span under the pointer or the Shift+click.
    head: (Caret, Caret),
}

impl<P: Copy + Eq> Selection<P> {
    /// An empty selection at `caret`, where a drag starts.
    pub fn caret(pane: P, caret: Caret) -> Self {
        Self::span(pane, Unit::Text, caret, caret)
    }

    pub fn span(pane: P, unit: Unit, start: Caret, end: Caret) -> Self {
        Self { pane, unit, origin: (start, end), head: (start, end) }
    }

    /// Line `row` in `pane`.
    pub fn line(pane: P, row: usize) -> Self {
        Self::span(pane, Unit::Lines, Caret::new(row, 0), Caret::new(row, 0))
    }

    /// Lines `first..=last`, as Ctrl+A picks them.
    pub fn lines(pane: P, first: usize, last: usize) -> Self {
        Self {
            pane,
            unit: Unit::Lines,
            origin: (Caret::new(first, 0), Caret::new(first, 0)),
            head: (Caret::new(last, 0), Caret::new(last, 0)),
        }
    }

    /// The word, run of spaces or single symbol at `column` of `line` (shown on row `row`), as a
    /// double-click picks it.
    pub fn word(pane: P, row: usize, line: &str, column: usize) -> Self {
        let word = word_at(line, column);
        Self::span(pane, Unit::Text, Caret::new(row, word.start), Caret::new(row, word.end))
    }

    /// What a primary press at `at` makes of `current`: Shift extends it when it is in the same
    /// pane (to whole lines when pressed in the gutter), a press in the gutter picks the line,
    /// anywhere else puts the caret there for a drag.
    pub fn press(current: Option<Self>, pane: P, at: Caret, in_gutter: bool, shift: bool) -> Self {
        let extended = current.filter(|selection| shift && selection.pane == pane);
        match (extended, in_gutter) {
            (Some(mut selection), true) if selection.unit == Unit::Lines => {
                selection.extend_to(Caret::new(at.row, 0));
                selection
            }
            (Some(selection), true) => Self::lines(pane, selection.bounds().0.row, at.row),
            (Some(mut selection), false) => {
                selection.drag_to(at);
                selection
            }
            (None, true) => Self::line(pane, at.row),
            (None, false) => Self::caret(pane, at),
        }
    }

    /// Moves the free end to `caret`, keeping the span picked first.
    pub fn extend_to(&mut self, caret: Caret) {
        self.head = (caret, caret);
    }

    /// Follows a drag to `at`; a selection of whole lines ignores the column.
    pub fn drag_to(&mut self, at: Caret) {
        let column = if self.unit == Unit::Lines { 0 } else { at.column };
        self.extend_to(Caret::new(at.row, column));
    }

    /// The free end was moved before the span picked first: a drag up, a Shift+click above.
    pub fn is_backward(&self) -> bool {
        self.head.0 < self.origin.0
    }

    /// Where the free end is: the caret a drag, a Shift+click or Shift+arrow left last.
    pub fn focus(&self) -> Caret {
        if self.is_backward() { self.head.0 } else { self.head.1 }
    }

    /// First and last caret.
    pub fn bounds(&self) -> (Caret, Caret) {
        (self.origin.0.min(self.head.0), self.origin.1.max(self.head.1))
    }

    pub fn is_empty(&self) -> bool {
        let (start, end) = self.bounds();
        self.unit == Unit::Text && start == end
    }

    /// What of row `row` in `pane` is selected; `len` is the character count of its line.
    pub fn on_row(&self, row: usize, pane: P, len: usize) -> Option<RowSelection> {
        let (start, end) = self.bounds();
        if pane != self.pane || row < start.row || row > end.row {
            return None;
        }
        if self.unit == Unit::Lines {
            return Some(RowSelection::Line);
        }
        let from = if row == start.row { start.column.min(len) } else { 0 };
        let to = if row == end.row { end.column.min(len) } else { len };
        let through_end = row < end.row;
        (from < to || through_end)
            .then_some(RowSelection::Text { columns: from..to.max(from), through_end })
    }

    /// The selected text, without line numbers or gutter. `line_text` gives the line row `row`
    /// shows in the selected pane; rows without one (gaps, comments, the empty side of a
    /// side-by-side row) are left out. Whole lines end with a newline each.
    pub fn text<S: AsRef<str>>(&self, mut line_text: impl FnMut(usize) -> Option<S>) -> String {
        let (start, end) = self.bounds();
        let mut text = String::new();
        let mut is_first = true;
        for row in start.row..=end.row {
            let Some(line) = line_text(row) else {
                continue;
            };
            let line = line.as_ref();
            if self.unit == Unit::Lines {
                text.push_str(line);
                text.push('\n');
                continue;
            }
            if !is_first {
                text.push('\n');
            }
            is_first = false;
            let from = if row == start.row { start.column } else { 0 };
            let to = if row == end.row { end.column } else { usize::MAX };
            text.push_str(char_slice(line, from, to));
        }
        text
    }

    /// Moves the selection to the rows `row_of` maps its rows to; `false` (and unchanged) when
    /// one of them is gone.
    pub fn map_rows(&mut self, mut row_of: impl FnMut(usize) -> Option<usize>) -> bool {
        let mut carets = [self.origin.0, self.origin.1, self.head.0, self.head.1];
        for caret in &mut carets {
            match row_of(caret.row) {
                Some(row) => caret.row = row,
                None => return false,
            }
        }
        self.origin = (carets[0], carets[1]);
        self.head = (carets[2], carets[3]);
        true
    }
}

/// Characters `from..to` of `text` (clamped).
pub(crate) fn char_slice(text: &str, from: usize, to: usize) -> &str {
    let byte = |column: usize| text.char_indices().nth(column).map_or(text.len(), |(byte, _)| byte);
    let (start, end) = (byte(from), byte(to.max(from)));
    &text[start..end]
}

/// The word (letters, digits, `_`), run of spaces or single other character at `column`.
pub(crate) fn word_at(text: &str, column: usize) -> Range<usize> {
    let characters: Vec<char> = text.chars().collect();
    if characters.is_empty() {
        return 0..0;
    }
    let at = column.min(characters.len() - 1);
    let class = |character: char| {
        if character.is_alphanumeric() || character == '_' {
            0
        } else if character.is_whitespace() {
            1
        } else {
            2
        }
    };
    let wanted = class(characters[at]);
    if wanted == 2 {
        return at..at + 1;
    }
    let start = characters[..at]
        .iter()
        .rposition(|character| class(*character) != wanted)
        .map_or(0, |index| index + 1);
    let end = characters[at..]
        .iter()
        .position(|character| class(*character) != wanted)
        .map_or(characters.len(), |index| at + index);
    start..end
}

/// Ctrl+C this frame. egui-winit turns Ctrl+C (and Ctrl+Shift+C) into a copy event; a bare key
/// press (tests, other back ends) counts too and is consumed.
pub fn copy_pressed(input: &mut InputState) -> bool {
    input.events.iter().any(|event| matches!(event, egui::Event::Copy))
        | input.consume_key(Modifiers::COMMAND, Key::C)
}

/// Ctrl+A this frame, consumed.
pub fn select_all_pressed(input: &mut InputState) -> bool {
    input.consume_key(Modifiers::COMMAND, Key::A)
}

/// How far to scroll this frame while a drag holds the pointer near or past the top or bottom
/// of `view`, so a selection can grow beyond what is shown; `None` away from the edges.
pub fn edge_scroll_step(view: Rect, pointer_y: f32) -> Option<f32> {
    let above = view.top() + SCROLL_MARGIN - pointer_y;
    let below = pointer_y - (view.bottom() - SCROLL_MARGIN);
    if above > 0.0 {
        Some(-above.min(MAX_SCROLL_STEP))
    } else if below > 0.0 {
        Some(below.min(MAX_SCROLL_STEP))
    } else {
        None
    }
}

/// Where the characters of a painted line sit, for the selection fill behind them.
pub enum LineGrid<'a> {
    /// Monospace text stamped from glyphs (see `mono_text`): every column `advance` wide, and
    /// `per_row` columns to a visual row when the line wraps after them. Stamped galleys carry no
    /// glyphs to measure, so this is the only way to know where their characters are.
    Mono { advance: f32, columns: usize, per_row: Option<usize> },
    /// A line laid out by egui, wrapped or not.
    Laid(&'a Galley),
}

impl<'a> LineGrid<'a> {
    /// The grid of `galley` as painted: stamped galleys (no glyphs) on columns `char_width` apart.
    pub(crate) fn of_galley(galley: &'a Galley, char_width: f32) -> Self {
        if galley.rows.iter().all(|placed| placed.row.glyphs.is_empty()) {
            Self::Mono { advance: char_width, columns: usize::MAX, per_row: None }
        } else {
            Self::Laid(galley)
        }
    }

    /// The laid-out columns of each visual row.
    fn rows(&self) -> Vec<Range<usize>> {
        match self {
            Self::Mono { columns, per_row, .. } => {
                let per_row = per_row.unwrap_or(*columns).max(1);
                (0..columns.div_ceil(per_row).max(1))
                    .map(|row| row * per_row..(row + 1).saturating_mul(per_row).min(*columns))
                    .collect()
            }
            Self::Laid(galley) => {
                let mut first = 0;
                galley
                    .rows
                    .iter()
                    .map(|placed| {
                        let count = placed.char_count_excluding_newline().0;
                        first += count;
                        first - count..first
                    })
                    .collect()
            }
        }
    }

    /// X of the left edge of `column` (counted from the start of visual row `row`).
    fn x(&self, row: usize, column: usize) -> f32 {
        match self {
            Self::Mono { advance, .. } => column as f32 * advance,
            Self::Laid(galley) => galley
                .rows
                .get(row)
                .map_or(0.0, |placed| placed.pos.x + placed.x_offset(CharIndex(column))),
        }
    }
}

/// Width of the newline a selection shows after a line's end when it runs on into the next line,
/// as editors do: an empty line in the middle of a selection still shows as selected.
pub fn newline_width(char_width: f32) -> f32 {
    char_width * 0.6
}

/// Per visual row of a line on `grid`: the row and the x-range (from where the text starts)
/// behind the laid-out characters `selected` (tabs expanded); `newline` adds that much after the
/// end of the last row.
pub fn selection_spans(
    grid: &LineGrid<'_>,
    selected: Range<usize>,
    newline: Option<f32>,
) -> Vec<(usize, Range<f32>)> {
    let rows = grid.rows();
    let last = rows.len().saturating_sub(1);
    rows.iter()
        .enumerate()
        .filter_map(|(row, columns)| {
            let from = selected.start.clamp(columns.start, columns.end) - columns.start;
            let to = selected.end.clamp(columns.start, columns.end) - columns.start;
            let (left, mut right) = (grid.x(row, from), grid.x(row, to));
            if row == last
                && let Some(newline) = newline
            {
                right += newline;
            }
            (right > left).then_some((row, left..right))
        })
        .collect()
}

/// The fills of `spans` ([`selection_spans`]) for a line whose text starts at `text_x` and whose
/// rows, `row_height` apart, fill `band`: each a whole row high and the last reaching down to the
/// band's bottom, so the selected lines of a block meet without a gap. Whole pixels, so two lines
/// share their edge exactly.
pub fn selection_fills(
    spans: Vec<(usize, Range<f32>)>,
    text_x: f32,
    band: Rect,
    row_height: f32,
    pixels_per_point: f32,
) -> impl Iterator<Item = Rect> {
    spans.into_iter().map(move |(row, span)| {
        let top = band.top() + row as f32 * row_height;
        let bottom =
            if top + row_height * 1.5 >= band.bottom() { band.bottom() } else { top + row_height };
        Rect::from_x_y_ranges(text_x + span.start..=text_x + span.end, top..=bottom)
            .round_to_pixels(pixels_per_point)
    })
}

/// Rectangles behind the laid-out characters `selected` (tabs expanded) of `galley`, row by row
/// of a wrapped line, relative to the galley. `through_end` adds a newline's width after the
/// last character, as editors show a selection that runs on into the next line.
pub fn selection_rects(
    galley: &Galley,
    selected: Range<usize>,
    through_end: bool,
    char_width: f32,
) -> Vec<Rect> {
    let newline = through_end.then(|| newline_width(char_width));
    selection_spans(&LineGrid::of_galley(galley, char_width), selected, newline)
        .into_iter()
        .filter_map(|(row, span)| {
            galley
                .rows
                .get(row)
                .map(|placed| Rect::from_x_y_ranges(span.start..=span.end, placed.rect().y_range()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Color32, Context, FontId, RawInput};

    #[test]
    fn copies_characters_across_lines() {
        let lines = ["alpha beta", "gamma", "delta", "epsilon"];
        let line = |row: usize| lines.get(row).copied();
        let mut selection = Selection::caret((), Caret::new(0, 6));
        selection.extend_to(Caret::new(2, 3));
        assert_eq!(selection.text(line), "beta\ngamma\ndel");
        selection.extend_to(Caret::new(0, 2));
        assert_eq!(selection.text(line), "pha ", "dragging back up over the start keeps working");
        assert_eq!(Selection::lines((), 1, 2).text(line), "gamma\ndelta\n");
        let gaps = |row: usize| (row != 1).then(|| lines[row]);
        assert_eq!(
            Selection::lines((), 0, 2).text(gaps),
            "alpha beta\ndelta\n",
            "rows without a line are left out"
        );
    }

    #[test]
    fn selects_rows_in_its_pane_only() {
        let mut selection = Selection::caret(0, Caret::new(1, 2));
        selection.extend_to(Caret::new(3, 1));
        assert_eq!(selection.on_row(2, 1, 5), None, "side by side keeps one pane");
        assert_eq!(
            selection.on_row(1, 0, 5),
            Some(RowSelection::Text { columns: 2..5, through_end: true })
        );
        assert_eq!(
            selection.on_row(2, 0, 5),
            Some(RowSelection::Text { columns: 0..5, through_end: true })
        );
        assert_eq!(
            selection.on_row(3, 0, 5),
            Some(RowSelection::Text { columns: 0..1, through_end: false })
        );
        assert_eq!(selection.on_row(4, 0, 5), None);
        assert_eq!(Selection::line(0, 3).on_row(3, 0, 0), Some(RowSelection::Line));
    }

    #[test]
    fn presses_drags_and_shift_clicks() {
        let caret = Selection::press(None, (), Caret::new(4, 3), false, false);
        assert_eq!(caret, Selection::caret((), Caret::new(4, 3)));
        let mut dragged = caret;
        dragged.drag_to(Caret::new(6, 2));
        assert_eq!(dragged.bounds(), (Caret::new(4, 3), Caret::new(6, 2)));
        let extended = Selection::press(Some(dragged), (), Caret::new(9, 1), false, true);
        assert_eq!(
            extended.bounds(),
            (Caret::new(4, 3), Caret::new(9, 1)),
            "Shift+click keeps the start"
        );
        assert_eq!(
            Selection::press(Some(dragged), (), Caret::new(9, 1), false, false),
            Selection::caret((), Caret::new(9, 1))
        );
        let in_gutter = Selection::press(Some(dragged), (), Caret::new(8, 0), true, true);
        assert_eq!(
            (in_gutter.unit, in_gutter.bounds().1.row),
            (Unit::Lines, 8),
            "Shift+click in the gutter takes whole lines"
        );
        let mut lines = Selection::press(None, (), Caret::new(2, 5), true, false);
        assert_eq!(lines, Selection::line((), 2));
        lines.drag_to(Caret::new(5, 7));
        assert_eq!(lines.bounds().1, Caret::new(5, 0), "whole lines ignore the column");
        let other_pane = Selection::press(
            Some(Selection::caret(1, Caret::new(0, 0))),
            2,
            Caret::new(3, 1),
            false,
            true,
        );
        assert_eq!(
            other_pane,
            Selection::caret(2, Caret::new(3, 1)),
            "Shift+click in another pane starts over"
        );
    }

    #[test]
    fn knows_which_end_the_user_moved() {
        let mut selection = Selection::caret((), Caret::new(4, 3));
        selection.drag_to(Caret::new(6, 2));
        assert_eq!(
            (selection.is_backward(), selection.focus()),
            (false, Caret::new(6, 2)),
            "a drag down ends at the last line"
        );
        selection.drag_to(Caret::new(1, 5));
        assert_eq!(
            (selection.is_backward(), selection.focus()),
            (true, Caret::new(1, 5)),
            "a drag up ends at the first line"
        );
        assert_eq!(selection.bounds(), (Caret::new(1, 5), Caret::new(4, 3)));
        let word = Selection::word((), 2, "let total = 0;", 6);
        assert_eq!(
            (word.is_backward(), word.focus()),
            (false, Caret::new(2, 9)),
            "a word ends where it ends"
        );
    }

    #[test]
    fn follows_its_rows() {
        let mut selection = Selection::lines((), 1, 2);
        assert!(selection.map_rows(|row| Some(row + 10)));
        assert_eq!(selection.bounds().0.row, 11);
        assert!(
            !selection.map_rows(|row| (row == 11).then_some(0)),
            "a row that is gone ends the selection"
        );
        assert_eq!(selection.bounds().0.row, 11, "and leaves it as it was");
    }

    #[test]
    fn finds_words_spaces_and_symbols() {
        let text = "let total_cents = a+b;";
        assert_eq!(word_at(text, 6), 4..15);
        assert_eq!(word_at(text, 3), 3..4);
        assert_eq!(word_at(text, 19), 19..20, "a symbol is a word of its own");
        assert_eq!(word_at(text, 99), 21..22);
        assert_eq!(word_at("", 0), 0..0);
        assert_eq!(
            Selection::word((), 7, text, 6),
            Selection::span((), Unit::Text, Caret::new(7, 4), Caret::new(7, 15))
        );
        assert_eq!(char_slice("äbc", 1, 2), "b");
        assert_eq!(char_slice("äbc", 2, usize::MAX), "c");
    }

    #[test]
    fn scrolls_only_near_the_edges() {
        let view = Rect::from_min_max(egui::pos2(0.0, 100.0), egui::pos2(500.0, 400.0));
        assert_eq!(edge_scroll_step(view, 250.0), None);
        assert_eq!(edge_scroll_step(view, 105.0), Some(-7.0));
        assert_eq!(edge_scroll_step(view, 0.0), Some(-MAX_SCROLL_STEP));
        assert_eq!(edge_scroll_step(view, 395.0), Some(7.0));
    }

    #[test]
    fn wrapped_lines_fill_whole_rows_that_meet() {
        // 10 columns, 4 to a visual row: 0..4, 4..8, 8..10.
        let grid = LineGrid::Mono { advance: 2.0, columns: 10, per_row: Some(4) };
        assert_eq!(
            selection_spans(&grid, 2..10, Some(1.0)),
            [(0, 4.0..8.0), (1, 0.0..8.0), (2, 0.0..5.0)]
        );
        assert_eq!(selection_spans(&grid, 3..3, None), Vec::<(usize, Range<f32>)>::new());
        assert_eq!(
            selection_spans(&grid, 10..10, Some(1.0)),
            [(2, 4.0..5.0)],
            "a selection past the end still shows the newline"
        );
        let band = Rect::from_min_max(egui::pos2(0.0, 100.0), egui::pos2(300.0, 160.0));
        let fills: Vec<Rect> =
            selection_fills(selection_spans(&grid, 0..10, None), 50.0, band, 20.0, 1.0).collect();
        let rows: Vec<(f32, f32, f32, f32)> = fills
            .iter()
            .map(|fill| (fill.left(), fill.right(), fill.top(), fill.bottom()))
            .collect();
        assert_eq!(
            rows,
            [(50.0, 58.0, 100.0, 120.0), (50.0, 58.0, 120.0, 140.0), (50.0, 54.0, 140.0, 160.0)],
            "whole rows, from the first column"
        );
    }

    /// Stamped and laid-out galleys of the same ASCII line get the same selection fill, to within
    /// the pixel egui snaps each glyph to.
    #[test]
    fn measures_stamped_and_laid_out_lines_alike() {
        let ctx = Context::default();
        cv_base::theme::fonts::install(&ctx);
        let font = FontId::monospace(13.0);
        let mut result = None;
        for _ in 0..2 {
            let output = ctx.run_ui(RawInput::default(), |ui| {
                let job = egui::text::LayoutJob::simple_singleline(
                    "let total = 42;".to_owned(),
                    font.clone(),
                    Color32::WHITE,
                );
                let stamped = crate::mono_text::layout_job(ui.ctx(), job.clone());
                let laid_out = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
                let char_width = ui.ctx().fonts_mut(|fonts| fonts.glyph_width(&font, '0'));
                result = Some((stamped, laid_out, char_width));
            });
            output.drop_without_applying_deltas();
        }
        let (stamped, laid_out, char_width) = result.expect("ran");
        assert!(stamped.rows[0].row.glyphs.is_empty(), "the line is stamped");
        let from_stamped = selection_rects(&stamped, 4..9, true, char_width);
        let from_layout = selection_rects(&laid_out, 4..9, true, char_width);
        assert_eq!(from_stamped.len(), 1);
        assert_eq!(from_layout.len(), 1);
        for (stamped, laid_out) in [
            (from_stamped[0].left(), from_layout[0].left()),
            (from_stamped[0].right(), from_layout[0].right()),
        ] {
            assert!(
                (stamped - laid_out).abs() < 1.0,
                "{stamped} vs {laid_out}: more than a pixel apart"
            );
        }
        assert_eq!(selection_rects(&stamped, 3..3, false, char_width), Vec::<Rect>::new());
    }
}
