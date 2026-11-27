//! Comment threads in the rendered diff. Each thread sits under the block that holds its (last)
//! line: the new version's block for a thread on the new side, the old version's for one on
//! removed lines, with the same card, replies and status menu as in the text diff. A line the
//! rendered text does not show (inside a comment or markup, a blank line) puts the card under the
//! next block, noting the line and offering the text diff; so does a removed block New only leaves
//! out. Threads in folded blocks are counted on the fold's bar instead.
//!
//! Cards are measured where they are painted, as in the text diff: until then they count with an
//! estimate, and a card whose height changes because of a hand-off (a reply posted) eases to it.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use egui::{Rect, RichText, Ui, pos2, vec2};
use review_core::{DiffSide, ReviewThread};

use super::model::{other, slot};
use super::{GUTTER, Model, RIGHT_PADDING, Row};
use cv_base::theme::{self, icons, style};
use cv_pr_diff::diff_composer;
use cv_pr_diff::diff_view::cards::{CARD_GAP, CARD_MAX_WIDTH, measure_card};
use cv_pr_threads::{self as threads, CardOptions, ThreadEvent, ThreadsUi};
use cv_widgets::{buttons, glyph};

/// A thread's card under a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ThreadRow {
    /// Index into the threads the rows were built for.
    pub thread: usize,
    /// The lane of the block the card is under.
    pub lane: DiffSide,
    pub note: Option<Note>,
}

/// Why a card is not right under the block of its line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Note {
    /// The line renders as nothing (a comment, markup, a blank line): the card is under the next block.
    Unshown { line: u32 },
    /// New only leaves out the removed block of the line: the card is under the next block.
    Removed { line: u32 },
}

impl Note {
    fn text(self) -> String {
        match self {
            Note::Unshown { line } => {
                format!("On line {line}, which the rendered text does not show")
            }
            Note::Removed { line } => format!("On line {line} of the old version, removed here"),
        }
    }
}

/// Where a thread goes in the rows built so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Placement {
    /// Under row `row`, a block.
    Under { row: usize, thread: ThreadRow, line: u32 },
    /// In the fold at row `row`.
    Folded { row: usize },
}

/// Identifies what the thread rows depend on: the threads, their lines, comments and status.
pub(super) fn threads_key(threads: &[&ReviewThread]) -> u64 {
    let mut hasher = DefaultHasher::new();
    threads.len().hash(&mut hasher);
    for thread in threads {
        thread.id.hash(&mut hasher);
        thread.comments.len().hash(&mut hasher);
        thread.status.hash(&mut hasher);
        thread.is_outdated.hash(&mut hasher);
        if let Some(anchor) = &thread.anchor {
            (anchor.old_line, anchor.new_line, diff_composer::is_right(anchor)).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// The block of `thread`'s line: its side and unit, and the line when the rendered text does not
/// show it (the unit is then the next block, or the last). `None` without a line or blocks.
fn locate(model: &Model, thread: &ReviewThread) -> Option<(DiffSide, usize, Option<u32>)> {
    let anchor = thread.anchor.as_ref()?;
    let wanted = if diff_composer::is_right(anchor) { DiffSide::New } else { DiffSide::Old };
    let line = anchor.line_in(wanted).or(anchor.new_line).or(anchor.old_line)?;
    // A side the file does not have (an added file's old side): its one version.
    let side = if model.units(wanted).is_empty() { other(wanted) } else { wanted };
    let units = &model.units(side).units;
    let last = units.len().checked_sub(1)?;
    let index = units.partition_point(|unit| unit.lines.end <= line);
    Some(match units.get(index) {
        Some(unit) if unit.lines.start <= line && side == wanted => (side, index, None),
        Some(_) => (side, index, Some(line)),
        None => (side, last, Some(line)),
    })
}

/// Where `thread` goes in `rows`: under the row showing its block, or the block's other version
/// (an unchanged block shows once inline, an edited one only in its new version in New only),
/// under the next block where its removed block is left out, or in a fold.
pub(super) fn place(
    model: &Model,
    rows: &[Row],
    unit_rows: [&[Option<usize>]; 2],
    pair_rows: &[usize],
    index: usize,
    thread: &ReviewThread,
) -> Option<Placement> {
    let (side, unit, unshown) = locate(model, thread)?;
    let line =
        thread.anchor.as_ref().and_then(|anchor| anchor.new_line.or(anchor.old_line)).unwrap_or(0);
    let row_of = |side: DiffSide, unit: usize| unit_rows[slot(side)].get(unit).copied().flatten();
    let under = |row: usize, lane: DiffSide, note: Option<Note>| Placement::Under {
        row,
        thread: ThreadRow { thread: index, lane, note },
        line,
    };
    let note = unshown.map(|line| Note::Unshown { line });
    if let Some(row) = row_of(side, unit) {
        return Some(under(row, side, note));
    }
    let pair_index = model.pair_of(side, unit)?;
    let pair = &model.pairs[pair_index];
    let across = match side {
        DiffSide::Old => pair.new.map(|unit| (DiffSide::New, unit)),
        DiffSide::New => pair.old.map(|unit| (DiffSide::Old, unit)),
    };
    if let Some(row) = across.and_then(|(side, unit)| Some((side, row_of(side, unit)?))) {
        return Some(under(row.1, row.0, note));
    }
    let row = *pair_rows.get(pair_index)?;
    match rows.get(row)? {
        Row::Folded { .. } => Some(Placement::Folded { row }),
        Row::Removal { .. } => {
            let removed = Some(Note::Removed {
                line: unshown.unwrap_or_else(|| model.units(side).units[unit].lines.start),
            });
            let below =
                (row + 1..rows.len()).find(|index| matches!(rows[*index], Row::Blocks { .. }));
            let host = below.or_else(|| {
                (0..row).rev().find(|index| matches!(rows[*index], Row::Blocks { .. }))
            })?;
            let lane = rows[host]
                .cells()
                .into_iter()
                .flatten()
                .next()
                .map_or(DiffSide::New, |cell| cell.side);
            Some(under(host, lane, removed))
        }
        Row::Blocks { .. } | Row::Thread(_) => None,
    }
}

/// What the user did on a card.
pub(super) enum CardAction {
    Thread(ThreadEvent),
    /// The note's "Show in text diff".
    ShowSource,
}

/// Lays out the card of `thread` in its lane (`lane`'s x range) on a band across `row`, its note
/// above it, and returns the row height it needs. `clip`: the row is easing to that height.
pub(super) fn show_card_row(
    ui: &mut Ui,
    row: Rect,
    lane: egui::Rangef,
    thread: &ReviewThread,
    note: Option<Note>,
    clip: bool,
    threads_ui: &mut ThreadsUi,
) -> (f32, Option<CardAction>) {
    let width = (lane.span() - GUTTER - RIGHT_PADDING).clamp(240.0, CARD_MAX_WIDTH);
    let card = Rect::from_min_size(
        pos2(lane.min + GUTTER, row.top() + CARD_GAP),
        vec2(width, f32::INFINITY),
    );
    measure_card(ui, row, card, clip, |ui| {
        let mut action = None;
        if let Some(note) = note {
            if show_note(ui, note) {
                action = Some(CardAction::ShowSource);
            }
            ui.add_space(style::SPACE_XS);
        }
        if let Some(event) =
            threads::show_card(ui, thread, threads_ui, &CardOptions { show_location: false })
        {
            action = Some(CardAction::Thread(event));
        }
        action
    })
}

/// Where the comment really is, and the text diff that shows its line; true when clicked.
fn show_note(ui: &mut Ui, note: Note) -> bool {
    let palette = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        glyph::decorative(
            ui,
            RichText::new(icons::INFO)
                .font(icons::rest_font(&palette, icons::SIZE_SMALL))
                .color(palette.text_secondary),
        );
        ui.label(RichText::new(note.text()).size(style::SMALL_SIZE).color(palette.text_secondary));
        buttons::subtle(ui, icons::FILE_TEXT, "Show in text diff").clicked()
    })
    .inner
}

/// The thread's card is left out: comments are hidden (not `show_threads`) and the user did not
/// show it (it is not in `revealed`).
pub(super) fn is_hidden(show_threads: bool, revealed: &HashSet<String>, id: &str) -> bool {
    !show_threads && !revealed.contains(id)
}
