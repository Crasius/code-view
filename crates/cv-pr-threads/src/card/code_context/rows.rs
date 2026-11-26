//! The excerpt's code rows, painted by the diff's own row painter with the diff's metrics, colors
//! and line styles: one pane in New only and Diff; in Split the base version on the left and the
//! head version on the right, each with its own line numbers and clipped at its edge, a divider
//! between them. The line numbers of either side lead to the thread's line in the Files tab.

use std::ops::Range;

use cv_base::theme;
use cv_code::diff_paint::{
    self, Invisibles, LineStyle, Metrics, PaintContext, PaneLine, RangeMark, TEXT_END_PADDING,
};
use cv_code::line_source::CodeLine;
use cv_widgets::focus;
use egui::{Rangef, Rect, Sense, Ui, vec2};
use review_git::LineKind;

use super::code_file::{CodeFile, CodeRow};
use super::window::{Focus, Mode, Shown};

/// Code columns each side of Split needs (our own pick): a narrower card shows Diff instead.
const MIN_SPLIT_COLUMNS: f32 = 40.0;

/// Whether rows `width` wide fit Split's two sides with [`MIN_SPLIT_COLUMNS`] of code each.
pub fn fits_split(metrics: &Metrics, width: f32) -> bool {
    let code = width / 2.0 - metrics.text_inset(false) - TEXT_END_PADDING;
    code / metrics.char_width >= MIN_SPLIT_COLUMNS
}

/// The number a row shows in New only and in the excerpt's caption: removed lines count in the
/// base, the others in the head.
pub fn number(row: &CodeRow) -> u32 {
    if row.kind == LineKind::Deleted { row.old_line } else { row.new_line }
}

/// The excerpt's code rows.
pub struct Rows<'a> {
    pub file: &'a CodeFile,
    pub shown: Shown<'a>,
    pub focus: &'a Focus,
    pub mode: Mode,
    pub font: f32,
}

impl Rows<'_> {
    /// Paints the rows at `lines` of the shown ones; true when a line number was clicked.
    pub fn show(&self, ui: &mut Ui, lines: &Range<usize>) -> bool {
        let palette = theme::palette(ui.ctx());
        let metrics = Metrics::new(ui, self.font, self.file.max_line());
        let line_height = metrics.line_height;
        let (rect, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), lines.len() as f32 * line_height),
            Sense::hover(),
        );
        let columns = self.columns(rect);
        let y_of = |index: usize| rect.top() + index as f32 * line_height;
        let pane = |index: usize, column: Rangef| {
            Rect::from_x_y_ranges(column, Rangef::new(y_of(index), y_of(index + 1)))
        };
        // Each line reads as text to assistive technology; the gutters on top of them take clicks.
        for (index, position) in lines.clone().enumerate() {
            for (slot, row) in self.labeled(position) {
                let column = if slot == 2 { rect.x_range() } else { columns[slot] };
                let id = ui.scope_id().with(("line", position, slot));
                let response = ui.interact(pane(index, column), id, Sense::hover());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Label, true, self.accessible(row))
                });
            }
        }
        let has_two_numbers = self.mode == Mode::Diff;
        let mut is_clicked = false;
        let mut hovered = None;
        for (slot, column) in columns.iter().enumerate() {
            let gutter = Rect::from_x_y_ranges(
                Rangef::new(column.min, column.min + metrics.text_inset(has_two_numbers)),
                rect.y_range(),
            );
            let response = self.gutter(ui, gutter, slot);
            if let Some(pointer) = response.hover_pos() {
                hovered = Some((slot, ((pointer.y - rect.top()) / line_height).floor() as usize));
            }
            focus::ring(ui, &response, gutter);
            is_clicked |= response.clicked();
        }
        for (slot, column) in columns.iter().enumerate() {
            let clip = Rect::from_x_y_ranges(*column, rect.y_range());
            let painter = ui.painter().with_clip_rect(clip.intersect(ui.clip_rect()));
            let context = PaintContext { painter: &painter, palette: &palette, metrics: &metrics };
            for (index, position) in lines.clone().enumerate() {
                let pane = pane(index, *column);
                if ui.is_rect_visible(pane) {
                    let line = self.pane_line(position, slot, hovered == Some((slot, index)));
                    diff_paint::paint_line(&context, pane, &line);
                }
            }
        }
        if let [left, _] = columns[..] {
            diff_paint::paint_divider(ui.painter(), left.max, rect, &palette);
        }
        is_clicked
    }

    /// The panes' horizontal extents: the whole row, or Split's base and head halves.
    fn columns(&self, rect: Rect) -> Vec<Rangef> {
        if self.mode != Mode::Split {
            return vec![rect.x_range()];
        }
        let middle = (rect.left() + rect.width() / 2.0).round();
        vec![Rangef::new(rect.left(), middle), Rangef::new(middle, rect.right())]
    }

    /// The rows at `position` with the pane each is read in: 0 and 1 for Split's sides, 2 for an
    /// unchanged line across both (read once), 0 for the one pane.
    fn labeled(&self, position: usize) -> Vec<(usize, &CodeRow)> {
        if self.mode != Mode::Split {
            return vec![(0, self.shown.row(position))];
        }
        match self.shown.sides(position) {
            [Some(old), Some(new)] if old == new => vec![(2, new)],
            sides => sides
                .into_iter()
                .enumerate()
                .filter_map(|(slot, row)| row.map(|row| (slot, row)))
                .collect(),
        }
    }

    /// The line numbers of pane `slot`, leading to the file. The gutter on the thread's side
    /// names its line; Split's other one says where it leads.
    fn gutter(&self, ui: &mut Ui, gutter: Rect, slot: usize) -> egui::Response {
        let is_thread_side = self.mode != Mode::Split || self.focus.is_old_side() == (slot == 0);
        let name = if is_thread_side {
            format!("Show line {} in the file", self.focus.first())
        } else if slot == 0 {
            "Show the commented line in the file, from the old side".to_owned()
        } else {
            "Show the commented line in the file, from the new side".to_owned()
        };
        let response = ui
            .interact(gutter, ui.scope_id().with(("gutter", slot)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Show in the file");
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &name));
        response
    }

    /// How pane `slot` of the row at `position` is drawn.
    fn pane_line(&self, position: usize, slot: usize, is_hovered: bool) -> PaneLine<'_> {
        let (row, number, style) = match self.mode {
            Mode::Split => {
                let row = self.shown.sides(position)[slot];
                let number =
                    row.map_or(0, |row| if slot == 0 { row.old_line } else { row.new_line });
                (row, number, LineStyle::Diff)
            }
            Mode::Diff => {
                let row = self.shown.row(position);
                (Some(row), number(row), LineStyle::Diff)
            }
            Mode::NewOnly => {
                let row = self.shown.row(position);
                (Some(row), number(row), LineStyle::Focus)
            }
        };
        // An unchanged line is on both sides; the band goes on the side the thread is on.
        let is_marked = row.is_some_and(|row| {
            self.focus.covers(row)
                && (self.mode != Mode::Split
                    || row.kind != LineKind::Context
                    || self.focus.is_old_side() == (slot == 0))
        });
        PaneLine {
            line: row.map(|row| self.code_line(row, number)),
            has_two_numbers: self.mode == Mode::Diff,
            style,
            words: &[],
            h_offset: 0.0,
            wrap_width: None,
            is_hovered,
            hide_marker: false,
            selection: None,
            range: is_marked.then_some(RangeMark::Thread),
            invisibles: Invisibles::default(),
            syntax_mix: 1.0,
        }
    }

    fn code_line(&self, row: &CodeRow, number: u32) -> CodeLine<'_> {
        let spans = self.file.spans(row);
        CodeLine {
            text: self.file.text(row),
            kind: row.kind,
            old_line: row.old_line,
            new_line: row.new_line,
            number,
            spans: spans.unwrap_or_default(),
            has_file_spans: spans.is_some(),
        }
    }

    /// "Line 20, commented: let cap = self.limit;", "Removed line 20: …".
    fn accessible(&self, row: &CodeRow) -> String {
        let kind = match (row.kind, self.mode) {
            (LineKind::Deleted, _) => "Removed line",
            (LineKind::Added, Mode::Diff | Mode::Split) => "Added line",
            (LineKind::Added, Mode::NewOnly) => "Changed line",
            (LineKind::Context | LineKind::Hunk, _) => "Line",
        };
        let commented = if self.focus.covers(row) { ", commented" } else { "" };
        format!("{kind} {}{commented}: {}", number(row), self.file.text(row))
    }
}
