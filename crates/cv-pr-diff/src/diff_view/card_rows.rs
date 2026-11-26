//! The comment card rows of the text diff: a thread's card under its side's code, and the new
//! comment's card ([`Drafting`]) from the moment it opens until the posted thread takes its place.
//!
//! A card just opened is laid out once in a pass that is discarded, so its height is known before
//! any frame shows it; the next pass of the same frame scrolls it fully into view and paints it in
//! its final place (see [`Drafting::reveal_target`]). While comments are hidden in the diff, the
//! cards the user showed one by one stay ([`Revealed`]).

use std::collections::HashSet;

use egui::{Rect, Ui, pos2, vec2};
use review_core::{ReviewThread, ThreadAnchor};
use review_git::FileDiff;

use super::cards::{CARD_GAP, CARD_MAX_WIDTH, CardHeights, DEFAULT_CARD_HEIGHT, measure_card};
use super::offsets::RowOffsets;
use super::{DiffEvent, DiffInputs, Frame};
use crate::diff_composer::{self, Composer, ComposerOutcome};
use cv_code::diff_rows::{self, Layout, Row, RowKind};
use cv_pr_threads::{self as threads, CardOptions, ThreadEvent, ThreadsUi};

/// The key of the new comment's card among the card heights (thread ids are never empty).
pub(super) const COMPOSER_KEY: &str = "\u{0}composer";

/// A composer being scrolled into view after it opened.
#[derive(Debug, Clone, Copy)]
struct ComposerReveal {
    /// (base, head) line numbers of the line the pointer or keyboard was at.
    focus: (Option<u32>, Option<u32>),
    /// How far below the top of the view that line was when the composer opened.
    below_top: f32,
    /// The card was laid out once, so its height is measured, not estimated.
    is_measured: bool,
}

/// The new comment being written in the diff, if any.
#[derive(Default)]
pub(super) struct Drafting {
    pub composer: Option<Composer>,
    /// A composer the user just opened is being scrolled fully into view.
    reveal: Option<ComposerReveal>,
    /// The comment card moved from above its lines to below them as it was posted: put it back at
    /// this distance below the top of the view once the rows are rebuilt.
    keep_below_top: Option<f32>,
    /// A kept new-thread draft of this file was looked for (once per file).
    is_restored: bool,
}

/// Where the new comment's card row is: `rect`, and how far below the top of the view it starts.
pub(super) struct CardRow {
    pub rect: Rect,
    pub below_top: Option<f32>,
}

/// What showing the new comment's card gave.
#[derive(Default)]
pub(super) struct CardShown {
    pub event: Option<DiffEvent>,
    /// The card's row height changed: the rows below move.
    pub moved: bool,
    /// Where the card was painted, for tests that watch it frame by frame.
    #[cfg(any(test, feature = "test-support"))]
    pub card: Option<Rect>,
}

/// The thread cards shown although comments are hidden in the diff: the user clicked their count
/// or jumped to them.
#[derive(Default)]
pub(super) struct Revealed {
    ids: HashSet<String>,
    /// Whether comments showed in the diff last frame.
    showed_threads: Option<bool>,
}

impl Revealed {
    /// The card of `thread` is shown although comments are hidden.
    pub fn contains(&self, thread: &ReviewThread) -> bool {
        self.ids.contains(&thread.id)
    }

    /// The card of `thread` is left out: comments are hidden in the diff (`show_threads` is off)
    /// and the user did not show it.
    pub fn hides(&self, show_threads: bool, thread: &ReviewThread) -> bool {
        !show_threads && !self.contains(thread)
    }

    /// Shows the card of thread `thread_id`; false when it showed already.
    pub fn insert(&mut self, thread_id: &str) -> bool {
        self.ids.insert(thread_id.to_owned())
    }

    /// Shows the cards of `threads`, or hides them again.
    pub fn set<'a>(&mut self, threads: impl IntoIterator<Item = &'a ReviewThread>, show: bool) {
        for thread in threads {
            if show {
                self.ids.insert(thread.id.clone());
            } else {
                self.ids.remove(&thread.id);
            }
        }
    }

    /// Comments are on or off in the diff (`show_threads`); after a switch, which this tells, the
    /// cards shown one by one go back under the switch.
    pub fn follow(&mut self, show_threads: bool) -> bool {
        let showed = self.showed_threads.replace(show_threads);
        if showed.is_none_or(|showed| showed == show_threads) {
            return false;
        }
        self.ids.clear();
        true
    }
}

/// The height the card row `kind` is laid out with: a thread's card as measured in `cards`
/// (nothing while `revealed` leaves it out), the new comment's as measured or `unmeasured` while
/// the pass that measures it runs.
pub(super) fn row_height(
    kind: RowKind,
    inputs: &DiffInputs<'_>,
    revealed: &Revealed,
    cards: &CardHeights,
    unmeasured: Option<f32>,
) -> f32 {
    match kind {
        RowKind::Thread(thread) => {
            let thread = inputs.threads[thread as usize];
            if revealed.hides(inputs.show_threads, thread) {
                0.0
            } else {
                cards.shown(&thread.id).unwrap_or(DEFAULT_CARD_HEIGHT)
            }
        }
        _ => unmeasured.or_else(|| cards.shown(COMPOSER_KEY)).unwrap_or(DEFAULT_CARD_HEIGHT),
    }
}

/// The rows the new comment's card is placed among.
pub(super) struct Placed<'a> {
    pub rows: &'a [Row],
    /// The card's row.
    pub card: Option<usize>,
    pub offsets: &'a RowOffsets,
}

impl Drafting {
    /// A card just opened was not laid out yet: the pass laying it out measures it and is
    /// discarded.
    pub fn is_measuring(&self) -> bool {
        self.reveal.is_some_and(|reveal| !reveal.is_measured)
    }

    /// This pass leaves a card just opened out of the rows: it could not be discarded after
    /// measuring it (see [`is_last_pass`]).
    pub fn holds_card(&self, ctx: &egui::Context) -> bool {
        self.is_measuring() && is_last_pass(ctx)
    }

    /// Until a new card is measured it counts as taller than the view: the pass that measures it
    /// then scrolls within content at least as tall as the final one, never past its end.
    pub fn unmeasured_height(&self, view_height: f32) -> Option<f32> {
        self.is_measuring().then(|| (view_height * 2.0).max(DEFAULT_CARD_HEIGHT))
    }

    /// Opens the composer of a new-thread draft kept for the file at `path`, once per file.
    pub fn restore(&mut self, path: &str, threads_ui: &ThreadsUi) {
        if self.is_restored || self.composer.is_some() {
            return;
        }
        self.is_restored = true;
        if let Some(anchor) = threads_ui.new_thread_drafts(path).into_iter().next() {
            self.composer = Some(Composer::restored(anchor));
        }
    }

    /// The new thread the composer posted is in the loaded `threads`: its card takes the
    /// composer's place in the rows built next, with the height (and any ease) the composer's card
    /// had in `cards`, so nothing below it moves. Without a match (the platform renamed it) the
    /// card is measured in a discarded pass as any new card is.
    pub fn hand_over_landed(
        &mut self,
        threads: &[&ReviewThread],
        threads_ui: &ThreadsUi,
        cards: &mut CardHeights,
    ) {
        let Some(composer) =
            self.composer.as_ref().filter(|composer| composer.has_landed(threads_ui))
        else {
            return;
        };
        if let Some(thread_id) = threads_ui
            .landed_thread(composer.key())
            .filter(|id| threads.iter().any(|thread| thread.id == *id))
        {
            cards.hand_over(COMPOSER_KEY, thread_id);
        }
        self.composer = None;
    }

    /// Opens a new comment on `anchor`'s lines: right above the line the pointer or keyboard is at
    /// when `above` (a range picked dragging up), else below. That line, `below_top` below the
    /// top of the view, keeps its place on screen and the card scrolls fully into view.
    pub fn open(&mut self, anchor: ThreadAnchor, above: bool, below_top: f32) {
        let composer = Composer::new(anchor, above);
        let focus = match (&composer.anchor.range, composer.anchor.above) {
            (Some(range), true) => (range.start_old_line, range.start_new_line),
            _ => (composer.anchor.old_line, composer.anchor.new_line),
        };
        self.composer = Some(composer);
        self.reveal = Some(ComposerReveal { focus, below_top, is_measured: false });
    }

    /// How far below the top of the view the card posted from above its lines goes back to, once.
    pub fn take_keep(&mut self) -> Option<f32> {
        self.keep_below_top.take()
    }

    /// Where to scroll a composer the user just opened so its whole card is in view, moving the
    /// diff as little as possible: the line the pointer is at keeps its place on screen unless the
    /// card would then not fit, and a card taller than the view shows its top. A card opened above
    /// that line pushes the content above it up, not the line down. `view_top` and `view_height`
    /// are the view's; `is_embedded`: another view scrolls the rows and clamps the offset.
    ///
    /// The first pass counts the card taller than the view (see [`Self::unmeasured_height`]), so
    /// the scroll area's extent from it covers the final scroll offset: past that extent the
    /// scroll area would spring back over the next frames.
    pub fn reveal_target(
        &mut self,
        placed: &Placed<'_>,
        diff: &FileDiff,
        view_top: f32,
        view_height: f32,
        is_embedded: bool,
    ) -> Option<f32> {
        let reveal = self.reveal?;
        if self.composer.is_none() {
            self.reveal = None;
            return None;
        }
        let offsets = placed.offsets;
        let card = placed
            .card
            .filter(|_| reveal.is_measured && offsets.is_placed(placed.rows.len()))
            .filter(|_| view_height > 0.0)?;
        let (top, bottom) = (offsets[card], offsets[card + 1]);
        let keep_line = diff_rows::find_row(diff, placed.rows, reveal.focus.0, reveal.focus.1)
            .map_or(view_top, |row| offsets[row] - reveal.below_top);
        let target = keep_line.max(bottom - view_height).min(top);
        let limit = (offsets.height() - view_height).max(0.0);
        Some(if is_embedded { target } else { target.clamp(0.0, limit) })
    }

    /// The new comment's card on a band across `row.rect`, under its side's code.
    pub fn show_card(
        &mut self,
        ui: &mut Ui,
        row: &CardRow,
        inputs: &DiffInputs<'_>,
        frame: &Frame<'_>,
        threads_ui: &mut ThreadsUi,
        cards: &mut CardHeights,
    ) -> CardShown {
        let Some(composer) = self.composer.as_mut() else {
            return CardShown::default();
        };
        let was_above = composer.anchor.above;
        let card = card_rect(row.rect, inputs.layout, composer.is_right(), frame);
        let is_easing = cards.is_easing(COMPOSER_KEY);
        let (height, outcome) =
            measure_card(ui, row.rect, card, is_easing, |ui| composer.show(ui, threads_ui));
        let is_settling = threads_ui.is_settling(composer.key());
        if was_above && !composer.anchor.above {
            // Posted from above its lines: the card goes under them and stays where it is on
            // screen, the lines moving up above it.
            self.keep_below_top = row.below_top;
        }
        let mut moved = cards.store(COMPOSER_KEY, height, is_settling);
        if self.reveal.is_none() {
            cards.redo_unless_shown(ui, row.rect, COMPOSER_KEY, is_easing);
        }
        match self.reveal.as_mut() {
            // The pass that measured the new card is not shown; the next one places it.
            Some(reveal) if !reveal.is_measured => {
                reveal.is_measured = true;
                moved = true;
                ui.ctx().request_discard("a new comment card was measured");
            }
            Some(_) => self.reveal = None,
            None => {}
        }
        let event = match outcome {
            ComposerOutcome::Open => None,
            ComposerOutcome::Close => {
                self.composer = None;
                None
            }
            ComposerOutcome::Create { key, thread } => Some(DiffEvent::Create { key, thread }),
        };
        CardShown {
            event,
            moved,
            #[cfg(any(test, feature = "test-support"))]
            card: Some(Rect::from_min_size(card.min, vec2(card.width(), height - CARD_GAP * 2.0))),
        }
    }
}

/// The card of `thread` on a band across `rect`, under its side's code. True with it when its
/// row height changed and the rows below move.
pub(super) fn show_thread_card(
    ui: &mut Ui,
    rect: Rect,
    thread: &ReviewThread,
    layout: Layout,
    frame: &Frame<'_>,
    threads_ui: &mut ThreadsUi,
    cards: &mut CardHeights,
) -> (Option<ThreadEvent>, bool) {
    let is_right = thread.anchor.as_ref().is_some_and(diff_composer::is_right);
    let card = card_rect(rect, layout, is_right, frame);
    let options = CardOptions { show_location: false };
    let is_easing = cards.is_easing(&thread.id);
    let (height, thread_event) = measure_card(ui, rect, card, is_easing, |ui| {
        threads::show_card(ui, thread, threads_ui, &options)
    });
    let moved = cards.store(&thread.id, height, threads_ui.is_settling(&thread.id));
    cards.redo_unless_shown(ui, rect, &thread.id, is_easing);
    (thread_event, moved)
}

/// The narrowest a card gets, and the room it leaves at the end of its pane.
const CARD_MIN_WIDTH: f32 = 240.0;
const CARD_END_GAP: f32 = 24.0;

/// Thread cards sit under their side's code: the right pane for head lines, the left for base lines.
/// A pane narrower than a card (the right side of a side-by-side card in a narrow folder summary)
/// moves the card left until it ends inside the row: the band is the card's alone.
fn card_rect(rect: Rect, layout: Layout, is_right: bool, frame: &Frame<'_>) -> Rect {
    let single_pane = layout.is_single_pane();
    let inset = frame.metrics.text_inset(layout.has_two_numbers());
    let is_right_pane = !single_pane && is_right;
    let pane_left = if is_right_pane { rect.left() + frame.left_width } else { rect.left() };
    let pane_width = match (single_pane, is_right_pane) {
        (true, _) => rect.width(),
        (false, true) => rect.width() - frame.left_width,
        (false, false) => frame.left_width,
    };
    let width =
        (pane_width - inset - CARD_END_GAP).clamp(CARD_MIN_WIDTH, CARD_MAX_WIDTH).min(rect.width());
    let left = (pane_left + inset).min(rect.right() - CARD_END_GAP - width).max(rect.left());
    Rect::from_min_size(pos2(left, rect.top() + CARD_GAP), vec2(width, f32::INFINITY))
}

/// This pass is the last egui runs for this frame although it would run more: one asking to be
/// discarded would be shown anyway. With a single pass per frame there is no later one to wait for.
fn is_last_pass(ctx: &egui::Context) -> bool {
    let max_passes = ctx.options(|options| options.max_passes.get());
    max_passes > 1 && ctx.current_pass_index() + 1 >= max_passes
}
