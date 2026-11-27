//! Caret geometry and motion: where a position is drawn, which position a point hits, and the
//! moves of the arrow, Home/End and Page keys and of word jumps. Positions map 1:1 to galley
//! chars (an atom is one placeholder char, a hard break one newline), so a block offset is a
//! galley `CCursor` index.

use egui::text::CCursor;
use egui::{Context, Galley, Pos2, Rect, Vec2, pos2, vec2};

use super::layout::{Laid, LayoutCache};
use super::model::{Document, Pos, Selection};

/// Where block `index` and its galley sit, from the content's top-left `origin`.
pub struct Placed {
    pub laid: std::sync::Arc<Laid>,
    pub top: f32,
    /// Top-left of the galley.
    pub galley_origin: Pos2,
}

pub fn place(
    layout: &mut LayoutCache,
    ctx: &Context,
    doc: &Document,
    origin: Pos2,
    index: usize,
) -> Placed {
    let top = layout.offsets(doc).tops.get(index).copied().unwrap_or_default();
    let laid = layout.get(ctx, doc, index);
    let galley_origin = origin + vec2(laid.indent + laid.inset.x, top + laid.inset.y);
    Placed { laid, top, galley_origin }
}

/// The block whose box holds `y` (content coordinates), the nearest at the ends.
pub fn block_at_y(layout: &mut LayoutCache, doc: &Document, y: f32) -> usize {
    let offsets = layout.offsets(doc);
    let after = offsets.tops.partition_point(|top| *top <= y);
    let index = after.saturating_sub(1);
    // In the gap after a block, the nearer of the two.
    let bottom = offsets.tops.get(index).copied().unwrap_or_default()
        + offsets.heights.get(index).copied().unwrap_or_default();
    match offsets.tops.get(index + 1) {
        Some(next) if y > bottom && (y - bottom) > (next - y) => index + 1,
        _ => index,
    }
}

/// Which way a caret move goes: up, to the row's start or to the char before, or the reverse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Backward,
    Forward,
}

/// The position under `point` (screen coordinates).
pub fn pos_at(
    layout: &mut LayoutCache,
    ctx: &Context,
    doc: &Document,
    origin: Pos2,
    point: Pos2,
) -> Pos {
    let index = block_at_y(layout, doc, point.y - origin.y);
    let placed = place(layout, ctx, doc, origin, index);
    let hit = placed.laid.galley.cursor_from_pos(point - placed.galley_origin);
    Pos::new(index, hit.index.0.min(doc.block(index).len()))
}

/// The caret's rectangle at `pos` (screen coordinates), one line high.
pub fn caret_rect(
    layout: &mut LayoutCache,
    ctx: &Context,
    doc: &Document,
    origin: Pos2,
    pos: Pos,
) -> Rect {
    let placed = place(layout, ctx, doc, origin, pos.block);
    let rect = placed.laid.galley.pos_from_cursor(CCursor::new(pos.offset));
    rect.translate(placed.galley_origin.to_vec2())
}

/// One row up (`Backward`) or down, keeping the column `x` (screen coordinates), across
/// blocks. Returns the new position and the column to keep.
pub fn vertical(
    layout: &mut LayoutCache,
    ctx: &Context,
    doc: &Document,
    origin: Pos2,
    pos: Pos,
    x: Option<f32>,
    direction: Direction,
) -> (Pos, f32) {
    let down = direction == Direction::Forward;
    let at = CCursor::new(pos.offset);
    let placed = place(layout, ctx, doc, origin, pos.block);
    let galley = &placed.laid.galley;
    let here = galley.pos_from_cursor(at).translate(placed.galley_origin.to_vec2());
    let x = x.unwrap_or(here.center().x);
    let row = galley.layout_from_cursor(at).row;
    let galley_x = x - placed.galley_origin.x;
    let within = if down { row + 1 < galley.rows.len() } else { row > 0 };
    if within {
        let (moved, _) = if down {
            galley.cursor_down_one_row(&at, Some(galley_x))
        } else {
            galley.cursor_up_one_row(&at, Some(galley_x))
        };
        return (Pos::new(pos.block, moved.index.0), x);
    }
    let next = if down { pos.block + 1 } else { pos.block.wrapping_sub(1) };
    if next >= doc.blocks.len() {
        let edge = if down { doc.end() } else { Pos::new(0, 0) };
        return (edge, x);
    }
    let placed = place(layout, ctx, doc, origin, next);
    let galley = &placed.laid.galley;
    let row_y = if down { 0.0 } else { galley.rect.height() - 1.0 };
    let hit = galley.cursor_from_pos(vec2(x - placed.galley_origin.x, row_y.max(0.0)));
    (Pos::new(next, hit.index.0.min(doc.block(next).len())), x)
}

/// The start (`Backward`) or end of the row `pos` is on.
pub fn row_edge(
    layout: &mut LayoutCache,
    ctx: &Context,
    doc: &Document,
    pos: Pos,
    direction: Direction,
) -> Pos {
    let laid = layout.get(ctx, doc, pos.block);
    let galley: &Galley = &laid.galley;
    let at = CCursor::new(pos.offset);
    let edge = match direction {
        Direction::Forward => galley.cursor_end_of_row(&at),
        Direction::Backward => galley.cursor_begin_of_row(&at),
    };
    Pos::new(pos.block, edge.index.0.min(doc.block(pos.block).len()))
}

/// A page up (`Backward`) or down: the position `height` above or below the caret.
pub fn page(
    layout: &mut LayoutCache,
    ctx: &Context,
    doc: &Document,
    origin: Pos2,
    pos: Pos,
    height: f32,
    direction: Direction,
) -> Pos {
    let here = caret_rect(layout, ctx, doc, origin, pos);
    let y = match direction {
        Direction::Forward => here.center().y + height,
        Direction::Backward => here.center().y - height,
    };
    let total = layout.offsets(doc).total;
    if y - origin.y < 0.0 {
        return Pos::new(0, 0);
    }
    if y - origin.y > total {
        return doc.end();
    }
    pos_at(layout, ctx, doc, origin, pos2(here.center().x, y))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Punctuation,
}

fn class(ch: char) -> CharClass {
    if ch.is_whitespace() {
        CharClass::Space
    } else if ch.is_alphanumeric() || ch == '_' {
        CharClass::Word
    } else {
        CharClass::Punctuation
    }
}

/// Ctrl+Left: the start of the word before the caret, or the end of the block before.
pub fn word_left(doc: &Document, pos: Pos) -> Pos {
    if pos.offset == 0 {
        return match pos.block.checked_sub(1) {
            Some(before) => Pos::new(before, doc.block(before).len()),
            None => pos,
        };
    }
    let chars: Vec<char> = doc.block(pos.block).content.text().chars().collect();
    let mut at = pos.offset.min(chars.len());
    while at > 0 && class(chars[at - 1]) == CharClass::Space {
        at -= 1;
    }
    if let Some(kind) = at.checked_sub(1).map(|before| class(chars[before])) {
        while at > 0 && class(chars[at - 1]) == kind {
            at -= 1;
        }
    }
    Pos::new(pos.block, at)
}

/// Ctrl+Right: the end of the word after the caret, or the start of the next block.
pub fn word_right(doc: &Document, pos: Pos) -> Pos {
    let chars: Vec<char> = doc.block(pos.block).content.text().chars().collect();
    if pos.offset >= chars.len() {
        return if pos.block + 1 < doc.blocks.len() { Pos::new(pos.block + 1, 0) } else { pos };
    }
    let mut at = pos.offset;
    while at < chars.len() && class(chars[at]) == CharClass::Space {
        at += 1;
    }
    if let Some(kind) = chars.get(at).map(|ch| class(*ch)) {
        while at < chars.len() && class(chars[at]) == kind {
            at += 1;
        }
    }
    Pos::new(pos.block, at)
}

/// The word (or run of spaces or punctuation) at `pos`, for a double click.
pub fn word_at(doc: &Document, pos: Pos) -> Selection {
    let chars: Vec<char> = doc.block(pos.block).content.text().chars().collect();
    let Some(kind) = chars
        .get(pos.offset)
        .or(pos.offset.checked_sub(1).and_then(|at| chars.get(at)))
        .map(|ch| class(*ch))
    else {
        return Selection::caret(pos);
    };
    let mut start = pos.offset.min(chars.len());
    while start > 0 && class(chars[start - 1]) == kind {
        start -= 1;
    }
    let mut end = pos.offset;
    while end < chars.len() && class(chars[end]) == kind {
        end += 1;
    }
    Selection { anchor: Pos::new(pos.block, start), head: Pos::new(pos.block, end) }
}

/// The char before (`Backward`) or after `pos`, across block boundaries.
pub fn step(doc: &Document, pos: Pos, direction: Direction) -> Pos {
    let pos = doc.clamp(pos);
    if direction == Direction::Forward {
        if pos.offset < doc.block(pos.block).len() {
            Pos::new(pos.block, pos.offset + 1)
        } else if pos.block + 1 < doc.blocks.len() {
            Pos::new(pos.block + 1, 0)
        } else {
            pos
        }
    } else if pos.offset > 0 {
        Pos::new(pos.block, pos.offset - 1)
    } else if let Some(before) = pos.block.checked_sub(1) {
        Pos::new(before, doc.block(before).len())
    } else {
        pos
    }
}

/// The size of the area the caret is scrolled into view with: a line above and below.
pub const SCROLL_MARGIN: Vec2 = vec2(0.0, 24.0);
