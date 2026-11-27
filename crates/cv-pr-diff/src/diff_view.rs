//! Virtualized diff: code rows have a fixed height (a multiple of it when wrapped), comment cards
//! are measured once and cached. Only rows intersecting the viewport are laid out or painted, so a
//! 50k-line file scrolls like a 50-line one.
//!
//! A card whose height changes because a comment was handed on (posted, answered by the platform,
//! shown by the reloaded threads, failed) eases to its new height (see [`cards`]) instead of
//! moving the rows below in one jump; the cached row height follows the ease, so rows above the
//! view and the scroll offset stay put.
//!
//! The view keeps its rows in [`rows::DiffRows`] (built when what they show changes), their tops
//! in [`offsets::RowOffsets`], where the view is in [`place::Place`] and the selection in
//! [`selecting::Selecting`]. Code rows paint in [`code_rows`], comment cards and the new comment
//! in [`card_rows`], removed lines of New only in [`removals`]; [`panes`] holds the sides and their
//! sideways scrolling, [`requests`] what other views ask for. This module lays the rows out and
//! paints the ones in view.

mod card_rows;
pub mod cards;
mod code_rows;
mod frame;
pub mod navigation;
mod offsets;
mod panes;
mod place;
mod removals;
mod requests;
mod rows;
mod selecting;

use std::sync::Arc;

use egui::{Rect, Ui, UiBuilder, pos2, vec2};
use review_core::{DiffSide, NewThread, ThreadAnchor};

use self::card_rows::{CardRow, Drafting, Placed, Revealed};
use self::cards::CardHeights;
#[cfg(any(test, feature = "test-support"))]
pub use self::code_rows::flash::{flash_asks_for_frames, flashed_lines};
use self::code_rows::menu::{self, MenuChoice, MenuItems};
use self::code_rows::{CodeRows, RowAction};
pub use self::frame::{DiffInputs, Embedded, EmbeddedOutput, keep_below};
use self::frame::{Frame, RowLayout, Screen, Sight, syntax_mix};
pub use self::navigation::{ChangePosition, Direction};
use self::offsets::RowOffsets;
use self::panes::{Sideways, find_panes, left_pane_width, pane_rects};
use self::place::Place;
use self::removals::Popover;
pub use self::requests::LineRequest;
use self::requests::Requests;
pub(crate) use self::rows::DiffRows;
use self::rows::{Change, KeepPlace};
use self::selecting::{Code, Drag, KeysView, Selecting};
use crate::detached_threads::{self, Detached, DetachedEvent};
use crate::diff_overview::past_end::Ends;
use crate::diff_status::{self, StatusAction};
use crate::find::bar::Panes;
use crate::find::text::{TextFind, TextRows};
use crate::gap_bar::{self, GapBar};
use crate::line_anchor::LineAnchor;
use crate::removal_popover::RemovalPopover;
use crate::selection::{DiffSelection, Selection};
use cv_base::platform::shortcut;
use cv_base::theme;
#[cfg(any(test, feature = "test-support"))]
use cv_code::diff_rows::RangeOwner;
use cv_code::diff_rows::{self, Layout, Row, RowKind};
use cv_code::gaps::Expand;
use cv_code::line_source::{self, Pane};
use cv_pr_threads::{ThreadEvent, ThreadsUi};
use cv_widgets::surfaces;

pub const OVERVIEW_WIDTH: f32 = 14.0;
/// Opens a comment on the selected lines.
pub(crate) const COMMENT_SHORTCUT: &str = shortcut!(alt "C");

#[derive(Debug)]
pub enum DiffEvent {
    Thread(ThreadEvent),
    /// Post the new thread drafted under `key`.
    Create {
        key: String,
        thread: NewThread,
    },
    ColorAnyway,
    RetryContents,
    /// New only has nothing to show (the head version is empty); the user picked another layout.
    SwitchLayout(Layout),
    /// The user finished moving the boundary between the old and new side.
    SplitResized,
    /// The user switched "Scroll past the end" in the overview strip's menu, to this.
    SwitchPastEnd(bool),
}

#[derive(Default)]
pub struct DiffView {
    /// The rows and what is worked out with them, built again when what they show changes.
    built: DiffRows,
    offsets: RowOffsets,
    /// Comment card heights by thread id (the new comment's under [`COMPOSER_KEY`]).
    cards: CardHeights,
    selecting: Selecting,
    sideways: Sideways,
    drafting: Drafting,
    place: Place,
    requests: Requests,
    /// Row and pane of the last secondary click, for the context menu.
    menu_target: Option<(usize, Pane)>,
    /// New only: the popover of removed lines.
    removal: RemovalPopover,
    revealed: Revealed,
    #[cfg(any(test, feature = "test-support"))]
    /// What the last frame painted and where, for tests that point at code.
    probe: Option<Probe>,
    /// The boundary between the sides is being dragged: rows show no hover or comment button.
    is_resizing: bool,
    /// The find widget and its hits.
    find: TextFind,
}

impl DiffView {
    pub fn reset(&mut self) {
        *self = DiffView { place: Place::opening(), ..DiffView::default() };
    }

    /// Layout switch: same file and revealed lines, new rows (built for the new layout when it is
    /// shown next, keeping the line at the top of the view in place).
    pub fn relayout(&mut self) {
        self.offsets.mark_dirty();
        self.sideways = Sideways::default();
        self.place.current_change = None;
        self.cards.clear();
        self.removal.close();
        self.selecting.clear();
    }

    /// A new comment is being written in this diff.
    pub fn composer(&self) -> Option<&crate::diff_composer::Composer> {
        self.drafting.composer.as_ref()
    }

    /// Scrolls to the thread once its row exists, showing it while comments are hidden in the diff.
    pub fn reveal_thread(&mut self, thread_id: &str) {
        self.requests.reveal_thread(thread_id.to_owned());
        if self.revealed.insert(thread_id) {
            self.offsets.mark_dirty();
        }
    }

    /// Carries out `request` once the rows are built, showing collapsed lines it needs.
    pub fn request_lines(&mut self, request: LineRequest) {
        self.requests.request_lines(request);
    }

    /// The first code line at the top of the view and its side, for the rendered Markdown diff to
    /// open there; `None` at the very top.
    pub fn top_line(&self) -> Option<(DiffSide, u32)> {
        self.place.top_line(&self.built, &self.offsets)
    }

    /// Where the change in focus is among the changes, for the header.
    pub fn change_position(&self) -> ChangePosition {
        ChangePosition { current: self.place.current_change, total: self.built.changes.len() }
    }

    pub fn go_to_change(&mut self, direction: Direction) {
        self.place.go_to_change(direction, &self.built.changes, &self.offsets);
    }

    /// A step (N, P or a change arrow) found no change further that way: its direction, once.
    pub fn take_ran_out(&mut self) -> Option<Direction> {
        self.place.ran_out.take()
    }

    /// `split` is the old side's share of a side-by-side diff; the user can drag it.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        inputs: &DiffInputs<'_>,
        split: &mut f32,
        threads_ui: &mut ThreadsUi,
    ) -> Option<DiffEvent> {
        self.place.is_embedded = false;
        self.rebuild_rows(ui, inputs, threads_ui);
        if inputs.layout == Layout::NewOnly && self.built.has_no_head_lines() {
            #[cfg(any(test, feature = "test-support"))]
            {
                self.probe = None;
            }
            return removals::show_empty_head(ui, inputs.diff);
        }
        let mut event = self.show_detached(ui, inputs, threads_ui);
        let full = ui.available_rect_before_wrap();
        let content =
            Rect::from_min_max(full.min, pos2(full.right() - OVERVIEW_WIDTH, full.bottom()));
        let strip = Rect::from_min_max(pos2(content.right(), full.top()), full.max);
        let left_width = left_pane_width(inputs.layout, content.width(), *split);
        self.prepare_find(ui, inputs, find_panes(inputs.layout, content, left_width));
        let sight = Sight { height: content.height(), keep_below: keep_below(ui.ctx(), content) };
        let layout = self.layout_rows(ui, inputs, content.width(), left_width, sight, true);
        self.apply_line_request(ui.ctx(), inputs, content.height());
        if let Some(direction) = navigation::navigation_keys(ui) {
            self.go_to_change(direction);
        }
        self.update_range_target(inputs);
        self.selection_keys(ui, inputs, content, left_width);
        self.update_range_target(inputs);

        ui.painter().rect_filled(content, 0, theme::palette(ui.ctx()).surface);
        surfaces::mark_code(ui, full);
        if layout.wrap_columns.is_none() {
            surfaces::mark_horizontal_scroll(ui, content);
        }
        let background = self.selecting.area(ui, content, inputs.path);
        let reach = self.sideways.clamp(self.built.widest, &layout);
        let screen = Screen { content, strip, left_width, layout: &layout, reach };
        if let Some(row_event) = self.show_scrolled(ui, &screen, &background, inputs, threads_ui) {
            event = Some(row_event);
        }
        if let Some(over_event) = self.show_overlays(ui, &screen, &background, inputs, split) {
            event = Some(over_event);
        }
        // Over the rows, so it paints on top of them and takes its own clicks.
        if self.find.show_widget(ui) {
            ui.memory_mut(|memory| memory.request_focus(ui.scope_id().with("diff_area")));
        }
        event
    }

    /// The find widget's keys and search over `panes` before the rows (see [`TextFind`]).
    fn prepare_find(&mut self, ui: &Ui, inputs: &DiffInputs<'_>, panes: Panes) {
        let area_id = ui.scope_id().with("diff_area");
        let owns_keys = ui.rect_contains_pointer(ui.available_rect_before_wrap())
            || ui.memory(|memory| memory.has_focus(area_id));
        let (built, selecting) = (&mut self.built, &self.selecting);
        let mut rows = TextRows {
            panes,
            rows: &built.rows,
            gaps: &built.gaps,
            generation: built.generation,
            offsets: self.offsets.as_slice(),
            view: self.place.viewport.0,
            overview: &mut built.overview,
            sideways: &mut self.sideways.offsets,
            selected: selecting.in_line(&built.rows, inputs),
            selection: selecting.selection,
        };
        let selected = || selecting.text(&built.rows, inputs);
        if let Some(request) = self.find.prepare(ui, inputs, owns_keys, selected, &mut rows) {
            self.requests.request_lines(request);
        }
    }

    /// Over the rows on `screen`: a drag's edge scroll, the context menu, the removed lines'
    /// popover, sideways scrollbars, the sides' boundary (at `split`), overview strip and status.
    fn show_overlays(
        &mut self,
        ui: &mut Ui,
        screen: &Screen<'_>,
        background: &egui::Response,
        inputs: &DiffInputs<'_>,
        split: &mut f32,
    ) -> Option<DiffEvent> {
        let content = screen.content;
        if let Some(offset) = self.selecting.edge_scroll(
            ui,
            content,
            self.place.viewport.0,
            self.offsets.height(),
            self.place.is_embedded,
        ) {
            self.place.scroll_to = Some(offset);
        }
        self.context_menu(background, inputs);
        let popover = Popover {
            metrics: &screen.layout.metrics,
            bounds: content,
            can_comment: inputs.can_comment && self.drafting.composer.is_none(),
        };
        if let Some((anchor, row_index)) =
            removals::show_popover(ui, &mut self.removal, &self.built.rows, inputs, &popover)
        {
            self.open_composer(anchor, false, row_index);
        }
        if screen.layout.wrap_columns.is_none() {
            self.sideways.show_scrollbars(ui, screen, inputs.layout);
        }
        let drag = panes::split_handle(ui, inputs.layout, content, content.y_range(), split);
        self.is_resizing = drag.is_dragged;
        let resized = drag.committed.then_some(DiffEvent::SplitResized);
        let switch = self.place.follow_strip(
            ui,
            screen.strip,
            &self.built.overview,
            self.offsets.as_slice(),
            inputs.past_end,
        );
        match diff_status::show(ui, content, inputs.contents) {
            Some(StatusAction::ColorAnyway) => Some(DiffEvent::ColorAnyway),
            Some(StatusAction::Retry) => Some(DiffEvent::RetryContents),
            None => switch.map(DiffEvent::SwitchPastEnd).or(resized),
        }
    }

    /// The threads whose line is in no row, above the rows.
    fn show_detached(
        &mut self,
        ui: &mut Ui,
        inputs: &DiffInputs<'_>,
        threads_ui: &mut ThreadsUi,
    ) -> Option<DiffEvent> {
        if self.built.detached.is_empty() {
            return None;
        }
        let detached = Detached {
            threads: &self.built.detached,
            rows: &self.built,
            can_expand: inputs.can_expand(),
        };
        match detached_threads::show(ui, &detached, inputs, threads_ui)? {
            DetachedEvent::Thread(thread_event) => Some(DiffEvent::Thread(thread_event)),
            DetachedEvent::ShowLine { gap, thread_id } => {
                self.built.expand_gap(gap, Expand::All);
                self.requests.reveal_thread(thread_id);
                None
            }
        }
    }

    /// The rows in their scroll area: the pointer selects on them, the ones in view paint, and
    /// the view's place is kept for the next frame.
    fn show_scrolled(
        &mut self,
        ui: &mut Ui,
        screen: &Screen<'_>,
        background: &egui::Response,
        inputs: &DiffInputs<'_>,
        threads_ui: &mut ThreadsUi,
    ) -> Option<DiffEvent> {
        let metrics = &screen.layout.metrics;
        let wrap_width = screen
            .layout
            .wrap_columns
            .map(|columns| columns.map(|columns| columns as f32 * metrics.char_width + 0.5));
        let ends = Ends::of(self.offsets.as_slice(), screen.content.height(), inputs.past_end);
        let scroll = egui::ScrollArea::vertical()
            .id_salt("diff_scroll")
            .auto_shrink(false)
            .scroll_source(egui::scroll_area::ScrollSource::MOUSE_WHEEL)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        let placed = self.place.scroll_to.take();
        self.removal.begin_frame();
        // Inline, New only and a whole added or deleted file show one full-width pane, without a splitter.
        let single_pane = inputs.layout.is_single_pane();
        let output = ui
            .scope_builder(UiBuilder::new().max_rect(screen.content), |ui| {
                cv_widgets::scroll::placed(ui, scroll, placed).show_viewport(ui, |ui, viewport| {
                    ui.set_min_height(ends.height);
                    let frame = Frame {
                        origin: ui.max_rect().min,
                        width: screen.content.width(),
                        left_width: screen.left_width,
                        metrics,
                        wrap_width,
                        syntax_mix: syntax_mix(ui.ctx(), inputs),
                    };
                    if wrap_width.is_none() {
                        self.sideways.wheel(ui, &frame, single_pane, screen.reach.max_offsets);
                    }
                    self.track_pointer(ui, background, &frame, inputs);
                    self.paint_rows(ui, &frame, viewport, inputs, threads_ui)
                })
            })
            .inner;
        self.place.viewport.0 = output.inner_rect.translate(
            vec2(0.0, output.state.clamped_offset().y) - output.inner_rect.min.to_vec2(),
        );
        #[cfg(any(test, feature = "test-support"))]
        {
            let top = self.place.top();
            let row = self.offsets.row_at(top);
            let below_top = top - self.offsets.get(row).unwrap_or(0.0);
            cv_shell::focus_mode::note_top_line(
                ui.ctx(),
                cv_shell::focus_mode::Surface::Files,
                row,
                below_top,
            );
        }
        self.place.forget_change_out_of_view(&self.built.changes, &self.offsets);
        output.inner
    }

    /// Lays out the rows shown inside another view's scrolling (a folder summary's card) before
    /// that view scrolls: then [`Self::height`] is their height and [`Self::take_scroll_request`]
    /// where that view should scroll for them (to show a new comment card in full).
    pub fn prepare_embedded(
        &mut self,
        ui: &Ui,
        inputs: &DiffInputs<'_>,
        threads_ui: &mut ThreadsUi,
        embedded: &Embedded,
    ) {
        self.place.is_embedded = true;
        self.rebuild_rows(ui, inputs, threads_ui);
        self.place.viewport.0 = embedded.local_view();
        let left_width = left_pane_width(inputs.layout, embedded.width, embedded.split);
        self.layout_rows(ui, inputs, embedded.width, left_width, embedded.sight(), true);
    }

    /// Paints the rows inside another view's vertical scrolling, with per-pane sideways scrolling,
    /// a splitter and the file view's selection, comment buttons, cards and context menu.
    /// `has_keyboard`: the selection keys (Shift+↑/↓, Ctrl+C, Ctrl+A, Alt+C, Esc) go to these
    /// rows. Scrolling the rows want is left for [`Self::take_scroll_request`].
    pub fn show_embedded(
        &mut self,
        ui: &mut Ui,
        inputs: &DiffInputs<'_>,
        threads_ui: &mut ThreadsUi,
        embedded: &Embedded,
        has_keyboard: bool,
    ) -> EmbeddedOutput {
        embedded.show(self, ui, inputs, threads_ui, has_keyboard)
    }

    /// The rows' height: code, comment cards and all.
    pub fn height(&self) -> f32 {
        self.offsets.height()
    }

    /// Where the view scrolling these rows should scroll: the y (from the top of the first row)
    /// its top should go to. Only for rows shown with [`Self::show_embedded`].
    pub fn take_scroll_request(&mut self) -> Option<f32> {
        self.place.scroll_to.take()
    }

    /// A selection made elsewhere replaces this one.
    pub fn clear_selection(&mut self) {
        self.selecting.clear();
    }

    /// Some text or lines are selected.
    pub fn has_selection(&self) -> bool {
        self.selecting.is_active()
    }

    /// Takes in a new diff and the threads on it, and opens the composer of a kept draft.
    fn rebuild_rows(&mut self, ui: &Ui, inputs: &DiffInputs<'_>, threads_ui: &mut ThreadsUi) {
        if self.revealed.follow(inputs.show_threads) {
            // Comments were switched on or off in the diff: the code line at the top of the view
            // stays where it is on screen while the cards above it appear or go.
            self.offsets.mark_dirty();
            self.place.keep_first_line(&self.built.rows, &self.offsets);
        }
        self.drafting.hand_over_landed(inputs.threads, threads_ui, &mut self.cards);
        self.drafting.restore(inputs.path, threads_ui);
        // A card just opened goes into the rows only in a pass that can be discarded: that pass
        // measures it, and the next one paints it in its final place (see [`card_rows`]).
        let holds_card = self.drafting.holds_card(ui.ctx());
        if holds_card {
            ui.ctx().request_repaint();
        }
        self.rebuild_if_needed(inputs, threads_ui, holds_card);
        if self.drafting.composer.is_some() && !holds_card && self.built.composer_row.is_none() {
            // Its line is not in this diff (any more): the draft stays kept, the "+" works again.
            self.drafting.composer = None;
            self.rebuild_if_needed(inputs, threads_ui, false);
        }
    }

    /// Row heights for `width` (`left_width` the left pane's) and the code size, and where the view
    /// goes for them in `sight`; `reveal` scrolls a new comment card into view (once per pass).
    fn layout_rows(
        &mut self,
        ui: &Ui,
        inputs: &DiffInputs<'_>,
        width: f32,
        left_width: f32,
        sight: Sight,
        reveal: bool,
    ) -> RowLayout {
        let view_height = sight.height;
        let layout = RowLayout::new(ui, inputs, width, left_width, self.built.max_line_number);
        let height_key = layout.height_key();
        let rows = self.built.rows.len();
        let anchor = self.offsets.rekey(height_key).and_then(|previous| {
            self.place.for_new_heights(previous, height_key, sight, &self.offsets, rows)
        });
        self.cards.begin(ui.ctx());
        if self.cards.any_easing() {
            // Cards easing to their height move the rows below them every frame until they land.
            self.offsets.mark_dirty();
            ui.ctx().request_repaint();
        }
        if self.offsets.is_dirty() {
            let unmeasured_card = self.drafting.unmeasured_height(view_height);
            self.recompute_offsets(inputs, &layout, unmeasured_card);
            self.cards.settle();
        }
        let card = self.drafting.take_keep().zip(self.built.composer_row);
        self.place.keep_places(card, anchor, &self.offsets);
        if reveal {
            let (built, offsets) = (&mut self.built, &self.offsets);
            if let Some(offset) = self.requests.reveal_pending(built, offsets, inputs, view_height)
            {
                self.place.jump(offset, offsets, view_height);
            }
            let placed = Placed {
                rows: &self.built.rows,
                card: self.built.composer_row,
                offsets: &self.offsets,
            };
            if let Some(offset) = self.drafting.reveal_target(
                &placed,
                inputs.diff,
                self.place.top(),
                view_height,
                self.place.is_embedded,
            ) {
                self.place.scroll_to = Some(offset);
            }
        }
        layout
    }

    /// Paints the rows overlapping `viewport` (in row coordinates) and returns what the user did
    /// with them.
    fn paint_rows(
        &mut self,
        ui: &mut Ui,
        frame: &Frame<'_>,
        viewport: Rect,
        inputs: &DiffInputs<'_>,
        threads_ui: &mut ThreadsUi,
    ) -> Option<DiffEvent> {
        #[cfg(any(test, feature = "test-support"))]
        {
            self.probe = Some(Probe::of(frame, inputs.layout));
        }
        let total_height = self.height();
        let mut row_event = None;
        let mut removals = Vec::new();
        let first = self.offsets.first_below(viewport.min.y);
        for row_index in first..self.built.rows.len() {
            let top = self.offsets[row_index];
            if top > viewport.max.y {
                break;
            }
            let kind = &self.built.rows[row_index].kind;
            if matches!(kind, RowKind::Removal) {
                removals.push(row_index);
            }
            #[cfg(any(test, feature = "test-support"))]
            if matches!(kind, RowKind::Line | RowKind::Expanded)
                && let Some(probe) = self.probe.as_mut()
            {
                probe.painted_rows += 1;
            }
            let height = self.offsets[row_index + 1] - top;
            let rect =
                Rect::from_min_size(frame.origin + vec2(0.0, top), vec2(frame.width, height));
            if let Some(shown) = self.show_row(ui, rect, row_index, inputs, frame, threads_ui) {
                row_event = Some(shown);
            }
        }
        // A new card is measured where it goes, in view or not: this pass is discarded.
        if self.drafting.is_measuring()
            && let Some(card) = self.built.composer_row
        {
            let rect = Rect::from_min_max(
                frame.origin + vec2(0.0, self.offsets[card]),
                frame.origin + vec2(frame.width, self.offsets[card + 1]),
            );
            if let Some(shown) = self.show_row(ui, rect, card, inputs, frame, threads_ui) {
                row_event = Some(shown);
            }
        }
        // Wedges sit on the boundary between two rows, so they are painted over both.
        for row_index in removals {
            let row = (row_index, &self.built.rows[row_index]);
            let top = self.offsets[row_index];
            removals::show_wedge(ui, &mut self.removal, row, top, inputs.diff, frame, total_height);
        }
        row_event
    }

    /// Builds the rows again when what they show changed. `holds_card`: leave the composer out of
    /// the rows for now.
    fn rebuild_if_needed(
        &mut self,
        inputs: &DiffInputs<'_>,
        threads_ui: &mut ThreadsUi,
        holds_card: bool,
    ) {
        // A new version of the same file (a live update): an unsent comment follows its line.
        if let (Some(composer), Some(shown)) =
            (self.drafting.composer.as_mut(), self.built.shown_diff.as_ref())
            && !Arc::ptr_eq(shown, inputs.diff)
        {
            composer.follow(shown, inputs.diff, threads_ui);
        }
        let composer = (self.drafting.composer.as_ref())
            .filter(|_| !holds_card)
            .map(|composer| composer.anchor.clone());
        let key = self.built.key_for(inputs, composer);
        let Some(change) = self.built.change_to(&key, inputs.threads) else {
            return;
        };
        // New commits or comments arriving, and a switch between inline and side by side, keep the
        // line at the top of the view where it is. Rows inside another view's scrolling leave the
        // former to that view, which keeps its cards in place.
        let keeps_place = match change.keep_place {
            KeepPlace::Always => true,
            KeepPlace::UnlessEmbedded => !self.place.is_embedded,
            KeepPlace::Never => false,
        };
        // A scroll already asked for, or to a thread, goes elsewhere anyway.
        let goes_elsewhere = self.place.scroll_to.is_some() || self.requests.is_revealing();
        let place = (keeps_place && !goes_elsewhere)
            .then(|| self.place.at_top(&self.built, &self.offsets))
            .flatten();
        // So does the change in focus (see [`Place::focused_lines`]).
        let focused_lines = self.place.focused_lines(&self.built, change.is_new_diff);
        let old_rows = self.built.rebuild(inputs, key);
        self.follow_rebuilt_rows(inputs, &old_rows, &change, place);
        self.place.refocus(&self.built, focused_lines);
    }

    /// After a rebuild that replaced `old_rows` (`change`): the selection follows its lines, the
    /// line at the top of the view keeps its `place`, and an open removal popover, which would
    /// point at another row now, closes.
    fn follow_rebuilt_rows(
        &mut self,
        inputs: &DiffInputs<'_>,
        old_rows: &[Row],
        change: &Change,
        place: Option<(LineAnchor, f32)>,
    ) {
        // A selection follows its lines when a comment card or revealed lines move them. The
        // picked lines stay selected until the card of a new comment on them is in the rows,
        // which mark them from then on: no frame shows them unmarked.
        if let Some(selection) = self.selecting.selection.as_mut()
            && (change.is_new_diff
                || change.card_appears
                || !selection.remap(old_rows, &self.built.rows))
        {
            self.selecting.clear();
        }
        if let Some((anchor, below_top)) = place
            && let Some(row) = self.built.row_of_anchor(&anchor, inputs.diff)
        {
            self.place.keep_row(row, below_top);
        }
        self.removal.close();
        self.offsets.rows_rebuilt();
    }

    /// Carries out the rendered Markdown diff's request once the rows hold its lines: a line
    /// goes to the top of the view (a collapsed gap holding it is enough); lines to comment on
    /// are shown (their gaps expanded first, which rebuilds the rows), selected and the comment
    /// opened on them. The next frame lays the result out (scroll, comment card), so it is asked for.
    fn apply_line_request(
        &mut self,
        ctx: &egui::Context,
        inputs: &DiffInputs<'_>,
        view_height: f32,
    ) {
        let (built, offsets) = (&mut self.built, &self.offsets);
        let Some(outcome) = self.requests.carry_out_lines(built, offsets, inputs, view_height)
        else {
            return;
        };
        ctx.request_repaint();
        self.place.scroll_to = outcome.scroll_to.or(self.place.scroll_to);
        self.place.current_change = outcome.change.or(self.place.current_change);
        if let Some(selection) = outcome.select {
            self.selecting.selection = Some(selection);
            self.update_range_target(inputs);
            if self.drafting.composer.is_none() {
                self.open_range_composer();
            }
        }
    }

    fn recompute_offsets(
        &mut self,
        inputs: &DiffInputs<'_>,
        layout: &RowLayout,
        unmeasured_card: Option<f32>,
    ) {
        let (cards, revealed) = (&self.cards, &self.revealed);
        let card =
            |kind: RowKind| card_rows::row_height(kind, inputs, revealed, cards, unmeasured_card);
        let built = &self.built;
        let (metrics, wrap_columns) = (&layout.metrics, layout.wrap_columns);
        self.offsets.recompute(&built.rows, &built.row_columns, metrics, wrap_columns, card);
    }

    fn show_row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        row_index: usize,
        inputs: &DiffInputs<'_>,
        frame: &Frame<'_>,
        threads_ui: &mut ThreadsUi,
    ) -> Option<DiffEvent> {
        match self.built.rows[row_index].kind {
            RowKind::Gap(gap_index) => {
                let gap = self.built.gaps[gap_index as usize];
                let section = gap
                    .hunk
                    .map_or("", |hunk| inputs.diff.text(&inputs.diff.lines()[hunk as usize]));
                let bar = GapBar {
                    gap: &gap,
                    reveal: self.built.reveals[gap_index as usize],
                    section,
                    can_expand: inputs.can_expand(),
                    has_two_numbers: inputs.layout.has_two_numbers(),
                };
                let id = ui.scope_id().with(("gap", gap_index));
                if let Some(expand) = gap_bar::show(ui, rect, id, &bar, frame.metrics) {
                    self.built.expand_gap(gap_index as usize, expand);
                }
                None
            }
            RowKind::Line | RowKind::Expanded => {
                self.show_code_row(ui, rect, row_index, inputs, frame, threads_ui);
                None
            }
            // Painted after all rows, see [`removals::show_wedge`].
            RowKind::Removal => None,
            RowKind::Separator => {
                let palette = theme::palette(ui.ctx());
                let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                gap_bar::paint_separator(&painter, rect, frame.metrics, &palette);
                None
            }
            RowKind::Thread(index) => {
                let thread = inputs.threads[index as usize];
                if self.revealed.hides(inputs.show_threads, thread) {
                    return None;
                }
                let (thread_event, moved) = card_rows::show_thread_card(
                    ui,
                    rect,
                    thread,
                    inputs.layout,
                    frame,
                    threads_ui,
                    &mut self.cards,
                );
                if moved {
                    self.offsets.mark_dirty();
                }
                thread_event.map(DiffEvent::Thread)
            }
            RowKind::Composer => {
                let below_top = self.offsets.get(row_index).map(|top| top - self.place.top());
                let row = CardRow { rect, below_top };
                let shown =
                    self.drafting.show_card(ui, &row, inputs, frame, threads_ui, &mut self.cards);
                #[cfg(any(test, feature = "test-support"))]
                if let (Some(card), Some(probe)) = (shown.card, self.probe.as_mut()) {
                    probe.composer_card = Some(card);
                }
                if shown.moved {
                    self.offsets.mark_dirty();
                }
                shown.event
            }
        }
    }

    /// Shows the threads on code row `row_index` in `pane` while comments are hidden, or hides them again.
    fn set_revealed(&mut self, row_index: usize, pane: Pane, inputs: &DiffInputs<'_>, show: bool) {
        self.revealed.set(self.built.pane_threads(row_index, pane, inputs.threads), show);
        self.offsets.mark_dirty();
    }

    /// A code row (see [`code_rows`]); then what the user did on it, and a wrapped line laid out
    /// taller than estimated moves the rows below.
    fn show_code_row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        row_index: usize,
        inputs: &DiffInputs<'_>,
        frame: &Frame<'_>,
        threads_ui: &mut ThreadsUi,
    ) {
        let mut rows = CodeRows {
            built: &mut self.built,
            selecting: &self.selecting,
            revealed: &self.revealed,
            h_offset: self.sideways.offsets,
            has_composer: self.drafting.composer.is_some(),
            is_resizing: self.is_resizing,
            flash: &mut self.requests.flash,
            find: &mut self.find,
        };
        let outcome = rows.show(ui, rect, row_index, inputs, frame, threads_ui);
        match outcome.action {
            Some(RowAction::CommentOnSelection) => self.open_range_composer(),
            Some(RowAction::PickLine(pane)) => {
                // The press picks the line; dragging on picks more, and letting go opens the
                // comment on them. The button stays out of the way while the pointer moves on.
                self.selecting.selection = Some(Selection::line(pane, row_index));
                self.selecting.drag = Some(Drag::Plus);
            }
            Some(RowAction::CommentOnLine(anchor)) => self.open_composer(anchor, false, row_index),
            None => {}
        }
        if let Some((pane, show)) = outcome.toggled_threads {
            self.set_revealed(row_index, pane, inputs, show);
            ui.ctx().request_repaint();
        }
        if frame.wrap_width.is_some() {
            let estimate = (rect.height() / frame.metrics.line_height).round() as usize;
            if self.offsets.note_wrapped(row_index, outcome.rows, estimate) {
                ui.ctx().request_repaint();
            }
        }
        if let Some(pane) = outcome.menu {
            self.menu_target = Some((row_index, pane));
        }
    }

    fn context_menu(&mut self, background: &egui::Response, inputs: &DiffInputs<'_>) {
        let Some((row_index, pane)) = self.menu_target else {
            return;
        };
        let Some(row) = self.built.rows.get(row_index).copied() else {
            return;
        };
        let items = MenuItems {
            selection: self.selecting.quote(&self.built.rows, inputs),
            target: self.selecting.target.as_ref().filter(|_| self.drafting.composer.is_none()),
            line: line_source::code_line(inputs.diff, inputs.contents, &row, pane)
                .map(|line| (line.text.to_owned(), line.new_line.max(line.old_line))),
            path: inputs.path,
            can_comment: inputs.can_comment
                && (row.kind == RowKind::Line || inputs.can_comment_outside_hunks),
        };
        let shown = menu::show(background, &items);
        match shown.choice {
            Some(MenuChoice::CommentOnLine) => {
                if let Some(anchor) = diff_rows::anchor_for(
                    inputs.diff,
                    &row,
                    pane != Pane::Left,
                    inputs.path,
                    inputs.old_path,
                ) {
                    self.open_composer(anchor, false, row_index);
                }
            }
            Some(MenuChoice::CommentOnSelection) => self.open_range_composer(),
            Some(MenuChoice::CopySelection) => {
                if let Some(text) = self.selecting.text(&self.built.rows, inputs) {
                    background.ctx.copy_text(text);
                }
            }
            None => {}
        }
        // The menu opens when the button is released, usually frames after the press that picked
        // the target; keep the target until then.
        let is_clicking = background.ctx.input(|input| input.pointer.secondary_down());
        if !shown.is_open && !is_clicking {
            self.menu_target = None;
        }
    }

    /// Works out the comment target for the current selection (see [`Selecting::update_target`]).
    fn update_range_target(&mut self, inputs: &DiffInputs<'_>) {
        let code = Code::new(&self.built, &self.offsets, self.sideways.offsets);
        self.selecting.update_target(&code, inputs);
    }

    /// The selection keys over `content`, its left pane `left_width` wide.
    fn selection_keys(&mut self, ui: &Ui, inputs: &DiffInputs<'_>, content: Rect, left_width: f32) {
        let code = Code::new(&self.built, &self.offsets, self.sideways.offsets);
        let view = KeysView {
            content,
            left_width,
            view: self.place.viewport.0,
            can_escape: self.removal.open_row().is_none(),
        };
        let wants = self.selecting.keys(ui, &code, inputs, &view);
        if let Some(offset) = wants.scroll_to {
            self.place.scroll_to = Some(offset);
        }
        if wants.comment && self.drafting.composer.is_none() {
            self.open_range_composer();
        }
    }

    /// Presses, drags and multi-clicks on the code (see [`Selecting::track_pointer`]).
    fn track_pointer(
        &mut self,
        ui: &Ui,
        background: &egui::Response,
        frame: &Frame<'_>,
        inputs: &DiffInputs<'_>,
    ) {
        let code = Code::new(&self.built, &self.offsets, self.sideways.offsets);
        if self.selecting.track_pointer(ui, background, frame, &code, inputs) {
            self.open_range_composer();
        }
    }

    /// Opens the comment on the selected lines, if the platform takes one there.
    fn open_range_composer(&mut self) {
        if let Some(comment) = self.selecting.take_comment() {
            self.open_composer(comment.anchor, comment.above, comment.focus_row);
        }
    }

    /// Opens a new comment the user asked for on the line of row `focus_row`, the one the pointer
    /// or keyboard is at: right above it when `above` (a range picked dragging up), else below.
    /// That line keeps its place on screen and the card scrolls fully into view (see
    /// [`Drafting::reveal_target`]), so the editor opens next to the pointer.
    fn open_composer(&mut self, anchor: ThreadAnchor, above: bool, focus_row: usize) {
        self.drafting.open(anchor, above, self.place.below_top(&self.offsets, focus_row));
    }

    #[cfg(any(test, feature = "test-support"))]
    /// The row showing these line numbers (head number first, as comments are placed).
    pub fn row_of(
        &self,
        diff: &review_git::FileDiff,
        old_line: Option<u32>,
        new_line: Option<u32>,
    ) -> usize {
        diff_rows::find_row(diff, &self.built.rows, old_line, new_line)
            .expect("a row shows the line")
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Where tests point the mouse: computed from the last frame's geometry, as a user would see it.
    fn probe_pane(&self, pane: Pane, row: usize) -> (Probe, Rect) {
        let probe = self.probe.expect("a frame was shown");
        let top = self.offsets[row];
        let rect = Rect::from_min_size(
            probe.origin + vec2(0.0, top),
            vec2(probe.width, self.offsets[row + 1] - top),
        );
        let (_, _, pane_rect) = pane_rects(rect, probe.single_pane, probe.left_width)
            .into_iter()
            .find(|(candidate, _, _)| *candidate == pane)
            .expect("pane");
        (probe, pane_rect)
    }

    #[cfg(test)]
    /// Screen position just inside character `column` of the line `pane` shows on `row`.
    pub(crate) fn text_point(&self, pane: Pane, row: usize, column: usize) -> egui::Pos2 {
        let (probe, rect) = self.probe_pane(pane, row);
        pos2(
            rect.left() + probe.text_inset + (column as f32 + 0.25) * probe.char_width,
            rect.top() + probe.line_height / 2.0,
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Where the new-comment card was painted in the last pass, if it was.
    pub fn composer_card(&self) -> Option<Rect> {
        self.probe.and_then(|probe| probe.composer_card)
    }

    #[cfg(any(test, feature = "test-support"))]
    /// How many code rows the last pass painted, for the tests that hold a view to what is on screen.
    pub fn painted_rows(&self) -> usize {
        self.probe.map_or(0, |probe| probe.painted_rows)
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Some lines show as picked: selected, or marked as the lines of the new comment.
    pub fn marks_lines(&self) -> bool {
        self.selecting.is_active()
            || self.built.ranges.iter().any(|range| range.owner == RangeOwner::Draft)
    }

    #[cfg(test)]
    /// Selects the text of every row of `pane`, as a drag from the first line to the end of the
    /// last would.
    pub(crate) fn select_all_text(&mut self, pane: Pane) {
        use cv_widgets::text_selection::{Caret, Unit};
        let last = self.built.rows.len().saturating_sub(1);
        self.selecting.selection =
            Some(Selection::span(pane, Unit::Text, Caret::new(0, 0), Caret::new(last, usize::MAX)));
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Where the code of the line `pane` shows on `row` is.
    pub fn text_area(&self, pane: Pane, row: usize) -> TextArea {
        let (probe, rect) = self.probe_pane(pane, row);
        let slot = usize::from(pane == Pane::Right);
        TextArea {
            rect: Rect::from_min_max(pos2(rect.left() + probe.text_inset, rect.top()), rect.max),
            char_width: probe.char_width,
            line_height: probe.line_height,
            wrap_columns: probe.wrap_columns.map(|columns| columns[slot]),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Screen position on the (first) line number of `row` in `pane`.
    pub fn gutter_point(&self, pane: Pane, row: usize) -> egui::Pos2 {
        let (probe, rect) = self.probe_pane(pane, row);
        pos2(rect.left() + probe.number_width / 2.0, rect.top() + probe.line_height / 2.0)
    }

    #[cfg(test)]
    /// The top of the view on the rows in the last frame.
    pub(crate) fn view_top(&self) -> f32 {
        self.place.top()
    }

    #[cfg(test)]
    /// Where row `row` starts, from the top of the first row.
    pub(crate) fn row_top(&self, row: usize) -> f32 {
        self.offsets[row]
    }

    #[cfg(any(test, feature = "test-support"))]
    /// The top of the view, each pane's sideways offset and the build of the rows, for tests that
    /// check an update left the view alone.
    pub fn kept_place(&self) -> (f32, [f32; 2], u64) {
        (self.place.top(), self.sideways.offsets, self.built.generation)
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Screen position of the comment button a hovered line shows, in the marker column left of the code.
    pub fn marker_point(&self, pane: Pane, row: usize) -> egui::Pos2 {
        let (probe, rect) = self.probe_pane(pane, row);
        pos2(rect.left() + probe.marker_center, rect.top() + probe.line_height / 2.0)
    }

    #[cfg(test)]
    pub(crate) fn range_rows(&self) -> &[cv_code::diff_rows::RangeRows] {
        &self.built.ranges
    }

    #[cfg(test)]
    pub fn occurrences(&self) -> Option<crate::find::text::OccurrenceRows> {
        let gaps = self.built.rows.iter().enumerate();
        let gaps = gaps.filter(|(_, row)| matches!(row.kind, diff_rows::RowKind::Gap(_)));
        let gap_rows = gaps.map(|(index, _)| index as u32).collect();
        let rows = self.find.occurrence_rows()?.to_vec();
        Some(crate::find::text::OccurrenceRows {
            rows,
            ticks: self.built.overview.occurrences.clone(),
            gap_rows,
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy)]
struct Probe {
    origin: egui::Pos2,
    width: f32,
    single_pane: bool,
    left_width: f32,
    text_inset: f32,
    number_width: f32,
    /// Center of the marker column (the comment button), from the pane's left edge.
    marker_center: f32,
    char_width: f32,
    line_height: f32,
    /// Columns per visual row of each pane's code when lines wrap.
    wrap_columns: Option<[usize; 2]>,
    /// Where the new-comment card was painted in this pass.
    composer_card: Option<Rect>,
    /// Code rows painted in this pass.
    painted_rows: usize,
}

#[cfg(any(test, feature = "test-support"))]
impl Probe {
    fn of(frame: &Frame<'_>, layout: Layout) -> Self {
        let two_numbers = layout.has_two_numbers();
        let unit = Rect::from_min_size(egui::Pos2::ZERO, vec2(1.0, 1.0));
        Self {
            origin: frame.origin,
            width: frame.width,
            single_pane: layout.is_single_pane(),
            left_width: frame.left_width,
            text_inset: frame.metrics.text_inset(two_numbers),
            number_width: frame.metrics.number_width,
            marker_center: frame.metrics.marker_rect(unit, two_numbers, 1.0).center().x,
            char_width: frame.metrics.char_width,
            line_height: frame.metrics.line_height,
            wrap_columns: frame
                .wrap_width
                .map(|widths| widths.map(|width| (width / frame.metrics.char_width) as usize)),
            composer_card: None,
            painted_rows: 0,
        }
    }
}

/// Where a line's code sits on screen, for tests that look at the pixels.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy)]
pub struct TextArea {
    /// From the left edge of the text to the pane's right edge, the row's full height.
    pub rect: Rect,
    pub char_width: f32,
    pub line_height: f32,
    /// Columns per visual row when the line wraps.
    pub wrap_columns: Option<usize>,
}
