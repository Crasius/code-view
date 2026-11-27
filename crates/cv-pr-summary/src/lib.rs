//! The folder summary of the Files tab, as Azure DevOps shows a folder: "3 changed files" over one
//! card per changed file below the folder, in tree order, each with its changed lines and a few
//! unchanged ones around them. Shown for the pull request's root while no file is open and for a
//! folder picked in the tree.
//!
//! Built for pull requests with thousands of files and hundreds of thousands of changed lines:
//!
//! - Two levels of virtualization. Every card's height is known up front (estimated from its line
//!   counts until its diff arrives, exact afterwards), their running total finds the cards on
//!   screen by binary search, and inside a card only the rows in view are painted.
//! - Diffs load lazily, through the same mirror and diff cache as the file view, for the cards in
//!   view and one screen around them: nearest first, a few at a time, none while the view flies
//!   by (a fling, a dragged scroll bar) or the window is in the background.
//! - A loaded diff that changes a card's height keeps the card at the top of the view in place.
//! - Bodies live in a bounded cache, so memory follows the screen; laid-out text is egui's,
//!   dropped by the frame that no longer draws it.
//!
//! A card's rows are the file view's diff view in its compact layout, so they are commented on as
//! in the file view: the same selection, "+" button, context menu, Alt+C, comment card and the
//! threads at their lines. Comment cards count into the card's height (a new one is measured in a
//! pass egui discards, so it shows in its final place at once), and a card with a comment being
//! written stays in the body cache.
//!
//! The cards follow the file view's layout choice: side by side, each change's deleted lines sit
//! left of the added ones (with the file view's draggable boundary between them), otherwise
//! inline; added and deleted files show whole in one pane either way. Every card's shape is known
//! for both layouts once its diff loaded, so switching rebuilds the index at once, and the line at
//! the top of the view stays there.
//!
//! The cards' rows are code surfaces: the code zoom (Ctrl+wheel over them, Ctrl+Plus/Minus/0
//! with keyboard focus inside them) changes their code size, not the app's, as in the file view,
//! and the line under the pointer stays put.
//!
//! With the header's Preview on, a Markdown file's card shows its rendered diff instead of its
//! lines ([`rendered`]): its height comes from its blocks as they are measured, kept like a code
//! card's, and commenting on a block opens the file's text diff at its lines, as in the file view.
#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while line counts are u32 and indices usize.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

mod bodies;
mod card;
mod code;
#[cfg(test)]
mod comments_check;
mod diffs;
mod header;
mod keys;
mod layout;
mod loads;
mod rendered;
#[cfg(test)]
mod rendered_check;
#[cfg(test)]
mod split_check;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod zoom_check;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use egui::{Rect, Ui, UiBuilder, pos2, vec2};
use review_core::{ChangeBreakdown, NewThread, ProviderKind, ReviewThread};
use review_git::{ChangedFile, DiffOptions, FileDiff};
use review_store::{DiffLayout, RenderedWidth};

use self::bodies::{BodyCache, CardBody};
use self::card::{Body, CardAction, CardView, Content};
use self::code::{CardCode, CodeOutput, ThreadsByPath};
use self::header::HeaderAction;
use self::layout::{CardIndex, Geometry, Shape, Shapes};
use self::loads::Loads;
use self::rendered::{RenderedCards, RenderedFrame};
use cv_base::theme::{self, style};
use cv_code::compaction::SUMMARY_CONTEXT;
use cv_code::diff_paint::Metrics;
use cv_pr_diff::diff_view::{self, DiffEvent, Embedded};
use cv_pr_rendered::markdown_diff::RenderedEvent;
use cv_pr_rendered::preview::{self, Loaded};
use cv_pr_threads::{ThreadEvent, ThreadsUi};
use cv_runtime::load::Load;
use cv_syntax::highlight::LineSpans;
use cv_widgets::change_breakdown;

/// Bodies built from cached diffs per frame, so a jump into cached cards never stalls a frame.
const MAX_BUILDS_PER_FRAME: usize = 8;
/// Space around the cards; the scroll bar keeps its own gutter on the right.
const MARGIN: f32 = 16.0;

/// Where the summary's diffs come from: the file view's diff cache and the git mirror.
pub trait DiffSource {
    /// A diff loaded before, without a request.
    fn cached(&mut self, file: &ChangedFile) -> Option<Arc<FileDiff>>;
    /// Starts loading `file`'s diff; `None` while there is nothing to load from yet.
    fn start(&mut self, file: &ChangedFile) -> Option<Load<Arc<FileDiff>>>;
    /// A diff the summary loaded, kept for the file view.
    fn store(&mut self, file: &ChangedFile, diff: Arc<FileDiff>);
    /// Starts loading the versions of the Markdown file `file` for its rendered card; `None` while
    /// there is nothing to load from yet.
    fn start_rendered(&mut self, file: &ChangedFile) -> Option<Load<Arc<Loaded>>>;
    /// The syntax colors by diff line of `file`'s diff, when they are all worked out already (the
    /// file view's, or the ones its load waited for).
    fn spans(&mut self, _file: &ChangedFile) -> Option<Arc<LineSpans>> {
        None
    }
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "six independent per-frame switches from the tab, each read on its own"
)]
pub struct SummaryInputs<'a> {
    pub files: &'a [ChangedFile],
    /// The cards: indices into `files` of the visible files below `folder`, in tree order.
    pub order: &'a [usize],
    /// Changes whenever `order` does, so the cards are rebuilt only then.
    pub order_version: u64,
    /// `""` for the pull request's root.
    pub folder: &'a str,
    /// What the diffs compare (base, head, how): another value starts over.
    pub compare: (&'a str, &'a str, DiffOptions),
    pub font_size: f32,
    pub thread_counts: &'a HashMap<String, usize>,
    /// The pull request's threads; each card shows the ones on its lines.
    pub threads: &'a [&'a ReviewThread],
    pub can_comment: bool,
    /// The platform takes comments on unchanged lines outside the hunks (GitHub does not).
    pub can_comment_outside_hunks: bool,
    /// The mirror is there. Never waits for the line counts (a download of every changed blob).
    pub can_load: bool,
    /// A category, search or viewed filter hides files.
    pub is_filtered: bool,
    /// The user's diff layout: side by side shows the cards split, the others inline.
    pub layout: DiffLayout,
    /// "Show comments in the diff", as in the file view.
    pub show_threads: bool,
    /// The header's Preview: Markdown cards show their rendered diff.
    pub preview: bool,
    pub platform: ProviderKind,
    /// The reading column of rendered Markdown, as the file view sets it.
    pub rendered_width: RenderedWidth,
}

impl SummaryInputs<'_> {
    fn is_split(&self) -> bool {
        self.layout == DiffLayout::SideBySide
    }

    /// `file`'s card shows its rendered diff: a Markdown file with content changes, Preview on.
    fn renders(&self, file: &ChangedFile) -> bool {
        self.preview
            && preview::is_markdown(&file.path)
            && !file.is_binary
            && !layout::is_unchanged(file)
    }
}

pub enum SummaryEvent {
    /// Open the file (index into the files) in the file view.
    View(usize),
    /// Show the summary of this folder (`""` for the root).
    OpenFolder(String),
    ClearFilters,
    /// Something done with a thread in a card: reply, resolve, edit.
    Thread(ThreadEvent),
    /// Post the new thread drafted under `key`.
    Create {
        key: String,
        thread: NewThread,
    },
    /// The header's layout choice: inline or side by side, as in the file view.
    SwitchLayout(DiffLayout),
    /// The user finished moving the boundary between the old and new side.
    SplitResized,
    /// The header's Preview, switched to this.
    SwitchPreview(bool),
    /// Something done in the rendered card of the file (index into the files): commenting on a
    /// block, showing its source, a thread's reply.
    Rendered {
        file: usize,
        event: RenderedEvent,
    },
}

#[derive(Clone, Copy)]
struct Slot {
    file: usize,
    shape: Shape,
}

/// What the last frame did, for the tests that hold the summary to O(screen) work.
#[cfg(test)]
#[derive(Debug, Default, Clone, Copy)]
pub struct FrameStats {
    pub cards_shown: usize,
    pub rows_painted: usize,
    pub wanted: usize,
}

pub struct FolderSummary {
    slots: Vec<Slot>,
    index: CardIndex,
    index_dirty: bool,
    geometry: Option<Geometry>,
    built: Option<(u64, String)>,
    compare: Option<(String, String, DiffOptions)>,
    /// Exact shapes (inline and side by side) of the cards whose diff was loaded, kept when their
    /// bodies are dropped.
    shapes: HashMap<String, Shapes>,
    /// Exact body heights of the cards laid out, comment cards included; kept when their bodies
    /// are dropped.
    heights: HashMap<String, f32>,
    /// The cards show side by side.
    split: bool,
    /// Where the user moved the boundary between the sides in a card this frame, and whether
    /// they let go of it.
    split_moved: Option<f32>,
    split_committed: bool,
    /// The cards' width in the last frame, to lay out their rows before the view scrolls.
    width: f32,
    /// The card whose rows hold the selection: one selection in the summary at a time.
    selected: Option<String>,
    /// Collapsed cards by path, for the whole session of the pull request.
    collapsed: HashSet<String>,
    /// Changes above the inline limit the user asked to see here.
    forced: HashSet<String>,
    bodies: BodyCache,
    loads: Loads,
    frame: u64,
    /// The visible part of the cards, in card coordinates (0 is the first card's top).
    viewport: Rect,
    scroll_to: Option<f32>,
    /// Scroll offset per folder, for coming back to where the user was.
    positions: HashMap<String, f32>,
    /// The card n/p moved to, while it is in view.
    current: Option<usize>,
    /// The cards' files and lines by kind of file, behind the header's line totals.
    breakdown: ChangeBreakdown,
    /// The rendered cards of Markdown files, while Preview is on.
    rendered: RenderedCards,
    /// Cards of Markdown files, for the header's Preview.
    markdown: usize,
    #[cfg(test)]
    stats: FrameStats,
    /// Where the cards showed on screen in the last pass: the pointer's place among them.
    screen: Rect,
}

impl Default for FolderSummary {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            index: CardIndex::default(),
            index_dirty: true,
            geometry: None,
            built: None,
            compare: None,
            shapes: HashMap::new(),
            heights: HashMap::new(),
            split: false,
            split_moved: None,
            split_committed: false,
            width: 0.0,
            selected: None,
            collapsed: HashSet::new(),
            forced: HashSet::new(),
            bodies: BodyCache::default(),
            loads: Loads::default(),
            frame: 0,
            viewport: Rect::from_min_size(pos2(0.0, 0.0), vec2(0.0, 0.0)),
            scroll_to: None,
            positions: HashMap::new(),
            current: None,
            breakdown: ChangeBreakdown::default(),
            rendered: RenderedCards::default(),
            markdown: 0,
            #[cfg(test)]
            stats: FrameStats::default(),
            screen: Rect::NOTHING,
        }
    }
}

impl FolderSummary {
    /// The summary is not on screen (a file is open): its queued coloring makes way.
    pub fn hide(&mut self) {
        self.bodies.cancel_coloring();
        self.loads.forget_motion();
    }

    #[cfg(test)]
    pub fn stats(&self) -> FrameStats {
        self.stats
    }

    #[cfg(test)]
    pub fn bodies_kept(&self) -> usize {
        self.bodies.len()
    }

    /// `split` is the old side's share of a side-by-side card, the file view's; the user can drag it
    /// in any card.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        inputs: &SummaryInputs<'_>,
        source: &mut dyn DiffSource,
        threads_ui: &mut ThreadsUi,
        split: &mut f32,
    ) -> Option<SummaryEvent> {
        self.frame += 1;
        #[cfg(test)]
        {
            self.stats = FrameStats::default();
        }
        self.follow_compare(inputs.compare);
        self.loads.receive_spans(&mut self.bodies);
        diffs::finish_loads(self, inputs, source);
        if self.rendered.poll() {
            self.index_dirty = true;
        }
        self.rebuild_if_needed(inputs);
        let geometry = self.follow_geometry(ui, inputs.font_size);

        let mut event = self.header(ui, inputs);
        // A layout picked in the header shows in this frame already; the tab saves it.
        let layout_event = event.take_if(|event| matches!(event, SummaryEvent::SwitchLayout(_)));
        let is_split = match &layout_event {
            Some(SummaryEvent::SwitchLayout(layout)) => *layout == DiffLayout::SideBySide,
            _ => inputs.is_split(),
        };
        self.follow_layout(inputs, is_split);
        rendered::follow_preview(self, inputs);
        if self.slots.is_empty() {
            return layout_event.or(event).or_else(|| header::empty_state(ui, inputs));
        }
        let threads = code::threads_by_path(inputs.threads);
        let requests = self.lay_out_bodies(ui, inputs, &threads, threads_ui, &geometry, *split);
        let requests =
            [requests, rendered::lay_out_rendered(self, ui, inputs, &threads, &geometry, *split)]
                .concat();
        if self.index_dirty {
            self.rebuild_index(inputs, &geometry);
        }
        // A card's rows asked to be scrolled to (a new comment card shown in full): that beats
        // keeping the top card in place.
        if let Some(&(card, y)) = requests.last() {
            self.scroll_to =
                Some(self.clamp_offset(self.index.top(card) + geometry.header + y + MARGIN));
        }
        if let Some(keyboard) = keys::apply(self, ui, &geometry) {
            event = Some(keyboard);
        }
        if let Some(cards_event) =
            self.show_cards(ui, inputs, &threads, threads_ui, &geometry, split)
        {
            event = Some(cards_event);
        }
        let now = ui.input(|input| input.time);
        let is_active = ui.input(|input| cv_base::platform::is_window_active(input.viewport()));
        diffs::plan_wanted(self, inputs);
        diffs::start_loads(self, ui.ctx(), inputs, source, now, is_active);
        diffs::color_wanted(self, ui.ctx(), inputs);
        layout_event.or(event)
    }

    /// The line height of the code size: a new one lays out every card again.
    fn follow_geometry(&mut self, ui: &Ui, font_size: f32) -> Geometry {
        let line_height = Metrics::new(ui, font_size, 1).line_height;
        let geometry = Geometry::new(line_height);
        if self.geometry != Some(geometry) {
            self.geometry = Some(geometry);
            self.heights.clear();
            self.index_dirty = true;
        }
        geometry
    }

    /// The scrolled cards; notes where the view went and how fast it moves. Returns what was done
    /// in a card, else the end of a drag of a card's side boundary.
    fn show_cards(
        &mut self,
        ui: &mut Ui,
        inputs: &SummaryInputs<'_>,
        threads: &ThreadsByPath<'_>,
        threads_ui: &mut ThreadsUi,
        geometry: &Geometry,
        split: &mut f32,
    ) -> Option<SummaryEvent> {
        let area = ui.available_rect_before_wrap();
        let palette = theme::palette(ui.ctx());
        ui.painter().rect_filled(area, 0, palette.ground);
        let margin = egui::Margin {
            left: MARGIN as i8,
            right: (MARGIN + style::SCROLL_GUTTER) as i8,
            top: MARGIN as i8,
            bottom: MARGIN as i8,
        };
        // A drag on the code selects it, as in the file view; it does not scroll.
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt(("folder_summary", inputs.folder))
            .auto_shrink(false)
            .scroll_source(
                egui::scroll_area::ScrollSource::SCROLL_BAR
                    | egui::scroll_area::ScrollSource::MOUSE_WHEEL,
            )
            .content_margin(margin);
        if let Some(offset) = self.scroll_to.take() {
            scroll = scroll.vertical_scroll_offset(offset);
        }
        let total = self.index.total();
        let output = ui
            .scope_builder(UiBuilder::new().max_rect(area), |ui| {
                scroll.show_viewport(ui, |ui, viewport| {
                    ui.set_min_height(total);
                    let frame = PaintFrame {
                        inputs,
                        threads,
                        geometry,
                        origin: ui.max_rect().min,
                        width: ui.available_width(),
                        // The viewport counts the top margin; cards start below it.
                        viewport: viewport.translate(vec2(0.0, -MARGIN)),
                        split: *split,
                    };
                    self.paint_cards(ui, &frame, threads_ui)
                })
            })
            .inner;
        if let Some(moved) = self.split_moved.take() {
            *split = moved;
            ui.ctx().request_repaint();
        }
        let resized =
            std::mem::take(&mut self.split_committed).then_some(SummaryEvent::SplitResized);
        let offset = output.state.clamped_offset().y;
        self.viewport = Rect::from_min_size(pos2(0.0, offset - MARGIN), output.inner_rect.size());
        self.screen = output.inner_rect;
        self.positions.insert(inputs.folder.to_owned(), offset);
        self.forget_current_out_of_view();
        let now = ui.input(|input| input.time);
        self.loads.track_motion(ui.ctx(), now, offset);
        output.inner.or(resized)
    }

    /// The user's layout changed (or is known for the first time): every card takes its shape for
    /// it, from its diff where that was loaded. Heights measured for the other layout's widths are
    /// laid out again; the index keeps the card at the top of the view in place, and its rows keep
    /// the line at the top (see [`DiffView`]'s scroll request).
    fn follow_layout(&mut self, inputs: &SummaryInputs<'_>, is_split: bool) {
        if self.split == is_split {
            return;
        }
        self.split = is_split;
        let context = self.context();
        for slot in &mut self.slots {
            slot.shape = layout::shape_of(
                &self.shapes,
                &inputs.files[slot.file],
                context,
                &self.forced,
                is_split,
            );
        }
        self.heights.clear();
        self.index_dirty = true;
    }

    /// A new comparison (live update, another range, other context) makes every diff stale.
    fn follow_compare(&mut self, compare: (&str, &str, DiffOptions)) {
        let is_same = self.compare.as_ref().is_some_and(|(base, head, options)| {
            (base.as_str(), head.as_str(), *options) == compare
        });
        if is_same {
            return;
        }
        self.compare = Some((compare.0.to_owned(), compare.1.to_owned(), compare.2));
        self.loads.start_over();
        self.bodies.clear();
        self.rendered.clear();
        self.shapes.clear();
        self.heights.clear();
        self.selected = None;
        // Keeps the folder: only the shapes are rebuilt, and the anchor keeps the view in place.
        if let Some((version, _)) = &mut self.built {
            *version = u64::MAX;
        }
    }

    fn context(&self) -> u32 {
        SUMMARY_CONTEXT
            .min(self.compare.as_ref().map_or(SUMMARY_CONTEXT, |(_, _, options)| options.context))
    }

    fn rebuild_if_needed(&mut self, inputs: &SummaryInputs<'_>) {
        let key = (inputs.order_version, inputs.folder.to_owned());
        if self.built.as_ref() == Some(&key) {
            return;
        }
        let is_new_folder = self.built.as_ref().is_none_or(|(_, folder)| *folder != key.1);
        self.built = Some(key);
        let context = self.context();
        self.slots = inputs
            .order
            .iter()
            .map(|&file| Slot {
                file,
                shape: layout::shape_of(
                    &self.shapes,
                    &inputs.files[file],
                    context,
                    &self.forced,
                    self.split,
                ),
            })
            .collect();
        self.breakdown =
            change_breakdown::tally(inputs.order.iter().map(|&file| &inputs.files[file]));
        self.markdown = inputs
            .order
            .iter()
            .filter(|&&file| preview::is_markdown(&inputs.files[file].path))
            .count();
        self.index_dirty = true;
        self.current = None;
        self.loads.clear_wanted();
        if is_new_folder {
            let offset = self.positions.get(inputs.folder).copied().unwrap_or(0.0);
            self.scroll_to = Some(offset);
            self.viewport = Rect::from_min_size(pos2(0.0, offset - MARGIN), self.viewport.size());
            self.loads.forget_motion();
        }
    }

    fn slot_height(&self, inputs: &SummaryInputs<'_>, geometry: &Geometry, slot: &Slot) -> f32 {
        let path = &inputs.files[slot.file].path;
        if self.collapsed.contains(path) {
            return geometry.slot(slot.shape, true);
        }
        match self.heights.get(path) {
            Some(body) => geometry.header + body + geometry.gap,
            None => geometry.slot(slot.shape, false),
        }
    }

    /// `offset` within the scroll range of the cards.
    fn clamp_offset(&self, offset: f32) -> f32 {
        let limit = (self.index.total() + 2.0 * MARGIN - self.viewport.height()).max(0.0);
        offset.clamp(0.0, limit)
    }

    /// Lays out the rows of the loaded cards near the view before the view scrolls, as in the
    /// last frame's place: their heights (comment cards included) go into the index, and where
    /// their rows ask the view to scroll comes back as (card, y below the card's body top).
    fn lay_out_bodies(
        &mut self,
        ui: &Ui,
        inputs: &SummaryInputs<'_>,
        threads: &ThreadsByPath<'_>,
        threads_ui: &mut ThreadsUi,
        geometry: &Geometry,
        split: f32,
    ) -> Vec<(usize, f32)> {
        let mut requests = Vec::new();
        if self.width <= 0.0 || self.index.len() != self.slots.len() {
            return requests;
        }
        let context = self.context();
        let is_split = self.split;
        // A new code size keeps the line under the pointer in place, as in the file view.
        let keep_below = diff_view::keep_below(ui.ctx(), self.screen);
        for &card in self.loads.wanted() {
            let file = &inputs.files[self.slots[card].file];
            if self.collapsed.contains(&file.path) || inputs.renders(file) {
                continue;
            }
            let Some(body) =
                self.bodies.get_mut(&file.path).filter(|body| body.placeholder.is_none())
            else {
                continue;
            };
            let embedded = Embedded {
                origin: pos2(0.0, self.index.top(card) + geometry.header),
                width: self.width,
                view: self.viewport,
                split,
                keep_below,
            };
            let CardBody { diff, view, spans, .. } = body;
            let diff_inputs = code::card_inputs(
                diff,
                spans.as_deref(),
                file,
                inputs,
                code::threads_of(threads, file),
                code::card_layout(file, context, is_split),
            );
            view.prepare_embedded(ui, &diff_inputs, threads_ui, &embedded);
            if let Some(y) = view.take_scroll_request() {
                requests.push((card, y));
            }
            let height = view.height();
            if self
                .heights
                .insert(file.path.clone(), height)
                .is_none_or(|previous| (previous - height).abs() > 0.5)
            {
                self.index_dirty = true;
            }
        }
        requests
    }

    /// New running totals; the card at the top of the view stays where it is on screen.
    fn rebuild_index(&mut self, inputs: &SummaryInputs<'_>, geometry: &Geometry) {
        let top = self.viewport.top();
        let anchor = (top > 0.0 && self.index.len() == self.slots.len())
            .then(|| self.index.at(top))
            .flatten()
            .map(|card| (card, self.index.top(card), top - self.index.top(card)));
        let heights: Vec<f32> =
            self.slots.iter().map(|slot| self.slot_height(inputs, geometry, slot)).collect();
        self.index.rebuild(heights.into_iter());
        self.index_dirty = false;
        if let Some((card, old_top, below)) = anchor
            && self.scroll_to.is_none()
        {
            let new_top = self.index.top(card);
            if (new_top - old_top).abs() > 0.5 {
                let height = self.index.bottom(card) - new_top;
                self.scroll_to = Some(new_top + below.min(height) + MARGIN);
            }
        }
    }

    /// "3 changed files", the line totals and the folder as a trail of links, over the cards.
    fn header(&mut self, ui: &mut Ui, inputs: &SummaryInputs<'_>) -> Option<SummaryEvent> {
        let paths = || inputs.order.iter().map(|&file| &inputs.files[file].path);
        let all_collapsed =
            !self.slots.is_empty() && paths().all(|path| self.collapsed.contains(path));
        let counts = header::Counts {
            cards: self.slots.len(),
            breakdown: &self.breakdown,
            all_collapsed,
            markdown: self.markdown,
        };
        match header::show(ui, inputs, &counts)? {
            HeaderAction::Event(event) => Some(event),
            HeaderAction::ToggleCollapseAll => {
                if all_collapsed {
                    for path in paths() {
                        self.collapsed.remove(path);
                    }
                } else {
                    self.collapsed.extend(paths().cloned());
                }
                self.index_dirty = true;
                None
            }
        }
    }

    fn forget_current_out_of_view(&mut self) {
        let Some(current) = self.current else {
            return;
        };
        let is_visible = self.index.bottom(current) > self.viewport.top()
            && self.index.top(current) < self.viewport.bottom();
        if !is_visible && self.scroll_to.is_none() {
            self.current = None;
        }
    }

    fn paint_cards(
        &mut self,
        ui: &mut Ui,
        frame: &PaintFrame<'_>,
        threads_ui: &mut ThreadsUi,
    ) -> Option<SummaryEvent> {
        self.width = frame.width;
        let mut pass = Pass::default();
        let cards = self.index.overlapping(frame.viewport.top(), frame.viewport.bottom());
        #[cfg(test)]
        let shown = cards.len();
        for card in cards {
            self.paint_card(ui, frame, threads_ui, card, &mut pass);
        }
        #[cfg(test)]
        {
            self.stats = FrameStats {
                cards_shown: shown,
                rows_painted: pass.rows,
                wanted: self.stats.wanted,
            };
        }
        self.finish_pass(ui.ctx(), pass.newly_selected, pass.is_measured);
        pass.event
    }

    fn paint_card(
        &mut self,
        ui: &mut Ui,
        frame: &PaintFrame<'_>,
        threads_ui: &mut ThreadsUi,
        card: usize,
        pass: &mut Pass,
    ) {
        let (geometry, inputs) = (frame.geometry, frame.inputs);
        let slot = self.slots[card];
        let file = &inputs.files[slot.file];
        let top = self.index.top(card);
        let rect = Rect::from_min_size(
            frame.origin + vec2(0.0, top),
            vec2(frame.width, self.index.bottom(card) - top - geometry.gap),
        );
        let code = CardCode {
            file,
            inputs,
            threads: code::threads_of(frame.threads, file),
            layout: code::card_layout(file, self.context(), self.split),
            split: frame.split,
        };
        let rendered = RenderedFrame {
            file,
            inputs,
            threads: code.threads,
            is_split: self.split,
            split: frame.split,
        };
        // Owned, so the card can borrow the rendered cards to show one.
        let rendered_failure = self.rendered.failure(&file.path).map(str::to_owned);
        let body = if inputs.renders(file) {
            let failure = rendered_failure.as_deref();
            match self.rendered.show(&file.path, self.frame) {
                Some(card) => Body::Rendered(card),
                // A rendered card has no inline limit: only its blocks in view are laid out.
                None => card::placeholder_body(file, true, failure),
            }
        } else {
            let failure = self.loads.failure(&file.path);
            match self.bodies.show(&file.path, self.frame) {
                Some(body) => Body::Loaded(body),
                None => card::placeholder_body(file, self.forced.contains(&file.path), failure),
            }
        };
        let view = CardView {
            file,
            rect,
            shape: slot.shape,
            is_collapsed: self.collapsed.contains(&file.path),
            is_current: self.current == Some(card),
            comments: inputs.thread_counts.get(&file.path).copied().unwrap_or(0),
            body,
            geometry,
        };
        let mut code_output = None;
        let mut rows = |ui: &mut Ui, body: Rect, clip: Rect, content: Content<'_>| {
            code_output = Some(match content {
                Content::Code(card_body) => {
                    code::show(ui, body, clip, card_body, &code, threads_ui)
                }
                Content::Rendered(card) => card.show(ui, body, clip, &rendered, threads_ui),
            });
        };
        let action = card::show(ui, view, &mut rows);
        if let Some(output) = code_output {
            let laid_out = rect.height() - geometry.header;
            self.take_code_output(
                ui.ctx(),
                (slot.file, &file.path),
                top + geometry.header,
                laid_out,
                output,
                pass,
            );
        }
        if let Some(event) = action.and_then(|action| self.apply_action(action, card, file)) {
            pass.event = Some(event);
        }
    }

    /// Takes in what the rows of the card of `file` (its index and path) did: their height
    /// (`laid_out` was the one the index had), a scroll they asked for below `body_top`, a new
    /// selection, a boundary drag, a thread event, what was done in a rendered card.
    fn take_code_output(
        &mut self,
        ctx: &egui::Context,
        (file, path): (usize, &str),
        body_top: f32,
        laid_out: f32,
        output: CodeOutput,
        pass: &mut Pass,
    ) {
        #[cfg(test)]
        {
            pass.rows += output.rows;
        }
        self.heights.insert(path.to_owned(), output.height);
        if (output.height - laid_out).abs() > 0.5 {
            // Comment cards were measured: the index changes, and this pass is redone.
            self.index_dirty = true;
            pass.is_measured = true;
        }
        if let Some(y) = output.scroll {
            self.scroll_to = Some(self.clamp_offset(body_top + y + MARGIN));
            ctx.request_repaint();
        }
        if output.has_selection && self.selected.as_deref() != Some(path) {
            pass.newly_selected = Some(path.to_owned());
        }
        self.split_moved = output.split.or(self.split_moved);
        self.split_committed |= output.split_committed;
        match output.event {
            Some(DiffEvent::Thread(thread_event)) => {
                pass.event = Some(SummaryEvent::Thread(thread_event));
            }
            Some(DiffEvent::Create { key, thread }) => {
                pass.event = Some(SummaryEvent::Create { key, thread });
            }
            Some(
                DiffEvent::ColorAnyway
                | DiffEvent::RetryContents
                | DiffEvent::SwitchLayout(_)
                | DiffEvent::SplitResized,
            )
            | None => {}
        }
        if let Some(event) = output.rendered {
            pass.event = Some(SummaryEvent::Rendered { file, event });
        }
    }

    /// Does what the user did with a card's header or placeholder; opening the file is the tab's.
    fn apply_action(
        &mut self,
        action: CardAction,
        card: usize,
        file: &ChangedFile,
    ) -> Option<SummaryEvent> {
        match action {
            CardAction::Toggle => {
                if !self.collapsed.remove(&file.path) {
                    self.collapsed.insert(file.path.clone());
                }
                self.current = Some(card);
                self.index_dirty = true;
            }
            CardAction::View => return Some(SummaryEvent::View(self.slots[card].file)),
            CardAction::Retry => {
                self.loads.retry(&file.path);
                self.rendered.retry(&file.path);
            }
            CardAction::ShowHere => {
                self.forced.insert(file.path.clone());
                let measured = self.bodies.take_too_large(&file.path);
                self.slots[card].shape = layout::shown_here(file, measured);
                self.index_dirty = true;
            }
        }
        None
    }

    /// After the cards: a selection started in one card ends the one in another, and a pass
    /// that measured comment cards is redone (or the next frame lays them out).
    fn finish_pass(
        &mut self,
        ctx: &egui::Context,
        newly_selected: Option<String>,
        is_measured: bool,
    ) {
        if let Some(path) = newly_selected
            && let Some(previous) = self.selected.replace(path)
            && let Some(body) = self.bodies.get_mut(&previous)
        {
            body.view.clear_selection();
        }
        if is_measured
            && ctx.current_pass_index() + 1 < ctx.options(|options| options.max_passes.get())
        {
            ctx.request_discard("a card's comment cards were measured");
        } else if self.index_dirty {
            ctx.request_repaint();
        }
    }
}

/// Where the cards of this frame go: `viewport` is the visible part in card coordinates.
struct PaintFrame<'a> {
    inputs: &'a SummaryInputs<'a>,
    threads: &'a ThreadsByPath<'a>,
    geometry: &'a Geometry,
    origin: egui::Pos2,
    width: f32,
    viewport: Rect,
    /// The old side's share of a side-by-side card.
    split: f32,
}

/// What painting the cards of one pass gathered.
#[derive(Default)]
struct Pass {
    event: Option<SummaryEvent>,
    /// Code rows painted.
    #[cfg(test)]
    rows: usize,
    /// The card a selection started in this pass.
    newly_selected: Option<String>,
    /// A card's comment cards were measured: the index changed under this pass.
    is_measured: bool,
}
