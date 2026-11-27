//! How hits look: an amber fill under the matched text, stronger with an outline for the current
//! hit, so it stands out without color alone; in the overview strip a tick per hit in the color
//! of its side (red old, green new or the accent in New only, grey for an unchanged line) and an
//! amber bar for the current one. The selection's other occurrences show at the same time in a
//! grey-blue fill of their own and thin grey ticks; the test below keeps the two fills apart from
//! each other and from every row background in both themes.

use std::borrow::Cow;
use std::ops::Range;

use egui::{Color32, Painter, Rangef, Rect, Stroke, StrokeKind, vec2};

use cv_base::theme::Palette;

use crate::code_job;
use crate::diff_paint::Grid;

/// The version a hit was found in (Both for an unchanged line searched once), and the version a
/// pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitSide {
    Old,
    New,
    Both,
}

/// A hit painted on a line: its bytes in the line's text and how it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindMark {
    pub bytes: Range<usize>,
    pub look: MarkLook,
}

/// How a mark in the text shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkLook {
    /// A find bar hit.
    Hit,
    /// The find bar's current hit: stronger, with an outline.
    Current,
    /// Another occurrence of the selected text: quieter than a hit.
    Occurrence,
}

/// The fill of a hit in text.
pub fn fill(palette: &Palette, is_current: bool) -> Color32 {
    let strength = match (is_current, palette.is_dark) {
        (true, true) => 0.55,
        (true, false) => 0.45,
        (false, true) => 0.28,
        (false, false) => 0.22,
    };
    palette.warning.gamma_multiply(strength)
}

/// The hits `marks` of `text`, under its characters: a band per visual row they cover.
pub(crate) fn paint_marks(
    painter: &Painter,
    grid: &Grid,
    text: &str,
    marks: &[FindMark],
    palette: &Palette,
) {
    for mark in marks {
        let Range { start, end } = columns(text, &mark.bytes);
        let per_row = grid.per_row.unwrap_or(usize::MAX).max(1);
        let mut from = start;
        while from < end {
            let to = end.min((from / per_row).saturating_add(1).saturating_mul(per_row));
            let band = Rect::from_min_size(
                grid.cell(from),
                vec2((to - from) as f32 * grid.advance, grid.line_height),
            );
            let color = match mark.look {
                MarkLook::Occurrence => palette.occurrence,
                look => fill(palette, look == MarkLook::Current),
            };
            painter.rect_filled(band, 2.0, color);
            #[cfg(any(test, feature = "test-support"))]
            note_mark(painter.ctx(), mark.look, band, color);
            if mark.look == MarkLook::Current {
                let outline = Stroke::new(1.0, palette.warning);
                painter.rect_stroke(band, 2.0, outline, StrokeKind::Inside);
                #[cfg(any(test, feature = "test-support"))]
                note_current(painter.ctx(), band, painter.clip_rect());
            }
            from = to;
        }
    }
}

/// The columns (tabs expanded) of the bytes `bytes` of `text`.
pub fn columns(text: &str, bytes: &Range<usize>) -> Range<usize> {
    let column = |byte: usize| {
        let characters = text.get(..byte).map_or(0, |head| head.chars().count());
        code_job::expanded_index(text, characters)
    };
    column(bytes.start)..column(bytes.end)
}

/// The sideways offset of a code area `width` wide, scrolled `offset` now, that shows `columns`
/// of `advance` each: unchanged while they are in view, else they come in a few columns from the
/// near edge (from the left edge when they are wider than the area).
pub fn reveal_offset(columns: Range<usize>, advance: f32, width: f32, offset: f32) -> f32 {
    let (start, end) = (columns.start as f32 * advance, columns.end as f32 * advance);
    if start >= offset && end <= offset + width {
        return offset;
    }
    let margin = (4.0 * advance).min(width / 4.0);
    if start < offset || end - start + 2.0 * margin > width {
        (start - margin).max(0.0)
    } else {
        end + margin - width
    }
}

/// The changed-word bands `words` without the hits' bytes, so the hits show over them.
pub(crate) fn words_outside<'a>(
    words: &'a [Range<usize>],
    marks: &[FindMark],
) -> Cow<'a, [Range<usize>]> {
    if marks.is_empty() || words.is_empty() {
        return Cow::Borrowed(words);
    }
    let mut parts: Vec<Range<usize>> = words.to_vec();
    for mark in marks {
        parts = parts
            .into_iter()
            .flat_map(|word| {
                [
                    word.start..word.end.min(mark.bytes.start),
                    word.start.max(mark.bytes.end)..word.end,
                ]
            })
            .filter(|part| !part.is_empty())
            .collect();
    }
    Cow::Owned(parts)
}

/// The overview strip's ticks of the selection's other occurrences at the rows' tops `ys`: thin
/// and grey, under the find bar's ticks.
pub fn paint_occurrence_ticks(
    painter: &Painter,
    inner: Rect,
    ys: impl Iterator<Item = f32>,
    palette: &Palette,
) {
    let x = Rangef::new(inner.center().x, inner.right());
    let color = palette.text_secondary.gamma_multiply(0.6);
    let mut last = f32::NEG_INFINITY;
    for y in ys {
        if y - last < 1.0 {
            continue;
        }
        last = y;
        painter.rect_filled(Rect::from_x_y_ranges(x, Rangef::new(y, y + 1.5)), 0, color);
    }
}

/// The overview strip's ticks of the hits at `finds` (the y of each row's top and the hit's
/// side) across `inner`, and the current hit's bar at `current`.
pub fn paint_ticks(
    painter: &Painter,
    inner: Rect,
    finds: impl Iterator<Item = (f32, HitSide)>,
    current: Option<f32>,
    palette: &Palette,
    is_focus: bool,
) {
    let mut last = f32::NEG_INFINITY;
    for (y, side) in finds {
        if y - last < 1.0 {
            continue;
        }
        last = y;
        let color = match side {
            HitSide::Old => palette.error,
            HitSide::New if is_focus => palette.accent,
            HitSide::New => palette.success,
            HitSide::Both => palette.text_secondary,
        };
        let tick = Rect::from_x_y_ranges(inner.x_range(), Rangef::new(y, y + 2.0));
        painter.rect_filled(tick.expand2(vec2(0.0, 1.0)), 0, palette.surface);
        painter.rect_filled(tick, 0, color);
    }
    if let Some(y) = current {
        let bar = Rect::from_x_y_ranges(inner.x_range(), Rangef::new(y - 1.0, y + 3.0));
        painter.rect_filled(bar.expand(1.0), 1, palette.surface);
        painter.rect_filled(bar, 1, palette.warning);
    }
}

/// Notes the current hit painted at `rect` in the area `visible` shows (see [`last_current`]).
#[cfg(any(test, feature = "test-support"))]
pub fn note_current(ctx: &egui::Context, rect: Rect, visible: Rect) {
    cv_base::frame_probe::note(ctx, CURRENT_NOTE, (rect, visible));
}

/// Where the last frame painted the current hit (its text in each pane, or its block in the
/// rendered diff), each with the area that shows it; empty when it painted none.
#[cfg(any(test, feature = "test-support"))]
pub fn last_current(ctx: &egui::Context) -> Vec<(Rect, Rect)> {
    cv_base::frame_probe::last(ctx, CURRENT_NOTE)
}

/// Notes a mark painted at `rect` in `color` (see [`last_marks`]).
#[cfg(any(test, feature = "test-support"))]
fn note_mark(ctx: &egui::Context, look: MarkLook, rect: Rect, color: Color32) {
    cv_base::frame_probe::note(ctx, MARK_NOTE, (look, rect, color));
}

/// Every mark the last frame painted in text: how it looks, where and in which fill.
#[cfg(any(test, feature = "test-support"))]
pub fn last_marks(ctx: &egui::Context) -> Vec<(MarkLook, Rect, Color32)> {
    cv_base::frame_probe::last(ctx, MARK_NOTE)
}

#[cfg(any(test, feature = "test-support"))]
const MARK_NOTE: &str = "diff_find_marks";

#[cfg(any(test, feature = "test-support"))]
const CURRENT_NOTE: &str = "diff_find_current";

#[cfg(test)]
mod tests {
    use super::*;
    use cv_base::theme::palette::{contrast_ratio, over};

    #[test]
    fn a_hit_out_of_view_scrolls_in_from_the_near_edge() {
        // 10 points per column, a code area 200 wide.
        assert_eq!(reveal_offset(5..9, 10.0, 200.0, 0.0), 0.0, "in view: stays");
        assert_eq!(
            reveal_offset(40..44, 10.0, 200.0, 0.0),
            280.0,
            "right of view: in at the right"
        );
        assert_eq!(reveal_offset(10..14, 10.0, 200.0, 300.0), 60.0, "left of view: in at the left");
        assert_eq!(reveal_offset(40..80, 10.0, 200.0, 0.0), 360.0, "too wide: its start shows");
        assert_eq!(columns("\tab", &(1..3)), 4..6, "a tab spans to the next stop");
    }

    /// The straight-line distance of two colours in 0–255 RGB.
    fn distance(a: Color32, b: Color32) -> f32 {
        let channels = |color: Color32| [color.r(), color.g(), color.b()].map(f32::from);
        channels(a).iter().zip(channels(b)).map(|(a, b)| (a - b).powi(2)).sum::<f32>().sqrt()
    }

    /// The occurrence fill reads on every row background (it was near invisible in the dark
    /// theme), and it stays apart from the find hits shown at the same time.
    #[test]
    fn occurrences_read_on_every_row_and_apart_from_hits_in_both_themes() {
        for palette in [Palette::LIGHT, Palette::DARK] {
            let rows = [palette.surface, palette.diff_added_line, palette.diff_deleted_line];
            for row in rows {
                let plain = over(row, row);
                let occurrence = over(palette.occurrence, row);
                let hit = over(fill(&palette, false), row);
                let theme = if palette.is_dark { "dark" } else { "light" };
                let seen = contrast_ratio(occurrence, plain);
                assert!(seen >= 1.4, "{theme}: occurrence on {row:?} at {seen:.2}:1");
                let apart = distance(occurrence, hit);
                assert!(apart >= 35.0, "{theme}: occurrence and hit {apart:.0} apart on {row:?}");
            }
        }
    }

    #[test]
    fn hits_cut_the_changed_words_they_cover() {
        let marks = [FindMark { bytes: 4..8, look: MarkLook::Hit }];
        let words = [0..6, 10..12];
        assert_eq!(&*words_outside(&words, &marks), [0..4, 10..12]);
        assert!(
            matches!(words_outside(&words, &[]), Cow::Borrowed(_)),
            "nothing to cut, nothing copied"
        );
    }
}
