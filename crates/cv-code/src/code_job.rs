//! Lays out one code line: syntax colors, word-level change highlights on top, tabs expanded.

use std::ops::Range;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Color32, FontId, Painter, Pos2};

use cv_base::theme::syntax::{self, TokenKind};
use cv_base::theme::{Palette, motion};
use cv_syntax::highlight::Span;
use cv_widgets::mono_text;
use cv_widgets::text_selection::{self, LineGrid};

pub(crate) const TAB_WIDTH: usize = 4;
/// Changed-word bands reach one point past their glyphs, like egui's text backgrounds.
const WORD_EXPAND: f32 = 1.0;

pub struct JobStyle<'a> {
    pub font: &'a FontId,
    pub palette: &'a Palette,
    pub word_background: Color32,
    pub line_height: f32,
    /// Wrap width in points; `None` keeps the line on one row.
    pub wrap_width: Option<f32>,
    /// How far the syntax colors show, from the plain text color (0) to their own (1): below 1
    /// only while colors that arrived after the rows were on screen fade in.
    pub syntax_mix: f32,
}

impl JobStyle<'_> {
    fn color(&self, kind: TokenKind) -> Color32 {
        let color = syntax::color(kind, self.palette);
        if self.syntax_mix >= 1.0 {
            color
        } else {
            motion::mix(self.palette.text, color, self.syntax_mix)
        }
    }
}

pub fn layout_job(
    text: &str,
    spans: &[Span],
    words: &[Range<usize>],
    style: &JobStyle<'_>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    if let Some(width) = style.wrap_width {
        job.wrap =
            TextWrapping { max_width: width, break_anywhere: true, ..TextWrapping::default() };
    }
    let mut cuts: Vec<usize> = Vec::with_capacity(spans.len() + words.len() * 2 + 2);
    cuts.push(0);
    cuts.extend(spans.iter().map(|span| span.start()));
    for range in words {
        cuts.extend([range.start, range.end]);
    }
    cuts.push(text.len());
    cuts.retain(|cut| *cut <= text.len() && text.is_char_boundary(*cut));
    cuts.sort_unstable();
    cuts.dedup();

    let has_tabs = text.contains('\t');
    let mut column = 0;
    let mut span_index = 0;
    let mut word_index = 0;
    for window in cuts.windows(2) {
        let (start, end) = (window[0], window[1]);
        while spans.get(span_index + 1).is_some_and(|next| next.start() <= start) {
            span_index += 1;
        }
        let kind = spans
            .get(span_index)
            .filter(|span| span.start() <= start)
            .map_or(TokenKind::Plain, |span| span.kind());
        while words.get(word_index).is_some_and(|range| range.end <= start) {
            word_index += 1;
        }
        let is_changed_word = words.get(word_index).is_some_and(|range| range.start <= start);
        let format = TextFormat {
            font_id: style.font.clone(),
            color: style.color(kind),
            background: if is_changed_word { style.word_background } else { Color32::TRANSPARENT },
            line_height: Some(style.line_height),
            ..TextFormat::default()
        };
        let segment = &text[start..end];
        if has_tabs {
            let expanded = expand_tabs(segment, &mut column);
            job.append(&expanded, 0.0, format);
        } else {
            job.append(segment, 0.0, format);
        }
    }
    job
}

/// Paints one code line with its top-left at `origin` and returns how many visual rows it took
/// (more than one when wrapped). ASCII lines are stamped from cached glyphs, only the columns
/// inside the clip rect; anything else goes through egui's text layout.
pub(crate) fn paint(
    painter: &Painter,
    origin: Pos2,
    text: &str,
    spans: &[Span],
    words: &[Range<usize>],
    style: &JobStyle<'_>,
) -> usize {
    if !mono_text::is_stampable(text) {
        let galley = painter.layout_job(layout_job(text, spans, words, style));
        let rows = galley.rows.len().max(1);
        painter.galley(origin, galley, style.palette.text);
        return rows;
    }
    let ctx = painter.ctx();
    let clip = painter.clip_rect();
    let stamped = mono_text::Glyphs::with(ctx, style.font, Some(style.line_height), |glyphs| {
        let advance = glyphs.advance();
        if advance <= 0.0 {
            return None;
        }
        let wrap_columns = wrap_columns(style.wrap_width, advance);
        // Unwrapped lines can be far wider than the view: stamp only the columns that show.
        let visible = if wrap_columns.is_some() {
            0..usize::MAX
        } else {
            let first = ((clip.left() - origin.x) / advance).floor().max(0.0) as usize;
            let last = ((clip.right() - origin.x) / advance).ceil().max(0.0) as usize + 1;
            first..last
        };
        let place = |column: usize| match wrap_columns {
            Some(columns) => (column / columns, column % columns),
            None => (0, column),
        };
        let mut stamp = mono_text::Stamp::new(glyphs);
        stamp.rows_at_least(1);
        let mut column = 0;
        let mut span_index = 0;
        let mut word_index = 0;
        let mut word_start: Option<usize> = None;
        for (byte_index, byte) in text.bytes().enumerate() {
            while spans.get(span_index + 1).is_some_and(|next| next.start() <= byte_index) {
                span_index += 1;
            }
            while words.get(word_index).is_some_and(|range| range.end <= byte_index) {
                word_index += 1;
            }
            let in_word = words.get(word_index).is_some_and(|range| range.start <= byte_index);
            let width = if byte == b'\t' { tab_stop(column) } else { 1 };
            match (in_word, word_start) {
                (true, None) => word_start = Some(column),
                (false, Some(start)) => {
                    paint_word(&mut stamp, start..column, wrap_columns, style.word_background);
                    word_start = None;
                }
                _ => {}
            }
            if byte != b'\t' && byte != b' ' && visible.contains(&column) {
                let kind = spans
                    .get(span_index)
                    .filter(|span| span.start() <= byte_index)
                    .map_or(TokenKind::Plain, |span| span.kind());
                let (row, row_column) = place(column);
                stamp.char(ctx, row, row_column, byte, style.color(kind));
            }
            column += width;
        }
        if let Some(start) = word_start {
            paint_word(&mut stamp, start..column, wrap_columns, style.word_background);
        }
        let rows = wrap_columns.map_or(1, |columns| column.div_ceil(columns).max(1));
        stamp.rows_at_least(rows);
        if wrap_columns.is_none() {
            stamp.extend_to(column);
        }
        Some((stamp.finish()?, rows))
    });
    match stamped {
        Some((galley, rows)) => {
            painter.galley(origin, galley, style.palette.text);
            rows
        }
        None => 1,
    }
}

/// Columns per visual row of a stamped line wrapped at `width` points.
pub(crate) fn wrap_columns(width: Option<f32>, advance: f32) -> Option<usize> {
    width.map(|width| ((width / advance).floor() as usize).max(1))
}

/// Columns a tab at `column` takes: up to the next tab stop.
pub(crate) fn tab_stop(column: usize) -> usize {
    TAB_WIDTH - column % TAB_WIDTH
}

/// Where the laid-out columns `selected` (tabs expanded) of `text` sit as [`paint`] draws it: per
/// visual row the row's index and the x-range from the text's left edge. A selection that runs on
/// into the next line (`newline` set) also covers that many points after the end of the text, so
/// an empty line in the middle of a selection still shows.
pub(crate) fn selected_spans(
    painter: &Painter,
    text: &str,
    spans: &[Span],
    words: &[Range<usize>],
    selected: Range<usize>,
    newline: Option<f32>,
    style: &JobStyle<'_>,
) -> Vec<(usize, Range<f32>)> {
    if mono_text::is_stampable(text) {
        let advance =
            mono_text::Glyphs::with(painter.ctx(), style.font, Some(style.line_height), |glyphs| {
                glyphs.advance()
            });
        if advance <= 0.0 {
            return Vec::new();
        }
        let grid = LineGrid::Mono {
            advance,
            columns: columns(text),
            per_row: wrap_columns(style.wrap_width, advance),
        };
        return text_selection::selection_spans(&grid, selected, newline);
    }
    // Beyond ASCII, glyphs differ in width: ask egui's layout, which `paint` uses there too.
    let galley = painter.layout_job(layout_job(text, spans, words, style));
    text_selection::selection_spans(&LineGrid::Laid(&galley), selected, newline)
}

/// A changed-word band, split at row ends when the line wraps.
fn paint_word(
    stamp: &mut mono_text::Stamp<'_>,
    columns: Range<usize>,
    wrap_columns: Option<usize>,
    color: Color32,
) {
    let Some(per_row) = wrap_columns else {
        stamp.background(0, columns, color, WORD_EXPAND);
        return;
    };
    let mut start = columns.start;
    while start < columns.end {
        let row = start / per_row;
        let row_end = ((row + 1) * per_row).min(columns.end);
        stamp.background(
            row,
            start % per_row..start % per_row + (row_end - start),
            color,
            WORD_EXPAND,
        );
        start = row_end;
    }
}

/// Width of `text` in columns once tabs are expanded.
pub fn columns(text: &str) -> usize {
    if !text.contains('\t') {
        return text.chars().count();
    }
    let mut column = 0;
    expand_tabs(text, &mut column);
    column
}

/// Laid-out character index (tabs expanded) where character `index` of `text` starts.
pub(crate) fn expanded_index(text: &str, index: usize) -> usize {
    if !text.contains('\t') {
        return index;
    }
    let mut column = 0;
    for character in text.chars().take(index) {
        column += if character == '\t' { tab_stop(column) } else { 1 };
    }
    column
}

/// Character of `text` at laid-out index `expanded`: inside a tab, the nearer edge of it.
pub fn char_index(text: &str, expanded: usize) -> usize {
    if !text.contains('\t') {
        return expanded.min(text.chars().count());
    }
    let mut column = 0;
    for (index, character) in text.chars().enumerate() {
        let width = if character == '\t' { tab_stop(column) } else { 1 };
        if expanded < column + width {
            return if (expanded - column) * 2 >= width { index + 1 } else { index };
        }
        column += width;
    }
    text.chars().count()
}

fn expand_tabs(text: &str, column: &mut usize) -> String {
    let mut expanded = String::with_capacity(text.len() + 8);
    for character in text.chars() {
        if character == '\t' {
            let spaces = tab_stop(*column);
            expanded.extend(std::iter::repeat_n(' ', spaces));
            *column += spaces;
        } else {
            expanded.push(character);
            *column += 1;
        }
    }
    expanded
}

#[cfg(test)]
mod tests {
    use egui::epaint::text::ByteRangeExt as _;

    use super::*;

    fn sections(
        text: &str,
        spans: &[Span],
        words: &[Range<usize>],
    ) -> Vec<(String, TokenKind, bool)> {
        let palette = Palette::LIGHT;
        let font = FontId::monospace(13.0);
        let style = JobStyle {
            font: &font,
            palette: &palette,
            word_background: Color32::RED,
            line_height: 20.0,
            wrap_width: None,
            syntax_mix: 1.0,
        };
        let job = layout_job(text, spans, words, &style);
        job.sections
            .iter()
            .map(|section| {
                let kind = TokenKind::ALL
                    .into_iter()
                    .find(|kind| syntax::color(*kind, &palette) == section.format.color)
                    .unwrap();
                (
                    job.text[section.byte_range.as_usize()].to_owned(),
                    kind,
                    section.format.background == Color32::RED,
                )
            })
            .collect()
    }

    #[test]
    fn layers_word_changes_over_syntax_colors() {
        let text = "let total = 3;";
        let spans = [
            Span::new(0, TokenKind::Keyword),
            Span::new(3, TokenKind::Plain),
            Span::new(12, TokenKind::Number),
        ];
        let result = sections(text, &spans, std::slice::from_ref(&(9..14)));
        assert_eq!(
            result,
            [
                ("let".to_owned(), TokenKind::Keyword, false),
                (" total".to_owned(), TokenKind::Plain, false),
                (" = ".to_owned(), TokenKind::Plain, true),
                ("3;".to_owned(), TokenKind::Number, true),
            ]
        );
    }

    #[test]
    fn expands_tabs_to_the_next_stop() {
        let result = sections("\tx\ty", &[Span::new(1, TokenKind::Variable)], &[]);
        let text: String = result.iter().map(|(text, _, _)| text.as_str()).collect();
        assert_eq!(text, "    x   y");
        assert_eq!(columns("\tx\ty"), 9);
        assert_eq!(columns("äb"), 2);
    }

    #[test]
    fn maps_characters_to_laid_out_columns_and_back() {
        // "\tx\ty" lays out as "    x   y".
        assert_eq!(
            (0..=4).map(|index| expanded_index("\tx\ty", index)).collect::<Vec<_>>(),
            [0, 4, 5, 8, 9]
        );
        assert_eq!(
            (0..=9).map(|column| char_index("\tx\ty", column)).collect::<Vec<_>>(),
            [0, 0, 1, 1, 1, 2, 2, 3, 3, 4]
        );
        assert_eq!((expanded_index("äb", 1), char_index("äb", 7)), (1, 2));
    }
}
