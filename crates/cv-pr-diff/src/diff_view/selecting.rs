//! Selecting in the diff with the pointer and the keyboard, copying the selection, and
//! commenting on the selected lines.
//!
//! - Drag on code selects characters; double-click a word, triple-click the line.
//! - Click, drag or Shift+click line numbers (or drag a line's "+") to pick whole lines.
//! - Shift+click or Shift+↑/↓ extends, Ctrl+A selects the pane's lines, Ctrl+C copies the plain text.
//! - "Comment on lines X–Y" through the comment button where the selection was last moved to (the
//!   last line after a drag down, the first after a drag up, as in Azure DevOps), the context menu
//!   or Alt+C. The comment card opens at that end too: below the last line, or above the first.
//! - "Ask about these lines" through the context menu or Alt+A hands the selection to the assistant.

use egui::{
    Color32, CursorIcon, Key, Modifiers, Pos2, Rect, Response, Sense, Ui, Vec2, pos2, vec2,
};
use review_core::{DiffSide, ThreadAnchor};
use review_store::ChatQuote;

use super::offsets::RowOffsets;
use super::rows::DiffRows;
use super::{DiffInputs, Direction, Frame, pane_rects};
use crate::find::{self, text::Selected};
use crate::selection::{DiffSelection, Selection};
use cv_base::theme;
use cv_code::code_job::{self, JobStyle};
use cv_code::diff_rows::{self, RangePick, Row, RowKind};
use cv_code::line_source::{self, CodeLine, Pane};
use cv_pr_session::assistant_link as assistant;
use cv_widgets::text_selection::{self, Caret, Unit};

/// A drag that is changing the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Drag {
    /// Characters, from a press on the code.
    Text,
    /// Whole lines, from a press on the line numbers.
    Lines,
    /// Whole lines, from dragging a line's "+"; letting go opens a comment on them.
    Plus,
}

/// What "Comment on lines X–Y" comments on.
#[derive(Debug, Clone)]
pub(super) struct RangeTarget {
    pub pane: Pane,
    /// The row the comment button sits on: the selected line the pointer or keyboard left the
    /// selection at, its first or its last.
    pub focus_row: usize,
    /// The selection was moved up: the button sits on its first line, so hints go above it.
    pub focus_is_start: bool,
    /// "line 12" or "lines 12–18".
    pub label: String,
    pub anchor: ThreadAnchor,
    /// Why the platform takes no comment on these lines, said so the user knows what to pick instead.
    pub blocked: Option<&'static str>,
    /// The selection and row build it was worked out for.
    key: (Selection, u64),
}

/// A new comment the selection asks for: on `anchor`'s lines, its card opening at `focus_row`
/// (above it when `above`).
pub(super) struct CommentAt {
    pub anchor: ThreadAnchor,
    pub above: bool,
    pub focus_row: usize,
}

/// The rows a selection is made on, as this frame lays them out.
pub(super) struct Code<'a> {
    pub rows: &'a [Row],
    pub offsets: &'a RowOffsets,
    /// How far each pane is scrolled sideways.
    pub h_offset: [f32; 2],
    /// See [`DiffRows::generation`].
    pub generation: u64,
}

impl<'a> Code<'a> {
    pub(crate) fn new(built: &'a DiffRows, offsets: &'a RowOffsets, h_offset: [f32; 2]) -> Self {
        Self { rows: &built.rows, offsets, h_offset, generation: built.generation }
    }
}

/// Where the selection keys act: `content` with its left pane `left_width` wide, the part of the
/// rows in `view`; Esc lets go of the selection only when `can_escape` (no popover takes it).
pub(super) struct KeysView {
    pub content: Rect,
    pub left_width: f32,
    pub view: Rect,
    pub can_escape: bool,
}

/// What the selection keys ask of the view.
#[derive(Default)]
pub(super) struct KeysWant {
    pub scroll_to: Option<f32>,
    /// Alt+C: comment on the selected lines.
    pub comment: bool,
}

/// Where the pointer is in the code.
#[derive(Debug, Clone, Copy)]
struct Hit {
    row: usize,
    pane: Pane,
    column: usize,
    /// On the line numbers (or the marker column) rather than the code.
    in_gutter: bool,
}

/// Text or lines the user selected, the pointer drag that is making it, and what "Comment on
/// lines X–Y" would comment on.
#[derive(Default)]
pub(super) struct Selecting {
    pub selection: Option<Selection>,
    pub drag: Option<Drag>,
    /// For the selection of this frame.
    pub target: Option<RangeTarget>,
}

impl Selecting {
    /// Some text or lines are selected.
    pub(crate) fn is_active(&self) -> bool {
        self.selection.is_some_and(|selection| !selection.is_empty())
    }

    pub(crate) fn clear(&mut self) {
        self.selection = None;
        self.drag = None;
    }

    /// The area `rect` presses on the code start selections in, read out as the diff of `path`
    /// and what is selected.
    pub(crate) fn area(&self, ui: &Ui, rect: Rect, path: &str) -> Response {
        // Presses on the code start a selection, which the view follows itself (see
        // `track_pointer`): sensing drags here would take them from splitters along the edges of
        // the diff.
        let area = ui.interact(rect, ui.scope_id().with("diff_area"), Sense::click());
        area.widget_info(|| {
            let selected = self
                .target
                .as_ref()
                .map_or_else(String::new, |target| format!(", {} selected", target.label));
            egui::WidgetInfo::labeled(
                egui::Role::Document,
                true,
                format!("Diff of {path}{selected}"),
            )
        });
        area
    }

    /// Works out the comment target for the current selection; cached until rows or selection change.
    pub(crate) fn update_target(&mut self, code: &Code<'_>, inputs: &DiffInputs<'_>) {
        let Some(selection) =
            self.selection.filter(|selection| !selection.is_empty() && inputs.can_comment)
        else {
            self.target = None;
            return;
        };
        let key = (selection, code.generation);
        if self.target.as_ref().is_some_and(|target| target.key == key) {
            return;
        }
        self.target = target_for(code.rows, selection, inputs, key);
    }

    /// The comment on the selected lines, if the platform takes one there; the drag making the
    /// selection ends.
    pub(crate) fn take_comment(&mut self) -> Option<CommentAt> {
        let target = self.target.as_ref().filter(|target| target.blocked.is_none())?;
        let comment = CommentAt {
            anchor: target.anchor.clone(),
            above: target.focus_is_start,
            focus_row: target.focus_row,
        };
        self.drag = None;
        Some(comment)
    }

    /// The selected text, plain: no line numbers, +/− markers or gutter.
    pub(crate) fn text(&self, rows: &[Row], inputs: &DiffInputs<'_>) -> Option<String> {
        let selection = self.selection.filter(|selection| !selection.is_empty())?;
        let text = selection.text(|row| {
            rows.get(row)
                .and_then(|row| {
                    line_source::code_line(inputs.diff, inputs.contents, row, selection.pane)
                })
                .map(|line| line.text)
        });
        (!text.is_empty()).then_some(text)
    }

    /// The text selected within one line (a double-clicked word too), and whether it is a whole
    /// word there: word characters only, with none right before or after it.
    pub(crate) fn in_line(&self, rows: &[Row], inputs: &DiffInputs<'_>) -> Option<Selected> {
        let selection = self
            .selection
            .filter(|selection| !selection.is_empty() && selection.unit == Unit::Text)?;
        let (start, end) = selection.bounds();
        if start.row != end.row {
            return None;
        }
        let row = rows.get(start.row)?;
        let line = line_source::code_line(inputs.diff, inputs.contents, row, selection.pane)?;
        let characters: Vec<char> = line.text.chars().collect();
        let text: String = characters.get(start.column..end.column)?.iter().collect();
        let is_word_at = |column: Option<usize>| {
            column.and_then(|column| characters.get(column)).is_some_and(|c| find::is_word(*c))
        };
        let is_word = text.chars().all(find::is_word)
            && !is_word_at(start.column.checked_sub(1))
            && !is_word_at(Some(end.column));
        Some(Selected { text, is_word })
    }

    /// The selected lines for the assistant: their numbers in the file the pane shows (the base's
    /// when only deleted lines are selected) and their text.
    pub(crate) fn quote(&self, rows: &[Row], inputs: &DiffInputs<'_>) -> Option<ChatQuote> {
        let selection = self.selection.filter(|selection| !selection.is_empty())?;
        let pane = selection.pane;
        let (first, last) = selection.line_rows(rows, |row| has_line(row, pane))?;
        let lines: Vec<CodeLine<'_>> = rows
            .get(first..=last)?
            .iter()
            .filter_map(|row| line_source::code_line(inputs.diff, inputs.contents, row, pane))
            .collect();
        let is_old_side = pane == Pane::Left || lines.iter().all(|line| line.new_line == 0);
        let mut numbers = lines
            .iter()
            .map(|line| if is_old_side { line.old_line } else { line.new_line })
            .filter(|number| *number > 0);
        let start = numbers.next()?;
        let end = numbers.next_back().unwrap_or(start);
        let text = self.text(rows, inputs)?;
        Some(ChatQuote { path: inputs.path.to_owned(), start, end, is_old_side, text })
    }

    /// Ctrl+C, Ctrl+A, Alt+C, Alt+A (with a selection), Shift+↑/↓ and Esc while no text field has
    /// focus.
    pub(crate) fn keys(
        &mut self,
        ui: &Ui,
        code: &Code<'_>,
        inputs: &DiffInputs<'_>,
        view: &KeysView,
    ) -> KeysWant {
        let mut wants = KeysWant::default();
        if ui.ctx().text_edit_focused() {
            return wants;
        }
        let has_selection = self.is_active();
        let (copy, select_all, comment, escape, step) = ui.input_mut(|input| {
            let copy = text_selection::copy_pressed(input);
            let select_all = text_selection::select_all_pressed(input);
            let comment = input.consume_key(Modifiers::ALT, Key::C);
            let up = has_selection && input.consume_key(Modifiers::SHIFT, Key::ArrowUp);
            let down = has_selection && input.consume_key(Modifiers::SHIFT, Key::ArrowDown);
            let step = match (up, down) {
                (true, false) => Some(Direction::Previous),
                (false, true) => Some(Direction::Next),
                _ => None,
            };
            (copy, select_all, comment, input.key_pressed(Key::Escape), step)
        });
        if let Some(step) = step.filter(|_| self.drag.is_none()) {
            wants.scroll_to = self.extend_by_line(step, code, view.view);
        }
        if copy && let Some(text) = self.text(code.rows, inputs) {
            ui.ctx().copy_text(text);
        }
        if select_all {
            self.select_all(ui, code.rows, inputs, view.content, view.left_width);
        }
        wants.comment = comment;
        // Without a selection, or with the assistant off, Alt+A is the page's.
        if has_selection
            && assistant::is_enabled(ui.ctx())
            && ui.input_mut(|input| input.consume_key(Modifiers::ALT, Key::A))
            && let Some(quote) = self.quote(code.rows, inputs)
        {
            assistant::request(ui.ctx(), Some(quote));
        }
        // Esc that let go of a selection is used up: focus mode stays for the next one.
        if escape
            && self.drag.is_none()
            && view.can_escape
            && self.selection.take().is_some_and(|selection| !selection.is_empty())
        {
            ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        }
        wants
    }

    /// Shift+↑/↓: moves the selection's free end to the line above or below in its pane, keeping
    /// the column; returns the scroll offset that brings that line into `view`.
    fn extend_by_line(&mut self, step: Direction, code: &Code<'_>, view: Rect) -> Option<f32> {
        let mut selection = self.selection?;
        let focus = selection.focus();
        let pane = selection.pane;
        let rows = code.rows;
        let row = match step {
            Direction::Previous => (0..focus.row).rev().find(|row| has_line(&rows[*row], pane)),
            Direction::Next => (focus.row + 1..rows.len()).find(|row| has_line(&rows[*row], pane)),
        }?;
        selection.drag_to(Caret::new(row, focus.column));
        self.selection = Some(selection);
        let (top, bottom) = (code.offsets.get(row)?, code.offsets.get(row + 1)?);
        if top < view.top() {
            Some(top)
        } else if bottom > view.bottom() && view.height() > 0.0 {
            Some(bottom - view.height())
        } else {
            None
        }
    }

    /// All lines of the pane the selection is in, or the one under the pointer (the head pane otherwise).
    fn select_all(
        &mut self,
        ui: &Ui,
        rows: &[Row],
        inputs: &DiffInputs<'_>,
        content: Rect,
        left_width: f32,
    ) {
        let single_pane = inputs.layout.is_single_pane();
        let under_pointer =
            ui.ctx().pointer_hover_pos().filter(|pointer| content.contains(*pointer)).and_then(
                |pointer| {
                    pane_rects(content, single_pane, left_width)
                        .into_iter()
                        .find(|(_, _, rect)| rect.contains(pointer))
                        .map(|(pane, _, _)| pane)
                },
            );
        let fallback = if single_pane { Pane::Inline } else { Pane::Right };
        let pane =
            self.selection.map(|selection| selection.pane).or(under_pointer).unwrap_or(fallback);
        let first = rows.iter().position(|row| has_line(row, pane));
        let last = rows.iter().rposition(|row| has_line(row, pane));
        if let (Some(first), Some(last)) = (first, last) {
            self.selection = Some(Selection::lines(pane, first, last));
        }
    }

    /// Presses, drags and multi-clicks on the code; runs inside the scrolled rows before they
    /// paint. True when a drag of a line's "+" ended: time to comment on the lines it picked.
    pub(crate) fn track_pointer(
        &mut self,
        ui: &Ui,
        background: &Response,
        frame: &Frame<'_>,
        code: &Code<'_>,
        inputs: &DiffInputs<'_>,
    ) -> bool {
        let (pressed, down, shift, position) = ui.input(|input| {
            (
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
                input.modifiers.shift,
                input.pointer.interact_pos(),
            )
        });
        let at = Pointing { ui, frame, code, inputs };
        if background.hovered()
            && let Some(hover) = ui.ctx().pointer_hover_pos()
            && let Some(hit) = at.hit(hover, None, false)
        {
            ui.ctx().set_cursor_icon(if hit.in_gutter {
                CursorIcon::PointingHand
            } else {
                CursorIcon::Text
            });
        }
        // A widget over the code (a "+", a wedge, a card's buttons, a splitter at the edge) takes its
        // own press; egui still reports the background as hovered next to a drag-only widget.
        let owns_press = || {
            ui.ctx().viewport(|viewport| {
                let is_background = |widget: &egui::WidgetRect| widget.id == background.id;
                viewport.hits.click.as_ref().is_none_or(is_background)
                    && viewport.hits.drag.as_ref().is_none_or(is_background)
            })
        };
        match position {
            Some(position) if pressed && background.hovered() && owns_press() => {
                self.press(&at, position, shift);
            }
            Some(position) if down && self.drag.is_some() => {
                self.drag_to(&at, position);
            }
            _ => {}
        }
        let (double, triple) = (background.double_clicked(), background.triple_clicked());
        if let Some(position) = position.filter(|_| double || triple)
            && let Some(hit) = at.hit(position, None, true).filter(|hit| !hit.in_gutter)
        {
            self.selection = if triple {
                Some(Selection::line(hit.pane, hit.row))
            } else {
                line_source::code_line(inputs.diff, inputs.contents, &code.rows[hit.row], hit.pane)
                    .map(|line| Selection::word(hit.pane, hit.row, line.text, hit.column))
            };
        }
        if !down
            && let Some(drag) = self.drag.take()
            && drag == Drag::Plus
        {
            self.update_target(code, inputs);
            return true;
        }
        false
    }

    fn press(&mut self, at: &Pointing<'_>, position: Pos2, shift: bool) {
        let Some(hit) = at.hit(position, None, true) else {
            self.selection = None;
            return;
        };
        let selection = Selection::press(
            self.selection,
            hit.pane,
            Caret::new(hit.row, hit.column),
            hit.in_gutter,
            shift,
        );
        self.drag = Some(if selection.unit == Unit::Lines { Drag::Lines } else { Drag::Text });
        self.selection = Some(selection);
    }

    fn drag_to(&mut self, at: &Pointing<'_>, position: Pos2) {
        let Some(mut selection) = self.selection else {
            return;
        };
        let by_text = selection.unit == Unit::Text;
        if let Some(hit) = at.hit(position, Some(selection.pane), by_text) {
            selection.drag_to(Caret::new(hit.row, hit.column));
            self.selection = Some(selection);
        }
    }

    /// Dragging past the top or bottom of `content` scrolls on, so a selection can grow beyond
    /// `view`, the part of rows `total_height` tall on screen: the scroll offset to go to.
    /// `is_embedded`: another view scrolls the rows and clamps the offset itself.
    pub(crate) fn edge_scroll(
        &self,
        ui: &Ui,
        content: Rect,
        view: Rect,
        total_height: f32,
        is_embedded: bool,
    ) -> Option<f32> {
        self.drag?;
        let position = ui.input(|input| input.pointer.interact_pos())?;
        let step = text_selection::edge_scroll_step(content, position.y)?;
        let target = view.top() + step;
        let limit = (total_height - view.height()).max(0.0);
        ui.ctx().request_repaint();
        Some(if is_embedded { target } else { target.clamp(0.0, limit) })
    }
}

/// The comment target of `selection` on `rows`, for the selection and row build `key`.
fn target_for(
    rows: &[Row],
    selection: Selection,
    inputs: &DiffInputs<'_>,
    key: (Selection, u64),
) -> Option<RangeTarget> {
    let pane = selection.pane;
    let (first, last) = selection.line_rows(rows, |row| has_line(row, pane))?;
    let last_len = line_source::code_line(inputs.diff, inputs.contents, &rows[last], pane)
        .map_or(0, |line| line.text.chars().count());
    let prefer_right = pane != Pane::Left;
    let end = diff_rows::anchor_for(
        inputs.diff,
        &rows[last],
        prefer_right,
        inputs.path,
        inputs.old_path,
    )?;
    // Side by side: the pane's file. One column: the head, unless the last line is a deleted one.
    let is_old_side = match pane {
        Pane::Left => true,
        Pane::Right => false,
        Pane::Inline => end.new_line.is_none(),
    };
    let side = if is_old_side { DiffSide::Old } else { DiffSide::New };
    let pick = RangePick {
        start_row: first,
        end_row: last,
        prefer_right,
        side,
        offsets: selection.offsets(first, last, last_len),
    };
    let anchor = diff_rows::range_anchor(inputs.diff, rows, &pick, inputs.path, inputs.old_path)?;
    let focus_is_start = selection.is_backward() && first != last;
    Some(RangeTarget {
        pane,
        focus_row: if focus_is_start { first } else { last },
        focus_is_start,
        label: cv_pr_threads::suggestion::lines_label(&anchor),
        anchor,
        blocked: why_not_commentable(rows, inputs, first, last),
        key,
    })
}

/// GitHub takes comments on diff lines only, and a range only within one hunk.
fn why_not_commentable(
    rows: &[Row],
    inputs: &DiffInputs<'_>,
    first: usize,
    last: usize,
) -> Option<&'static str> {
    if inputs.can_comment_outside_hunks {
        return None;
    }
    if [first, last].iter().any(|row| rows[*row].kind == RowKind::Expanded) {
        return Some(
            "GitHub takes comments only on lines of the diff. Start and end the selection on lines next to the changes.",
        );
    }
    if first != last && diff_rows::spans_hunks(inputs.diff, rows, first, last) {
        return Some(
            "GitHub takes a comment on several lines only within one block of changes. Start and end the selection between the same two collapsed sections.",
        );
    }
    None
}

/// What the pointer is pointed at this frame.
struct Pointing<'a> {
    ui: &'a Ui,
    frame: &'a Frame<'a>,
    code: &'a Code<'a>,
    inputs: &'a DiffInputs<'a>,
}

impl Pointing<'_> {
    /// The code row, pane and character under `position`. A press needs a code row under it; a
    /// drag (`pane` set) over a comment or a collapsed bar takes the nearest code line instead.
    fn hit(&self, position: Pos2, pane: Option<Pane>, with_column: bool) -> Option<Hit> {
        let (frame, rows, offsets) = (self.frame, self.code.rows, self.code.offsets);
        let total = offsets.height();
        if rows.is_empty() || total <= 0.0 || !offsets.is_placed(rows.len()) {
            return None;
        }
        let y = (position.y - frame.origin.y).clamp(0.0, total - 0.01);
        let under = offsets.row_at(y).min(rows.len() - 1);
        let row = if is_code(&rows[under]) {
            under
        } else if pane.is_some() {
            (0..under)
                .rev()
                .find(|row| is_code(&rows[*row]))
                .or_else(|| (under..rows.len()).find(|row| is_code(&rows[*row])))?
        } else {
            return None;
        };
        let top = offsets[row];
        let row_rect = Rect::from_min_size(
            frame.origin + vec2(0.0, top),
            vec2(frame.width, offsets[row + 1] - top),
        );
        let layout = self.inputs.layout;
        let rects = pane_rects(row_rect, layout.is_single_pane(), frame.left_width);
        let panes = rects.as_slice();
        let found = match pane {
            Some(wanted) => panes.iter().find(|(candidate, _, _)| *candidate == wanted),
            None => panes.iter().find(|(_, _, rect)| position.x < rect.right()).or(panes.last()),
        };
        let (which, slot, pane_rect) = *found?;
        let text_left = pane_rect.left() + frame.metrics.text_inset(layout.has_two_numbers());
        let line = with_column
            .then(|| {
                line_source::code_line(self.inputs.diff, self.inputs.contents, &rows[row], which)
            })
            .flatten();
        let column = line.map_or(0, |line| {
            let h_offset = if frame.wrap_width.is_some() { 0.0 } else { self.code.h_offset[slot] };
            column_at(
                self.ui,
                line.text,
                frame,
                slot,
                position - pos2(text_left - h_offset, row_rect.top()),
            )
        });
        Some(Hit { row, pane: which, column, in_gutter: position.x < text_left })
    }
}

fn is_code(row: &Row) -> bool {
    matches!(row.kind, RowKind::Line | RowKind::Expanded)
}

/// The pane shows a line on this row (side by side leaves one side of a pure change empty).
fn has_line(row: &Row, pane: Pane) -> bool {
    match row.kind {
        RowKind::Line => match pane {
            Pane::Left => row.left.is_some(),
            Pane::Right => row.right.is_some(),
            Pane::Inline => true,
        },
        RowKind::Expanded => true,
        RowKind::Gap(_)
        | RowKind::Separator
        | RowKind::Removal
        | RowKind::Thread(_)
        | RowKind::Composer => false,
    }
}

/// Character of `text` nearest to `local` (relative to where the line's text starts), laid out
/// as the row paints it in pane `slot`.
fn column_at(ui: &Ui, text: &str, frame: &Frame<'_>, slot: usize, local: Vec2) -> usize {
    let palette = theme::palette(ui.ctx());
    let style = JobStyle {
        font: &frame.metrics.font,
        palette: &palette,
        word_background: Color32::TRANSPARENT,
        line_height: frame.metrics.line_height,
        wrap_width: frame.wrap_width.map(|widths| widths[slot]),
        syntax_mix: 1.0,
    };
    let galley = ui.painter().layout_job(code_job::layout_job(text, &[], &[], &style));
    code_job::char_index(text, galley.cursor_from_pos(local).index.0)
}
