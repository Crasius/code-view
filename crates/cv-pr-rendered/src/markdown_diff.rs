//! The rendered diff of a Markdown file. Both versions are rendered and lined up block by block
//! in one scroll area, so the old and the new version of a block always sit side by side,
//! whatever scrolls them: the wheel over either side, the keys, the overview strip. A block one
//! side lacks leaves a hatched gap of the other block's height there. Added blocks are tinted
//! green with a bar and a "+", removed ones red with a "−", and an edited block shows next to its
//! old version with the changed words marked. Long runs of unchanged blocks fold away behind a bar
//! (the context choice counts blocks here), the strip on the right shows where the changes are,
//! and N / P (F7 / Shift+F7) step through them.
//!
//! Inline puts the old version of an edited block right above the new one; New only shows the new
//! version with its changes in the accent color and a red wedge where blocks were removed (hover
//! it to read them, click it to see them side by side).
//!
//! Comment threads sit under the block of their line, as in the text diff (see `threads`); a
//! fold counts the ones it hides. With "Show comments in the diff" off, a count in a block's gutter
//! stands for its threads and shows them there on a click.
//!
//! Only the rows in view are laid out. Rows near the view are measured before anything is placed,
//! the rest a few per frame in its spare time (nearest first), and until then count with an
//! estimate. A measured height that differs from its estimate moves the scroll offset with it, so
//! what is on screen stays put.

mod align;
mod bars;
mod blocks;
pub mod embedded;
mod find;
mod heights;
mod inputs;
mod keys;
mod lanes;
mod model;
mod rows;
#[cfg(test)]
mod tests;
mod threads;

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cv_base::theme;
use cv_code::diff_paint;
use cv_markdown::RenderOptions;
use cv_media::image_state::image_generation;
use cv_pr_diff::diff_overview::{self, Overview, past_end::Ends};
use cv_pr_diff::diff_view::cards::{self, CardHeights, DEFAULT_CARD_HEIGHT};
use cv_pr_diff::diff_view::{ChangePosition, Direction, OVERVIEW_WIDTH, navigation};
use cv_pr_threads::{ThreadsUi, markers};
use cv_widgets::splitter::{Splitter, share_range};
use egui::{Id, Rangef, Rect, Ui, UiBuilder, vec2};
use review_core::{DiffSide, ReviewThread};
use review_store::{PaneSizes, RenderedWidth};

use self::bars::{
    paint_column_divider, paint_header, show_banner, show_empty, show_fold, show_removal,
};
#[cfg(any(test, feature = "test-support"))]
pub use self::blocks::last_layers;
use self::blocks::show_cell;
use self::find::{BarFrame, BlockFind, FindRows, panes};
use self::heights::{RowHeights, outward};
pub use self::inputs::{RenderedEvent, RenderedImages, RenderedInputs};
use self::lanes::{Areas, Lanes, reading_column, reading_room};
pub use self::model::MarkdownModel;
use self::model::{Cell, Mode, Row, RowsKey};
use self::rows::{begin_frame, build};
use self::threads::{CardAction, ThreadRow, is_hidden};

/// Left of each block: the change bar and its "+" or "−" (under the pointer, the comment button).
const GUTTER: f32 = 28.0;
const STRIPE_WIDTH: f32 = 3.0;
const RIGHT_PADDING: f32 = 16.0;
/// Room above and below a block inside its tint (at zoom 1).
const ROW_PAD: f32 = 4.0;
/// Rows out of view are measured in the frame's spare time, at most this long per frame.
const MEASURE_BUDGET: Duration = Duration::from_millis(3);
const ARROW_STEP: f32 = 48.0;

/// Where the view should go once the rows are laid out.
#[derive(Debug, Clone, PartialEq)]
enum Place {
    Top,
    /// A source line of one version (from the text diff): its block, opened if folded.
    Line {
        side: DiffSide,
        line: u32,
    },
    /// A block, this far into it, as before the rows were rebuilt.
    Unit {
        side: DiffSide,
        unit: usize,
        below_top: f32,
    },
    /// A thread's card, its fold opened and the card shown while comments are hidden.
    Thread(String),
    /// The find bar's current hit: its block in the middle of the view, opened if folded.
    Find {
        side: DiffSide,
        unit: usize,
    },
}

#[derive(Clone, Copy)]
struct Viewport(Rect);

impl Default for Viewport {
    fn default() -> Self {
        Self(Rect::ZERO)
    }
}

/// What painting the rows of a frame needs.
#[derive(Clone, Copy)]
struct Frame<'a> {
    origin: egui::Pos2,
    width: f32,
    lanes: Lanes,
    options: RenderOptions,
    id: Id,
    can_comment: bool,
    mode: Mode,
    model: &'a MarkdownModel,
    is_resizing: bool,
    /// The threads the rows were built for.
    threads: &'a [&'a ReviewThread],
    show_threads: bool,
    find: &'a BlockFind,
}

#[derive(Default)]
pub struct MarkdownDiff {
    model: Option<Arc<MarkdownModel>>,
    mode: Mode,
    rows_key: Option<RowsKey>,
    rows: Vec<Row>,
    /// The ids of the threads the rows were built for, by the index thread rows hold.
    thread_ids: Vec<String>,
    /// Threads in folded blocks: (the fold's first pair, the thread's index).
    folded_threads: Vec<(usize, usize)>,
    cards: CardHeights,
    /// "Show comments in the diff", this frame.
    show_threads: bool,
    /// Threads shown although comments are hidden: the user clicked their count or jumped to them.
    revealed: HashSet<String>,
    /// Space above each row, at zoom 1.
    spaces: Vec<f32>,
    /// Rows of each change: consecutive changed rows.
    hunks: Vec<Range<usize>>,
    overview: Overview,
    /// The row showing each unit of the old and of the new text.
    old_rows: Vec<Option<usize>>,
    new_rows: Vec<Option<usize>>,
    /// Each row's height and top, measured or estimated.
    heights: RowHeights,
    /// Folds the user opened, by their first pair.
    expanded: HashSet<usize>,
    expanded_version: u64,
    /// The visible part of the rows last frame, in row coordinates.
    viewport: Viewport,
    scroll_to: Option<f32>,
    pending: Option<Place>,
    /// The change the user last stepped to, while it is on screen.
    current: Option<usize>,
    is_resizing: bool,
    /// Every lane filled its width last frame (Full width, or a column as wide as the pane).
    fills: bool,
    /// The widest reading column the one column held last frame, at zoom 1.
    reading_room: f32,
    find: BlockFind,
    /// Its place in a holding scroll area, when embedded ([`embedded`]).
    embedded: embedded::EmbeddedPlace,
    #[cfg(test)]
    probe: tests::Probe,
}

impl MarkdownDiff {
    /// Where the change in focus is among the changes, for the header; `None` until the texts
    /// with `key` are laid out.
    pub(crate) fn change_position(&self, key: u64) -> Option<ChangePosition> {
        self.model.as_ref().filter(|model| model.key == key && self.rows_key.is_some())?;
        Some(ChangePosition { current: self.current, total: self.hunks.len() })
    }

    pub(crate) fn go_to_change(&mut self, direction: Direction) {
        let (tops, total) = (self.heights.tops(), self.heights.total());
        let top_of = |index: usize| tops.get(self.hunks[index].start).copied().unwrap_or(0.0);
        let view = self.viewport.0;
        let Some(index) =
            navigation::step(direction, self.current, self.hunks.len(), view.top(), top_of)
        else {
            return;
        };
        self.current = Some(index);
        self.scroll_to = Some(navigation::focus_offset(top_of(index), view.height(), total));
    }

    /// The source line at the top of the view and the version it is from, so the text diff can
    /// open at the same place.
    pub(crate) fn top_line(&self) -> Option<(DiffSide, u32)> {
        let model = self.model.as_ref()?;
        let (row, side, unit, below_top) = self.top_unit()?;
        let lines = &model.units(side).units.get(unit)?.lines;
        let span = lines.end.saturating_sub(lines.start).max(1);
        let fraction = (below_top / self.heights.height(row).max(1.0)).clamp(0.0, 1.0);
        let line = lines.start + ((span as f32 * fraction) as u32).min(span - 1);
        Some((side, line))
    }

    /// Opens at `line` of `side` (the text diff's place), or at the top.
    pub(crate) fn reveal_line(&mut self, place: Option<(DiffSide, u32)>) {
        self.pending = Some(match place {
            Some((side, line)) => Place::Line { side, line },
            None => Place::Top,
        });
    }

    /// Scrolls to the thread's card once the rows hold it: its fold opens, and while comments are
    /// hidden it shows.
    pub(crate) fn reveal_thread(&mut self, thread_id: &str) {
        self.pending = Some(Place::Thread(thread_id.to_owned()));
    }

    /// `width` is the reading column (or full width); dragging the column's edges changes it.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        inputs: &RenderedInputs<'_>,
        split: &mut f32,
        width: &mut RenderedWidth,
        threads_ui: &mut ThreadsUi,
    ) -> Option<RenderedEvent> {
        let palette = theme::palette(ui.ctx());
        let (model, mode, has_banner) = begin_frame(self, ui.ctx(), inputs);
        if model.old.is_empty() && model.new.is_empty() {
            return show_empty(ui, &palette);
        }
        let options = inputs.render_options();
        let id = ui.scope_id().with("markdown_diff");
        let full = ui.available_rect_before_wrap();
        let owns_keys = ui.rect_contains_pointer(full) || ui.memory(|m| m.focused().is_none());
        ui.painter().rect_filled(full, 0, palette.surface);
        cv_widgets::surfaces::mark_code(ui, full);
        let areas = Areas::new(full, mode, has_banner, *split, OVERVIEW_WIDTH);
        let content = areas.content;
        let bar = self.find.prepare(ui, panes(content, areas.left_width, mode), owns_keys, &model);
        let column = reading_column(*width, mode, inputs.scale);
        let lanes = Lanes::new(mode, content.width(), areas.left_width, column);
        self.fills = lanes.fills();
        self.reading_room = reading_room(lanes.first) / inputs.scale;

        self.layout(ui, &model, &lanes, options, content.height(), id);
        self.keyboard(ui, content.height(), inputs.past_end);
        self.go_to_find(ui, &bar, &model);
        // The rows read the hits while they paint, which needs the diff itself.
        let find = std::mem::take(&mut self.find);
        let frame = Frame {
            origin: content.min,
            width: content.width(),
            lanes,
            options,
            id,
            can_comment: inputs.can_comment,
            mode,
            model: &model,
            is_resizing: self.is_resizing,
            threads: inputs.threads,
            show_threads: inputs.show_threads,
            find: &find,
        };
        let scrolled = self.show_scrolled(ui, content, &frame, threads_ui, inputs.past_end);
        let chrome = self.show_chrome(ui, &areas, &frame, split, width, inputs.past_end);
        self.find = find;
        self.find.show_widget(ui);
        chrome.or(scrolled)
    }

    /// Places the find bar's hits among the rows and goes to the one it stepped to.
    fn go_to_find(&mut self, ui: &Ui, bar: &BarFrame, model: &MarkdownModel) {
        let rows = FindRows {
            key: self.rows_key,
            rows: &self.rows,
            old_rows: &self.old_rows,
            new_rows: &self.new_rows,
            heights: &self.heights,
            view: self.viewport.0,
        };
        if let Some((side, unit)) = self.find.go(bar, &rows, model, &mut self.overview) {
            self.pending = Some(Place::Find { side, unit });
            ui.ctx().request_repaint();
        }
    }

    /// The rows in their scroll area at `content`, with the room past the end when `past_end` is
    /// on: the ones in view paint (with `template`, put where the scroll area places the rows),
    /// and the view's place is kept for the next frame.
    fn show_scrolled(
        &mut self,
        ui: &mut Ui,
        content: Rect,
        template: &Frame<'_>,
        threads_ui: &mut ThreadsUi,
        past_end: bool,
    ) -> Option<RenderedEvent> {
        let total = Ends::of(self.heights.tops(), content.height(), past_end).height;
        let scroll = egui::ScrollArea::vertical()
            .id_salt("markdown_diff_scroll")
            .auto_shrink(false)
            .scroll_source(egui::scroll_area::ScrollSource::MOUSE_WHEEL)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        let placed = self.scroll_to.take();
        let output = ui
            .scope_builder(UiBuilder::new().max_rect(content), |ui| {
                cv_widgets::scroll::placed(ui, scroll, placed).show_viewport(ui, |ui, viewport| {
                    ui.set_min_height(total);
                    let frame = Frame { origin: ui.max_rect().min, ..*template };
                    self.paint_rows(ui, &frame, viewport, threads_ui)
                })
            })
            .inner;
        self.viewport.0 = output.inner_rect.translate(
            vec2(0.0, output.state.clamped_offset().y) - output.inner_rect.min.to_vec2(),
        );
        self.forget_change_out_of_view();
        output.inner
    }

    /// What frames the rows in `areas`: the header and the banner, the reading column's edges,
    /// side by side the divider over the header and the boundary between Before and After (at the
    /// old side's `split` share), and the overview strip (`past_end`: with room past the end).
    fn show_chrome(
        &mut self,
        ui: &mut Ui,
        areas: &Areas,
        frame: &Frame<'_>,
        split: &mut f32,
        width: &mut RenderedWidth,
        past_end: bool,
    ) -> Option<RenderedEvent> {
        let palette = theme::palette(ui.ctx());
        let (full, content) = (areas.full, areas.content);
        let mut event = None;
        if let Some(header) = areas.header {
            paint_header(ui, header, &frame.lanes, frame.model, &palette);
        }
        if let Some(banner) = areas.banner
            && let Some(banner_event) = show_banner(ui, banner, &palette)
        {
            event = Some(banner_event);
        }
        let scale = frame.options.scale;
        let edge = lanes::show_edges(ui, content, &frame.lanes, frame.mode, scale, width, frame.id);
        if edge.committed {
            event = Some(RenderedEvent::WidthResized);
        }
        if frame.mode == Mode::Columns {
            let x = content.left() + areas.left_width;
            // Over the header here; over the rows, where fold bars interrupt it, in `paint_rows`.
            diff_paint::paint_divider(
                ui.painter(),
                x,
                Rect::from_x_y_ranges(x..=x, Rangef::new(full.top(), content.top())),
                &palette,
            );
            let resize = Splitter::new("markdown_diff_split", "Resize Before and After")
                .default_value(PaneSizes::DEFAULT.preview_split)
                .range(share_range(content.width()))
                .per_point(1.0 / content.width().max(1.0))
                .draws_line(false)
                .show(ui, x, Rangef::new(full.top(), content.bottom()), split);
            self.is_resizing = resize.response.dragged() || edge.dragged;
            if resize.committed {
                event = Some(RenderedEvent::SplitResized);
            }
        } else {
            self.is_resizing = edge.dragged;
        }
        let id = frame.id.with("overview");
        let (overview, tops, view) = (&self.overview, self.heights.tops(), self.viewport.0);
        let pick = diff_overview::show(ui, areas.strip, id, overview, tops, view, past_end);
        if let Some(offset) = pick.scroll_to {
            self.scroll_to = Some(offset);
            ui.ctx().request_repaint();
        }
        pick.past_end.map(RenderedEvent::SwitchPastEnd).or(event)
    }

    /// The text filled the pane last frame: Full width was chosen, or the reading column was as
    /// wide as the pane allows.
    pub(crate) fn fills_width(&self) -> bool {
        self.fills
    }

    /// The width toggle: from a full pane to the reading column (back to its default width when
    /// the one chosen would fill the pane anyway), or to full width.
    pub(crate) fn switch_width(&self, width: &mut RenderedWidth) {
        if !self.fills {
            width.full = true;
            return;
        }
        width.full = false;
        if width.reading >= self.reading_room {
            width.reading = RenderedWidth::DEFAULT_READING;
        }
    }

    /// Comments were switched on or off in the diff: the threads shown one by one go back under
    /// the switch. The block at the top of the view stays where it is (see `recompute_offsets`).
    fn follow_thread_visibility(&mut self, show_threads: bool) {
        if self.show_threads != show_threads {
            self.show_threads = show_threads;
            self.revealed.clear();
            self.heights.mark_dirty();
        }
    }

    /// The model held for `model`'s key: the loaded one taken in once per pair of versions, a
    /// later load of the same keeps the held one. New texts start at the top (or where
    /// [`Self::reveal_line`] asked for).
    fn ensure_model(&mut self, model: &Arc<MarkdownModel>) -> Arc<MarkdownModel> {
        if let Some(shown) = self.model.as_ref().filter(|shown| shown.key == model.key) {
            return Arc::clone(shown);
        }
        *self = Self {
            model: Some(Arc::clone(model)),
            pending: Some(self.pending.take().unwrap_or(Place::Top)),
            viewport: self.viewport,
            find: std::mem::take(&mut self.find),
            ..Self::default()
        };
        Arc::clone(model)
    }

    /// The rows for `key`, built with `threads`; the block at the top of the view stays there.
    fn ensure_rows(&mut self, model: &MarkdownModel, key: RowsKey, threads: &[&ReviewThread]) {
        if self.rows_key == Some(key) {
            return;
        }
        let keep = self
            .rows_key
            .and_then(|_| self.top_unit())
            .map(|(_, side, unit, below_top)| Place::Unit { side, unit, below_top });
        let built = build(model, key.mode, key.context, &self.expanded, threads);
        self.heights.reset(built.rows.len());
        self.rows = built.rows;
        self.folded_threads = built.folded_threads;
        self.thread_ids = threads.iter().map(|thread| thread.id.clone()).collect();
        self.spaces = built.spaces;
        self.hunks = built.hunks;
        self.overview = built.overview;
        self.old_rows = built.old_rows;
        self.new_rows = built.new_rows;
        self.current = None;
        if self.pending.is_none() {
            self.pending = keep;
        }
        self.mode = key.mode;
        self.rows_key = Some(key);
    }

    /// Measures the rows near the view (before anything is placed), places the rows, goes where
    /// asked, and measures a few more rows in the frame's spare time.
    fn layout(
        &mut self,
        ui: &mut Ui,
        model: &MarkdownModel,
        lanes: &Lanes,
        options: RenderOptions,
        view_height: f32,
        id: Id,
    ) {
        // An image that arrived changes the height of its block: every block is measured again
        // (the view keeps its place, see `recompute_offsets`).
        self.heights.rekey(egui::epaint::util::hash((
            lanes.texts.map(|text| text.span().to_bits()),
            lanes.wides.map(f32::to_bits),
            options.scale.to_bits(),
            ui.ctx().pixels_per_point().to_bits(),
            self.mode,
            image_generation(ui.ctx()),
        )));
        self.prepare_thread_place();
        if self.cards.any_easing() {
            // Cards easing to their height move the rows below them every frame until they land.
            self.heights.mark_dirty();
            ui.ctx().request_repaint();
        }
        if self.heights.is_dirty() {
            self.recompute_offsets(model, lanes, options.scale);
            self.cards.settle();
        }
        self.apply_pending(model, view_height);
        if self.pending.is_some() {
            // A fold opens for the place asked for: the rows are rebuilt next frame.
            ui.ctx().request_repaint();
        }
        self.measure_near_view(ui, model, lanes, options, view_height, id);
        self.measure_in_spare_time(ui, model, lanes, options, view_height, id);
    }

    /// Rows a screen above and below the view are measured before anything is placed, so
    /// scrolling into them moves nothing.
    fn measure_near_view(
        &mut self,
        ui: &mut Ui,
        model: &MarkdownModel,
        lanes: &Lanes,
        options: RenderOptions,
        view_height: f32,
        id: Id,
    ) {
        for _ in 0..4 {
            let top = self.view_top();
            let stale: Vec<usize> = self
                .heights
                .rows_between(top - view_height, top + 2.0 * view_height)
                .filter(|row| !self.heights.is_measured(*row))
                .collect();
            if stale.is_empty() {
                break;
            }
            for row in stale {
                self.heights.measure(ui, model, lanes, options, self.rows[row].cells(), id);
            }
            self.recompute_offsets(model, lanes, options.scale);
        }
    }

    /// The other rows are measured nearest the view first, for at most [`MEASURE_BUDGET`] a
    /// frame, until all are.
    fn measure_in_spare_time(
        &mut self,
        ui: &mut Ui,
        model: &MarkdownModel,
        lanes: &Lanes,
        options: RenderOptions,
        view_height: f32,
        id: Id,
    ) {
        if self.heights.is_all_measured() {
            return;
        }
        let deadline = Instant::now() + MEASURE_BUDGET;
        let center = self.heights.row_at(self.view_top() + view_height / 2.0);
        let mut measured = false;
        for row in outward(center, self.rows.len()) {
            if self.heights.is_measured(row) {
                continue;
            }
            self.heights.measure(ui, model, lanes, options, self.rows[row].cells(), id);
            measured = true;
            if Instant::now() > deadline {
                break;
            }
        }
        if measured {
            self.recompute_offsets(model, lanes, options.scale);
        }
        if !self.heights.is_all_measured() {
            ui.ctx().request_repaint();
        }
    }

    /// Row tops from the heights known; the row at the top of the view keeps its place on screen.
    fn recompute_offsets(&mut self, model: &MarkdownModel, lanes: &Lanes, scale: f32) {
        let anchor = self.heights.is_placed().then(|| self.anchor()).flatten();
        let (cards, ids, revealed) = (&self.cards, &self.thread_ids, &self.revealed);
        let show_threads = self.show_threads;
        let card = |thread: &ThreadRow| {
            let id = &ids[thread.thread];
            if is_hidden(show_threads, revealed, id) {
                0.0
            } else {
                cards.shown(id).unwrap_or(DEFAULT_CARD_HEIGHT)
            }
        };
        self.heights.place(&self.rows, &self.spaces, model, lanes, scale, card);
        if let Some((row, below_top)) = anchor {
            // A card at the top of the view that goes (comments hidden) leaves the view on what
            // followed it.
            let top = (self.heights.top(row) + below_top.min(self.heights.height(row))).max(0.0);
            if (top - self.view_top()).abs() > 0.25 {
                self.scroll_to = Some(top);
            }
        }
    }

    /// The threads whose cards follow block row `row` in `lane`, by their index.
    fn lane_threads(&self, row: usize, lane: DiffSide) -> impl Iterator<Item = usize> + '_ {
        self.rows[row + 1..]
            .iter()
            .map_while(|row| match row {
                Row::Thread(thread) => Some(*thread),
                _ => None,
            })
            .filter(move |thread| thread.lane == lane)
            .map(|thread| thread.thread)
    }

    /// The row of the card of thread `id`.
    fn thread_row(&self, id: &str) -> Option<usize> {
        self.rows.iter().position(
            |row| matches!(row, Row::Thread(thread) if self.thread_ids[thread.thread] == id),
        )
    }

    /// A thread asked for ([`Place::Thread`]): shown while comments are hidden, its fold opened
    /// (the rows are rebuilt next frame); forgotten when the rows do not hold it.
    fn prepare_thread_place(&mut self) {
        let Some(Place::Thread(id)) = self.pending.clone() else {
            return;
        };
        if !self.show_threads && self.revealed.insert(id.clone()) {
            self.heights.mark_dirty();
        }
        if self.thread_row(&id).is_some() {
            return;
        }
        let fold = self
            .folded_threads
            .iter()
            .find(|(_, thread)| self.thread_ids[*thread] == id)
            .map(|(start, _)| *start);
        match fold {
            Some(start) => {
                self.expanded.insert(start);
                self.expanded_version += 1;
            }
            None => self.pending = None,
        }
    }

    /// What the gutter count of the threads under block row `row` in `lane` stands for.
    fn hidden_threads(
        &self,
        row: usize,
        lane: DiffSide,
        frame: &Frame<'_>,
        threads_ui: &ThreadsUi,
    ) -> markers::Hidden {
        let mut hidden = markers::Hidden::default();
        for thread in self.lane_threads(row, lane) {
            hidden.count += 1;
            hidden.is_shown |= self.revealed.contains(&self.thread_ids[thread]);
            hidden.has_news |=
                frame.threads.get(thread).is_some_and(|thread| threads_ui.is_unread(thread));
        }
        hidden
    }

    /// Shows the threads under block row `row` in `lane` while comments are hidden, or hides them again.
    fn toggle_revealed(&mut self, row: usize, lane: DiffSide, show: bool) {
        let ids: Vec<String> =
            self.lane_threads(row, lane).map(|thread| self.thread_ids[thread].clone()).collect();
        for id in ids {
            if show {
                self.revealed.insert(id);
            } else {
                self.revealed.remove(&id);
            }
        }
        self.heights.mark_dirty();
    }

    /// Where the view will be this frame: where it was asked to go, or where it was.
    fn view_top(&self) -> f32 {
        self.scroll_to.unwrap_or(self.viewport.0.top())
    }

    /// The row at the top of the view and how far into it the view starts; `None` at the very top.
    fn anchor(&self) -> Option<(usize, f32)> {
        let top = self.view_top();
        if top <= 0.0 || self.rows.is_empty() {
            return None;
        }
        let row = self.heights.row_at(top);
        Some((row, top - self.heights.top(row)))
    }

    /// The first block at or below the top of the view: its row, side and unit, and how far the
    /// view starts below the row's top (negative when the row starts lower).
    fn top_unit(&self) -> Option<(usize, DiffSide, usize, f32)> {
        if !self.heights.is_placed() {
            return None;
        }
        let top = self.view_top();
        (self.heights.row_at(top)..self.rows.len()).find_map(|row| match &self.rows[row] {
            Row::Blocks { left, right } => {
                right.or(*left).map(|cell| (row, cell.side, cell.unit, top - self.heights.top(row)))
            }
            Row::Folded { .. } | Row::Removal { .. } | Row::Thread(_) => None,
        })
    }

    /// Scrolls where [`Self::pending`] asks once the rows are placed. A line in a folded run opens
    /// the fold first and is placed in the next frame.
    fn apply_pending(&mut self, model: &MarkdownModel, view_height: f32) {
        let Some(place) = self.pending.take() else {
            return;
        };
        if !self.heights.is_placed() {
            self.pending = Some(place);
            return;
        }
        let target = match place {
            Place::Top => 0.0,
            // A third down the view, as the text diff puts a thread, with its block above it.
            Place::Thread(id) => {
                let Some(row) = self.thread_row(&id) else {
                    // Its fold opens: the rows hold it next frame.
                    self.pending = Some(Place::Thread(id));
                    return;
                };
                (self.heights.top(row) - view_height / 3.0).min(self.heights.total() - view_height)
            }
            Place::Unit { side, unit, below_top } => {
                match self.row_of_unit(model, side, unit, false) {
                    Some(row) => self.heights.top(row) + below_top,
                    None => return,
                }
            }
            Place::Line { side, line } => {
                let units = &model.units(side).units;
                let Some(unit) = units
                    .iter()
                    .position(|unit| unit.lines.end > line)
                    .or(units.len().checked_sub(1))
                else {
                    return;
                };
                let Some(row) = self.row_of_unit(model, side, unit, true) else {
                    self.pending = Some(place);
                    return;
                };
                let lines = &units[unit].lines;
                let span = lines.end.saturating_sub(lines.start).max(1) as f32;
                let fraction = (line.saturating_sub(lines.start) as f32 / span).clamp(0.0, 1.0);
                self.heights.top(row) + fraction * self.heights.height(row)
            }
            Place::Find { side, unit } => {
                let Some(row) = self.row_of_unit(model, side, unit, true) else {
                    self.pending = Some(place);
                    return;
                };
                let (top, total) = (self.heights.top(row), self.heights.total());
                navigation::center_offset(top..top + self.heights.height(row), view_height, total)
            }
        };
        self.scroll_to = Some(target.max(0.0));
    }

    /// The row showing `unit` of `side`, or where it would be: a fold holding it (opened instead
    /// with `open_fold`, then `None` until the rows are rebuilt), or New only's next row.
    fn row_of_unit(
        &mut self,
        model: &MarkdownModel,
        side: DiffSide,
        unit: usize,
        open_fold: bool,
    ) -> Option<usize> {
        let rows = match side {
            DiffSide::Old => &self.old_rows,
            DiffSide::New => &self.new_rows,
        };
        if let Some(row) = rows.get(unit).copied().flatten() {
            return Some(row);
        }
        let pair = model.pair_of(side, unit)?;
        let row = self
            .rows
            .iter()
            .position(|row| row.pairs().end > pair)
            .unwrap_or(self.rows.len().saturating_sub(1));
        if open_fold && let Some(Row::Folded { pairs }) = self.rows.get(row) {
            self.expanded.insert(pairs.start);
            self.expanded_version += 1;
            return None;
        }
        (row < self.rows.len()).then_some(row)
    }

    /// ↑/↓ scroll a little, Page Up/Down a screen, Home/End to the ends, N/P (F7) step through
    /// the changes; only while no field or button has the keyboard (N/P also over a button).
    fn keyboard(&mut self, ui: &Ui, view_height: f32, past_end: bool) {
        if let Some(direction) = navigation::navigation_keys(ui) {
            self.go_to_change(direction);
        }
        let Some(key) = keys::read(ui) else {
            return;
        };
        let page = (view_height - ARROW_STEP).max(ARROW_STEP);
        let ends = Ends::of(self.heights.tops(), view_height, past_end);
        self.scroll_to = Some(keys::target(key, self.view_top(), ARROW_STEP, page, ends));
        ui.ctx().request_repaint();
    }

    fn forget_change_out_of_view(&mut self) {
        let Some(hunk) = self.current.and_then(|index| self.hunks.get(index)) else {
            return;
        };
        let last = hunk.end - 1;
        let top = self.heights.top(hunk.start);
        let bottom = self.heights.top(last) + self.heights.height(last);
        if bottom < self.viewport.0.top() || top > self.viewport.0.bottom() {
            self.current = None;
        }
    }

    /// Paints the rows overlapping `viewport` (row coordinates) and returns what the user did.
    fn paint_rows(
        &mut self,
        ui: &mut Ui,
        frame: &Frame<'_>,
        viewport: Rect,
        threads_ui: &mut ThreadsUi,
    ) -> Option<RenderedEvent> {
        let palette = theme::palette(ui.ctx());
        #[cfg(test)]
        {
            self.probe = tests::Probe::default();
        }
        let mut event = None;
        let mut moved = false;
        let rows: Vec<usize> =
            self.heights.rows_between(viewport.top(), viewport.bottom()).collect();
        for &index in &rows {
            let rect = Rect::from_min_size(
                frame.origin + vec2(0.0, self.heights.top(index)),
                vec2(frame.width, self.heights.height(index)),
            );
            let row_event = match self.rows[index].clone() {
                Row::Blocks { left, right } => {
                    let cells = [left, right];
                    let (blocks_event, resized) =
                        self.paint_blocks(ui, frame, index, rect, cells, threads_ui);
                    moved |= resized;
                    blocks_event
                }
                Row::Folded { pairs } => {
                    self.paint_fold(ui, frame, rect, pairs);
                    None
                }
                Row::Thread(thread_row) => self.paint_card(ui, frame, rect, thread_row, threads_ui),
                Row::Removal { pairs } => {
                    let top = self.heights.top(index);
                    let below = self.heights.tops().get(index + 1).copied().unwrap_or(top);
                    let y = frame.origin.y + f32::midpoint(top, below);
                    show_removal(ui, frame, y, pairs, &palette)
                        .then_some(RenderedEvent::ShowSideBySide)
                }
            };
            if row_event.is_some() {
                event = row_event;
            }
        }
        if frame.mode == Mode::Columns {
            paint_column_divider(ui, frame, &self.rows, &self.heights, &rows, viewport, &palette);
        }
        if moved {
            // A block laid out at another height than measured (a font arriving): place again.
            self.heights.mark_dirty();
            ui.ctx().request_repaint();
        }
        event
    }

    /// The blocks of row `index` at `rect` in their lanes (a hatched gap where a side has none);
    /// what the user did there, and true when a block was laid out at another height than known.
    fn paint_blocks(
        &mut self,
        ui: &mut Ui,
        frame: &Frame<'_>,
        index: usize,
        rect: Rect,
        [left, right]: [Option<Cell>; 2],
        threads_ui: &mut ThreadsUi,
    ) -> (Option<RenderedEvent>, bool) {
        let palette = theme::palette(ui.ctx());
        let cells = match frame.mode {
            Mode::Columns => vec![(DiffSide::Old, left), (DiffSide::New, right)],
            Mode::Unified | Mode::NewOnly | Mode::Whole(_) => {
                vec![(left.map_or(DiffSide::New, |cell| cell.side), left)]
            }
        };
        let mut event = None;
        let mut moved = false;
        for (side, cell) in cells {
            let lane = frame.lanes.lane(side);
            let lane = Rect::from_x_y_ranges(
                lane.min + frame.origin.x..=lane.max + frame.origin.x,
                rect.y_range(),
            );
            #[cfg(test)]
            {
                self.probe.cells.push(tests::ProbeCell {
                    row: index,
                    side,
                    unit: cell.map(|cell| cell.unit),
                    rect: lane,
                });
            }
            let Some(cell) = cell else {
                diff_paint::paint_empty(ui.painter(), lane, &palette);
                continue;
            };
            let hidden = (!frame.show_threads)
                .then(|| self.hidden_threads(index, side, frame, threads_ui))
                .filter(|hidden| hidden.count > 0);
            let shown = show_cell(ui, frame, cell, lane, &palette, hidden);
            if shown.event.is_some() {
                event = shown.event;
            }
            if shown.toggles_threads
                && let Some(hidden) = hidden
            {
                self.toggle_revealed(index, side, !hidden.is_shown);
                ui.ctx().request_repaint();
            }
            moved |= self.heights.note_shown(cell, shown.height);
        }
        (event, moved)
    }

    /// The bar of the folded `pairs` at `rect`; clicked, the fold opens, and when it holds
    /// comments the first one comes into view.
    fn paint_fold(&mut self, ui: &mut Ui, frame: &Frame<'_>, rect: Rect, pairs: Range<usize>) {
        let mut threads = self
            .folded_threads
            .iter()
            .filter(|(start, _)| *start == pairs.start)
            .map(|(_, thread)| *thread);
        let first = threads.next();
        let comments = first.map_or(0, |_| 1 + threads.count());
        if show_fold(ui, rect, frame, pairs.clone(), comments) {
            self.expanded.insert(pairs.start);
            self.expanded_version += 1;
            if let Some(thread) = first {
                self.pending = Some(Place::Thread(self.thread_ids[thread].clone()));
            }
            ui.ctx().request_repaint();
        }
    }

    /// The card of `thread_row`'s thread on a band across `rect`, under its lane's column; nothing
    /// while it is hidden.
    fn paint_card(
        &mut self,
        ui: &mut Ui,
        frame: &Frame<'_>,
        rect: Rect,
        thread_row: ThreadRow,
        threads_ui: &mut ThreadsUi,
    ) -> Option<RenderedEvent> {
        let id = &self.thread_ids[thread_row.thread];
        let thread = frame.threads.get(thread_row.thread).copied().filter(|thread| {
            thread.id == *id && !is_hidden(self.show_threads, &self.revealed, id)
        })?;
        let column = frame.lanes.column(thread_row.lane);
        let lane = Rangef::new(column.min + frame.origin.x, column.max + frame.origin.x);
        let is_easing = self.cards.is_easing(&thread.id);
        let (height, action) =
            threads::show_card_row(ui, rect, lane, thread, thread_row.note, is_easing, threads_ui);
        let event = match action {
            Some(CardAction::Thread(thread_event)) => Some(RenderedEvent::Thread(thread_event)),
            Some(CardAction::ShowSource) => {
                Some(RenderedEvent::ShowThreadSource { thread_id: thread.id.clone() })
            }
            None => None,
        };
        if self.cards.store(&thread.id, height, threads_ui.is_settling(&thread.id)) {
            self.heights.mark_dirty();
            ui.ctx().request_repaint();
        }
        // A card laid out at another height than its row has this pass is redone at the right
        // one, so the rows below never show at a stale place.
        let is_off =
            self.cards.shown(&thread.id).is_some_and(|shown| (shown - rect.height()).abs() > 0.5);
        if is_off && !is_easing && cards::can_discard(ui.ctx()) {
            ui.ctx().request_discard("a comment card was measured");
        }
        event
    }
}
