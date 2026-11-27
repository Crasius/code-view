//! Code rows of the diff: each pane's line painted with its changed words, its selection and the
//! band of a multi-line comment, and in the marker column a thread's marker (or, while comments
//! are hidden, their count), the selection's "Comment on lines X–Y" or the hovered line's comment
//! button. The find bar's hits are marked in the text. A line the view was sent to flashes
//! ([`flash`]); the cited lines painted tell the assistant's mini chat where they are. Painting
//! reads the view's state and returns what the user did, for the view to carry out.

pub(super) mod flash;
mod gutter;
pub(super) mod menu;

use egui::{Pos2, Rect, Sense, Ui};
use review_core::{ReviewThread, ThreadAnchor};

use self::flash::Flash;
use super::card_rows::Revealed;
use super::rows::{self, DiffRows};
use super::selecting::{RangeTarget, Selecting};
use super::{DiffInputs, Frame, pane_rects};
use crate::find::text::TextFind;
use cv_base::theme;
use cv_code::diff_paint::{self, LineStyle, PaintContext, PaneLine};
use cv_code::diff_rows::{self, Layout, Row, RowKind};
use cv_code::find_paint::{self as find_paint, FindMark, MarkLook};
use cv_code::line_source::{self, CodeLine, Pane};
use cv_pr_session::assistant_link;
use cv_pr_threads::{ThreadsUi, markers};

/// What painting a code row reads of the view.
pub(super) struct CodeRows<'v> {
    pub built: &'v mut DiffRows,
    pub selecting: &'v Selecting,
    /// Threads shown although comments are hidden in the diff.
    pub revealed: &'v Revealed,
    /// How far each pane is scrolled sideways.
    pub h_offset: [f32; 2],
    /// A new comment is being written: no other comment button shows.
    pub has_composer: bool,
    /// The boundary between the sides is being dragged: rows show no hover or comment button.
    pub is_resizing: bool,
    pub flash: &'v mut Flash,
    /// The find bar's hits.
    pub find: &'v mut TextFind,
}

/// What a code row took and what the user did on it.
pub(super) struct RowOutcome {
    /// Visual rows its lines took (more than one when wrapped).
    pub rows: usize,
    /// A thread count was clicked: show the threads of that pane there, or hide them again.
    pub toggled_threads: Option<(Pane, bool)>,
    pub action: Option<RowAction>,
    /// A secondary click on this pane: its context menu.
    pub menu: Option<Pane>,
}

/// A comment button pressed on a code row.
pub(super) enum RowAction {
    /// "Comment on lines X–Y" was clicked.
    CommentOnSelection,
    /// The pointer went down on a line's comment button: the line is picked, and dragging on picks
    /// more.
    PickLine(Pane),
    /// A line's comment button was clicked from the keyboard (the pointer was never down on it).
    CommentOnLine(ThreadAnchor),
}

/// One pane of a code row as it is shown this frame.
struct Spot {
    row_index: usize,
    row: Row,
    pane: Pane,
    /// 0 for the left (or only) pane, 1 for the right one.
    slot: usize,
    rect: Rect,
    /// The whole row.
    row_rect: Rect,
    /// Where the pointer is, when over this row.
    pointer: Option<Pos2>,
}

/// What a pane's marker column shows, and whether the pane's line is hovered.
struct Gutter<'a> {
    is_hovered: bool,
    control: Control<'a>,
}

#[derive(Clone, Copy)]
enum Control<'a> {
    None,
    /// A thread's marker (or, while comments are hidden, the count of the threads on the line).
    Marker(&'a ReviewThread),
    /// "Comment on lines X–Y" on the selection.
    Range(&'a RangeTarget),
    /// The hovered line's comment button.
    Comment,
}

/// What showing a pane gave.
struct PaneShown {
    rows: usize,
    toggles_threads: Option<bool>,
    action: Option<RowAction>,
}

impl CodeRows<'_> {
    /// Code row `row_index` at `rect`: each pane's line and gutter, and the divider between the panes.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        row_index: usize,
        inputs: &DiffInputs<'_>,
        frame: &Frame<'_>,
        threads_ui: &mut ThreadsUi,
    ) -> RowOutcome {
        let single_pane = inputs.layout.is_single_pane();
        // Rows share their edges: a pointer right on one belongs to the row below, so only one row shows its comment button.
        let pointer = ui.ctx().pointer_hover_pos().filter(|pointer| {
            !self.is_resizing
                && pointer.y < rect.bottom()
                && rect.contains(*pointer)
                && ui.rect_contains_pointer(rect)
        });
        let row = self.built.rows[row_index];
        let mut outcome = RowOutcome { rows: 1, toggled_threads: None, action: None, menu: None };
        for (pane, slot, pane_rect) in pane_rects(rect, single_pane, frame.left_width) {
            let spot =
                Spot { row_index, row, pane, slot, rect: pane_rect, row_rect: rect, pointer };
            let shown = self.show_pane(ui, &spot, inputs, frame, threads_ui);
            outcome.rows = outcome.rows.max(shown.rows);
            if let Some(show) = shown.toggles_threads {
                outcome.toggled_threads = Some((pane, show));
            }
            if shown.action.is_some() {
                outcome.action = shown.action;
            }
        }
        if !single_pane {
            let palette = theme::palette(ui.ctx());
            diff_paint::paint_divider(ui.painter(), rect.left() + frame.left_width, rect, &palette);
        }
        if let Some(pointer) = pointer
            && ui.input(|input| input.pointer.secondary_pressed())
        {
            outcome.menu = Some(match (single_pane, pointer.x > rect.left() + frame.left_width) {
                (true, _) => Pane::Inline,
                (false, true) => Pane::Right,
                (false, false) => Pane::Left,
            });
        }
        outcome
    }

    /// One pane of a code row: its line painted, and in the marker column a thread's marker (or
    /// count), the selection's comment button or the hovered line's.
    fn show_pane(
        &mut self,
        ui: &mut Ui,
        spot: &Spot,
        inputs: &DiffInputs<'_>,
        frame: &Frame<'_>,
        threads_ui: &mut ThreadsUi,
    ) -> PaneShown {
        let style = line_style(inputs.layout);
        let line = pane_code_line(&spot.row, spot.pane, inputs);
        let flash = self.flash.strength(ui.ctx(), spot.pane, line.as_ref());
        if self.flash.is_cited(spot.pane, line.as_ref()) {
            let shown = spot.rect.intersect(ui.clip_rect());
            if shown.is_positive() {
                assistant_link::note_cited(ui.ctx(), shown);
            }
        }
        // Only the diff style marks changed words; computed before the borrows below, then read.
        let word_kind = line.as_ref().map(|line| line.kind).filter(|_| style == LineStyle::Diff);
        if let Some(kind) = word_kind {
            self.built.compute_words(inputs.diff, &spot.row, spot.pane, kind);
        }
        let words = word_kind.map_or(&[][..], |kind| self.built.words(&spot.row, spot.pane, kind));
        let selection =
            line.as_ref().zip(self.selecting.selection.as_ref()).and_then(|(line, selection)| {
                selection.on_row(spot.row_index, spot.pane, line.text.chars().count())
            });
        let gutter = self.gutter_for(spot, line.is_some(), inputs, frame);
        let palette = theme::palette(ui.ctx());
        let painter = ui.painter().with_clip_rect(spot.rect.intersect(ui.clip_rect()));
        let context = PaintContext { painter: &painter, palette: &palette, metrics: frame.metrics };
        let revealed = self.revealed;
        let pane_line = PaneLine {
            line,
            has_two_numbers: inputs.layout.has_two_numbers(),
            style,
            words,
            h_offset: if frame.wrap_width.is_some() { 0.0 } else { self.h_offset[spot.slot] },
            wrap_width: frame.wrap_width.map(|widths| widths[spot.slot]),
            is_hovered: gutter.is_hovered,
            hide_marker: !matches!(gutter.control, Control::None),
            selection,
            range: self.built.range_mark(spot.row_index, spot.pane, |thread| {
                revealed.hides(inputs.show_threads, inputs.threads[thread as usize])
            }),
            invisibles: self.built.invisibles(inputs.diff, &spot.row, spot.pane, style),
            syntax_mix: frame.syntax_mix,
        };
        let finds = self.find.marks(spot.pane, pane_line.line.as_ref());
        let reveal =
            frame.wrap_width.is_none().then(|| reveal_find(spot, &pane_line, &finds, frame));
        let painted =
            diff_paint::paint_flashed_line(&context, spot.rect, &pane_line, flash, &finds);
        if let Some(mark) = painted.no_newline {
            let id = ui.scope_id().with(("no_newline", spot.row_index, spot.pane as u8));
            let response = ui.interact(mark, id, Sense::hover());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Image, true, diff_paint::NO_NEWLINE_HINT)
            });
            response.on_hover_text(diff_paint::NO_NEWLINE_HINT);
        }
        let (toggles_threads, action) =
            self.show_control(ui, spot, gutter.control, inputs, frame, threads_ui);
        if let Some(reveal) = reveal.flatten() {
            reveal(self.find);
        }
        PaneShown { rows: painted.rows, toggles_threads, action }
    }

    /// What the marker column of a pane shows: a thread's marker takes the column its line's
    /// comment button would use, and while text or lines are selected, the selection's comment
    /// button is the only one (a hover button on another line would offer a second, different
    /// comment). `has_line`: the pane shows a line on this row.
    fn gutter_for<'a>(
        &'a self,
        spot: &Spot,
        has_line: bool,
        inputs: &DiffInputs<'a>,
        frame: &Frame<'_>,
    ) -> Gutter<'a> {
        let is_hovered =
            has_line && spot.pointer.is_some_and(|pointer| spot.rect.contains(pointer));
        // The "Comment on lines X–Y" button stays where the selection was last moved to.
        if let Some(target) = self.selecting.target.as_ref().filter(|target| {
            target.pane == spot.pane && target.focus_row == spot.row_index && !self.has_composer
        }) {
            return Gutter { is_hovered, control: Control::Range(target) };
        }
        let marker = self
            .built
            .pane_threads(spot.row_index, spot.pane, inputs.threads)
            .next()
            .filter(|_| has_line);
        if let Some(thread) = marker {
            return Gutter { is_hovered, control: Control::Marker(thread) };
        }
        let can_comment_here = inputs.can_comment
            && (spot.row.kind == RowKind::Line || inputs.can_comment_outside_hunks)
            && !self.has_composer;
        // A removal wedge on the row's edge takes the pointer there; the comment button would sit under it.
        let is_on_wedge = spot.pointer.is_some_and(|pointer| {
            self.built.is_on_wedge(pointer, spot.row_index, spot.row_rect, frame.metrics)
        });
        let shows_button = is_hovered
            && can_comment_here
            && !is_on_wedge
            && self.selecting.drag.is_none()
            && !self.selecting.is_active();
        let control = if shows_button { Control::Comment } else { Control::None };
        Gutter { is_hovered, control }
    }

    /// Shows `control` in the marker column of `spot`: a thread count clicked (whether to show
    /// the threads there or hide them again), and a comment button pressed.
    fn show_control(
        &self,
        ui: &mut Ui,
        spot: &Spot,
        control: Control<'_>,
        inputs: &DiffInputs<'_>,
        frame: &Frame<'_>,
        threads_ui: &mut ThreadsUi,
    ) -> (Option<bool>, Option<RowAction>) {
        let two_numbers = inputs.layout.has_two_numbers();
        match control {
            Control::None => (None, None),
            Control::Marker(thread) => {
                let slot = frame.metrics.marker_rect(spot.rect, two_numbers, spot.rect.height());
                if !inputs.show_threads {
                    return (self.show_thread_count(ui, slot, spot, inputs, threads_ui), None);
                }
                let id = ui.scope_id().with(("thread_marker", &thread.id));
                markers::show(ui, slot, id, thread, threads_ui);
                (None, None)
            }
            Control::Range(target) => {
                let clicked =
                    gutter::range_button(ui, spot.rect, two_numbers, frame.metrics, target);
                (None, clicked.then_some(RowAction::CommentOnSelection))
            }
            Control::Comment => {
                let pressed = gutter::comment_button(
                    ui,
                    spot.rect,
                    spot.row_index,
                    spot.pane,
                    two_numbers,
                    frame.metrics,
                );
                let action = if pressed.pointer_down {
                    Some(RowAction::PickLine(spot.pane))
                } else if pressed.clicked {
                    diff_rows::anchor_for(
                        inputs.diff,
                        &spot.row,
                        spot.pane != Pane::Left,
                        inputs.path,
                        inputs.old_path,
                    )
                    .map(RowAction::CommentOnLine)
                } else {
                    None
                };
                (None, action)
            }
        }
    }

    /// The gutter's marker of the threads on a line while comments are hidden: their count. A
    /// click returns whether to show them there (or hide them again).
    fn show_thread_count(
        &self,
        ui: &mut Ui,
        slot: Rect,
        spot: &Spot,
        inputs: &DiffInputs<'_>,
        threads_ui: &ThreadsUi,
    ) -> Option<bool> {
        let mut hidden = markers::Hidden::default();
        for thread in self.built.pane_threads(spot.row_index, spot.pane, inputs.threads) {
            hidden.count += 1;
            hidden.is_shown |= self.revealed.contains(thread);
            hidden.has_news |= threads_ui.is_unread(thread);
        }
        markers::show_count(
            ui,
            slot,
            ui.scope_id().with(("thread_count", spot.row_index, spot.pane as u8)),
            &hidden,
        )
        .then_some(!hidden.is_shown)
    }
}

/// How changed lines look in `layout`.
fn line_style(layout: Layout) -> LineStyle {
    match layout {
        Layout::NewOnly => LineStyle::Focus,
        Layout::WholeFile(_) => LineStyle::Plain,
        Layout::SideBySide | Layout::Inline | Layout::Compact(_) | Layout::CompactSplit(_) => {
            LineStyle::Diff
        }
    }
}

/// The line `pane` shows on `row`. Until the file's own colors reach a line, its colors by diff
/// line stand in, each pane's from the line it shows. Both agree almost always, so nothing changes
/// visibly when the file's arrive.
fn pane_code_line<'a>(row: &Row, pane: Pane, inputs: &DiffInputs<'a>) -> Option<CodeLine<'a>> {
    let mut line = line_source::code_line(inputs.diff, inputs.contents, row, pane)?;
    if let Some(spans) = inputs.line_spans
        && !line.has_file_spans
        && let Some(index) = rows::line_index(row, pane)
        && let Some(line_spans) = spans.line(index as usize)
    {
        line.spans = line_spans;
    }
    Some(line)
}

/// Where the current hit on `pane_line` is, for its pane to scroll sideways to it once it became
/// current (see [`TextFind::reveal`]).
fn reveal_find(
    spot: &Spot,
    pane_line: &PaneLine<'_>,
    finds: &[FindMark],
    frame: &Frame<'_>,
) -> Option<impl FnOnce(&mut TextFind) + use<>> {
    let current = finds.iter().find(|mark| mark.look == MarkLook::Current);
    let (Some(mark), Some(line)) = (current, pane_line.line.as_ref()) else {
        return None;
    };
    let width = spot.rect.width() - frame.metrics.text_inset(pane_line.has_two_numbers);
    let columns = find_paint::columns(line.text, &mark.bytes);
    let (slot, advance, offset) = (spot.slot, frame.metrics.char_width, pane_line.h_offset);
    Some(move |find: &mut TextFind| find.reveal(slot, columns, advance, width, offset))
}
