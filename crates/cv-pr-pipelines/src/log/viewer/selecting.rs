//! Selecting log text with the pointer, on the shared [`text_selection`] model.
//!
//! - Drag selects characters; double-click a word, triple-click the line.
//! - Click or drag the line numbers to pick whole lines; Shift+click extends.
//! - Ctrl+A (while the log has focus) selects every line; Ctrl+C copies the selection as plain
//!   text, without timestamps or ANSI escapes, exactly as "Copy log" does for the whole log.
//!
//! Positions are (line, character) in a line's text after its timestamp: the timestamp is never
//! highlighted, since it is never copied. Only the visible lines paint the selection.

use cv_base::theme::Palette;
use cv_widgets::numbers::group_digits;
use cv_widgets::text_selection::{self, Caret, RowSelection, Selection};
use egui::{Color32, CursorIcon, FontId, Galley, Painter, Pos2, Rect, Response, Ui, vec2};

use super::super::ansi;
use super::raw_log;

/// Where the log's lines sit this frame.
pub(super) struct Rows<'a> {
    /// Top of line 0, in screen points (above the view when scrolled down).
    pub top: f32,
    /// Distance from one line's top to the next one's.
    pub pitch: f32,
    /// Left edge of the line text, right of the line numbers.
    pub text_left: f32,
    pub line_count: usize,
    pub font: &'a FontId,
}

impl Rows<'_> {
    /// The line at height `y`, clamped to the log.
    fn line_at(&self, y: f32) -> usize {
        let line = ((y - self.top) / self.pitch).floor().max(0.0) as usize;
        line.min(self.line_count.saturating_sub(1))
    }
}

#[derive(Debug, Default)]
pub(super) struct Selecting {
    pub selection: Option<Selection>,
    /// A press on the lines is being dragged.
    pub dragging: bool,
    /// Where dragging past the top or bottom edge scrolls the view to next frame.
    pub scroll_offset: Option<f32>,
}

impl Selecting {
    /// The selection, unless it is empty.
    pub fn current(&self) -> Option<Selection> {
        self.selection.filter(|selection| !selection.is_empty())
    }

    /// What a screen reader hears for the lines: "Log lines, 3 lines selected".
    pub fn label(&self) -> String {
        let Some(selection) = self.current() else {
            return "Log lines".to_owned();
        };
        let (start, end) = selection.bounds();
        match end.row - start.row + 1 {
            1 => "Log lines, 1 line selected".to_owned(),
            count => format!("Log lines, {} lines selected", group_digits(count)),
        }
    }

    /// Presses, drags and multi-clicks on the lines; runs before the visible lines paint.
    /// `line_text` gives the raw text of a line that is read in.
    pub fn track_pointer<'t>(
        &mut self,
        ui: &Ui,
        area: &Response,
        rows: &Rows<'_>,
        palette: &Palette,
        line_text: impl Fn(usize) -> Option<&'t str>,
    ) {
        if rows.line_count == 0 {
            return;
        }
        let (pressed, down, shift, position) = ui.input(|input| {
            (
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
                input.modifiers.shift,
                input.pointer.interact_pos(),
            )
        });
        if area.hovered()
            && let Some(hover) = ui.ctx().pointer_hover_pos()
        {
            ui.ctx().set_cursor_icon(if hover.x < rows.text_left {
                CursorIcon::PointingHand
            } else {
                CursorIcon::Text
            });
        }
        let hit = |position: Pos2| {
            let line = rows.line_at(position.y);
            let column = line_text(line).map_or(0, |text| {
                column_at(ui, text, palette, rows.font, position.x - rows.text_left)
            });
            (Caret::new(line, column), position.x < rows.text_left)
        };
        match position {
            Some(position) if pressed && area.hovered() => {
                let (caret, in_gutter) = hit(position);
                self.selection =
                    Some(Selection::press(self.selection, (), caret, in_gutter, shift));
                self.dragging = true;
                area.request_focus();
            }
            Some(position) if down && self.dragging => {
                if let Some(selection) = self.selection.as_mut() {
                    selection.drag_to(hit(position).0);
                }
            }
            _ => {}
        }
        if !down {
            self.dragging = false;
        }
        let (double, triple) = (area.double_clicked(), area.triple_clicked());
        if let Some(position) = position.filter(|_| double || triple) {
            let (caret, in_gutter) = hit(position);
            if !in_gutter {
                self.selection = if triple {
                    Some(Selection::line((), caret.row))
                } else {
                    line_text(caret.row).map(|text| {
                        Selection::word((), caret.row, &raw_log::body(text), caret.column)
                    })
                };
            }
        }
    }

    /// Dragging past the top or bottom of `view` scrolls on, so a selection can grow beyond it.
    /// `offset` is the current scroll offset, `limit` the largest one.
    pub fn scroll_while_dragging(&mut self, ui: &Ui, view: Rect, offset: f32, limit: f32) {
        if !self.dragging {
            return;
        }
        let Some(position) = ui.input(|input| input.pointer.interact_pos()) else {
            return;
        };
        if let Some(step) = text_selection::edge_scroll_step(view, position.y) {
            self.scroll_offset = Some((offset + step).clamp(0.0, limit.max(0.0)));
            ui.ctx().request_repaint();
        }
    }

    /// How line `line` with raw text `text` is selected.
    pub fn on_line(&self, line: usize, text: &str) -> Option<RowSelection> {
        let selection = self.selection?;
        let (start, end) = selection.bounds();
        if line < start.row || line > end.row {
            return None;
        }
        selection.on_row(line, (), raw_log::body(text).chars().count())
    }
}

/// How the selection paints this frame.
#[derive(Debug, Clone, Copy)]
pub(super) struct Fill {
    pub color: Color32,
    /// Advance of one character of the log font.
    pub char_width: f32,
}

/// Paints the selection fill of one visible line: `band` is the line's full height, the gutter
/// included, and `galley` (painted at `text_at`) the line as shown, timestamp first.
pub(super) fn paint_line(
    painter: &Painter,
    selected: &RowSelection,
    band: Rect,
    text: &str,
    galley: &Galley,
    text_at: Pos2,
    fill: Fill,
) {
    let RowSelection::Text { columns, through_end } = selected else {
        painter.rect_filled(band, 0, fill.color);
        return;
    };
    let skip = timestamp_chars(text);
    let selected = columns.start + skip..columns.end + skip;
    for rect in text_selection::selection_rects(galley, selected, *through_end, fill.char_width) {
        let rect = Rect::from_x_y_ranges(
            text_at.x + rect.left()..=text_at.x + rect.right(),
            band.y_range(),
        );
        painter.rect_filled(rect, 0, fill.color);
    }
}

/// Characters of the platform timestamp at the start of `text`, shown but never selected.
fn timestamp_chars(text: &str) -> usize {
    let timestamp_len = text.len() - ansi::strip_timestamp(text).len();
    text[..timestamp_len].chars().count()
}

/// Character of the line's text (after the timestamp) nearest to `x`, measured from where the
/// line's text starts, laid out as the line paints.
fn column_at(ui: &Ui, text: &str, palette: &Palette, font: &FontId, x: f32) -> usize {
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(ansi::layout(text, palette, font)));
    let index = galley.cursor_from_pos(vec2(x, galley.size().y / 2.0)).index.0;
    index.saturating_sub(timestamp_chars(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_the_timestamp_only() {
        assert_eq!(timestamp_chars("2026-10-18T09:00:00.1000000Z ##[section]Start"), 29);
        assert_eq!(timestamp_chars("plain line"), 0);
    }

    #[test]
    fn finds_lines_by_height() {
        let font = FontId::monospace(13.0);
        let rows = Rows { top: 100.0, pitch: 20.0, text_left: 50.0, line_count: 10, font: &font };
        assert_eq!(rows.line_at(100.0), 0);
        assert_eq!(rows.line_at(139.0), 1);
        assert_eq!(rows.line_at(-500.0), 0, "above the log");
        assert_eq!(rows.line_at(9000.0), 9, "below the log");
    }
}
