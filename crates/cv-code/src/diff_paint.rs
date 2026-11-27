//! Paints code rows directly with the painter: no widgets per line, so only visible rows cost
//! anything. A pane is `| change stripe | line numbers | +/− | code |`; inline panes have two
//! number columns. New only paints a calmer focus style: `| line number | change bar | code |`
//! with a light accent tint on changed lines and a red wedge where lines were removed.

use std::ops::Range;

use cv_base::theme::{Palette, motion};
use cv_widgets::mono_text;
use cv_widgets::text_selection::{self, RowSelection};
use egui::{Align2, Color32, FontId, Painter, Pos2, Rangef, Rect, Shape, Stroke, Ui, pos2, vec2};
use review_git::LineKind;

use crate::code_job::{self, JobStyle};
use crate::find_paint::{self, FindMark};
use crate::line_source::CodeLine;

const STRIPE_WIDTH: f32 = 3.0;
const MARKER_WIDTH: f32 = 20.0;
/// New only: the change bar and the removal wedge start this far into the marker column.
const FOCUS_BAR_INSET: f32 = 3.0;
const WEDGE_WIDTH: f32 = 7.0;
const WEDGE_HALF_HEIGHT: f32 = 5.0;
const NUMBER_PADDING: f32 = 10.0;
/// Space kept free right of the longest line when scrolled fully right.
pub const TEXT_END_PADDING: f32 = 24.0;

/// Sizes derived from the code font, computed once per frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    pub font: FontId,
    pub number_font: FontId,
    pub line_height: f32,
    pub gap_height: f32,
    /// The thin band standing for unchanged lines a compact diff leaves out.
    pub separator_height: f32,
    pub char_width: f32,
    pub number_width: f32,
}

impl Metrics {
    pub fn new(ui: &Ui, font_size: f32, max_line_number: u32) -> Self {
        let font = FontId::monospace(font_size);
        let number_font = FontId::monospace(font_size - 1.0);
        let (char_width, digit_width) = ui.fonts_mut(|fonts| {
            (fonts.glyph_width(&font, '0'), fonts.glyph_width(&number_font, '0'))
        });
        // Four digits at least, so a file growing past 999 lines on expand does not shift the code.
        let digits = max_line_number.max(1).ilog10() as usize + 1;
        let line_height = (font_size * 1.54).round();
        Self {
            font,
            number_font,
            line_height,
            gap_height: (line_height * 1.6).round(),
            separator_height: (line_height * 0.9).round(),
            char_width,
            number_width: (digits.max(4) as f32 * digit_width + NUMBER_PADDING * 2.0).round(),
        }
    }

    /// Left edge of the code text inside a pane, relative to the pane's left edge; `two_numbers`
    /// for the inline gutter with base and head numbers.
    pub fn text_inset(&self, two_numbers: bool) -> f32 {
        let columns = if two_numbers { 2.0 } else { 1.0 };
        self.number_width * columns + MARKER_WIDTH
    }

    /// The marker column, where the hover "+" for a new comment sits.
    pub fn marker_rect(&self, pane: Rect, two_numbers: bool, row_height: f32) -> Rect {
        let left = pane.left() + self.text_inset(two_numbers) - MARKER_WIDTH;
        Rect::from_min_size(
            pos2(left, pane.top()),
            vec2(MARKER_WIDTH, row_height.min(self.line_height)),
        )
    }

    /// New only: where the removal wedge between two rows sits, its point on the boundary `y`.
    /// The hit area is larger than the wedge, so it is easy to hover.
    pub fn wedge_rect(&self, pane_left: f32, y: f32) -> Rect {
        let bar = self.focus_bar_left(pane_left);
        Rect::from_min_max(
            pos2(bar - FOCUS_BAR_INSET - 4.0, y - self.line_height * 0.4),
            pos2(bar + WEDGE_WIDTH + 6.0, y + self.line_height * 0.4),
        )
    }

    /// New only: the left edge of the change bar and the removal wedge in a pane at `pane_left`.
    fn focus_bar_left(&self, pane_left: f32) -> f32 {
        pane_left + self.text_inset(false) - MARKER_WIDTH + FOCUS_BAR_INSET
    }
}

pub struct PaintContext<'a> {
    pub painter: &'a Painter,
    pub palette: &'a Palette,
    pub metrics: &'a Metrics,
}

/// How changed lines look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineStyle {
    /// Green and red lines with +/− markers.
    Diff,
    /// New only: the accent bar and a light tint instead of green/red and +/−.
    Focus,
    /// An added or deleted file shown whole: every line changed, so none is marked.
    Plain,
}

/// The line belongs to a multi-line comment: a posted thread or the draft being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeMark {
    Thread,
    Draft,
}

/// How one pane of a code row is drawn.
pub struct PaneLine<'a> {
    pub line: Option<CodeLine<'a>>,
    /// Inline gutter: base and head numbers.
    pub has_two_numbers: bool,
    pub style: LineStyle,
    pub words: &'a [Range<usize>],
    pub h_offset: f32,
    /// Wrap width of the code area in points, for wrapped layout.
    pub wrap_width: Option<f32>,
    pub is_hovered: bool,
    /// The marker column shows the comment button instead of +/−.
    pub hide_marker: bool,
    pub selection: Option<RowSelection>,
    pub range: Option<RangeMark>,
    pub invisibles: Invisibles,
    /// See [`JobStyle::syntax_mix`].
    pub syntax_mix: f32,
}

/// What a line shows of the characters that are otherwise invisible, so a change of only those
/// can be seen: marks for the spaces and tabs inside its changed-word bands, and after its text a
/// tag for a line ending or byte order mark that differs from the line it replaced (or that
/// replaced it), and the "no newline at end of file" mark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Invisibles {
    /// `·` for each space and `→` for each tab inside the changed-word bands.
    pub whitespace: bool,
    /// "CRLF" or "LF": the line's ending differs from its partner's.
    pub ending: Option<&'static str>,
    /// The line carries a byte order mark its partner does not.
    pub bom: bool,
    /// The line is its file's last and has no newline.
    pub no_newline: bool,
}

/// What [`paint_line`] drew: how many visual rows the text took (more than one when wrapped), and
/// where the "no newline at end of file" mark is, for its tooltip.
pub struct Painted {
    pub rows: usize,
    pub no_newline: Option<Rect>,
}

pub const NO_NEWLINE_HINT: &str = "No newline at end of file";
const TAG_GAP: f32 = 6.0;
const TAG_PADDING: f32 = 4.0;

pub struct LineColors {
    pub background: Color32,
    pub gutter: Color32,
    pub stripe: Color32,
    pub word: Color32,
    pub marker: Color32,
}

pub fn line_colors(kind: LineKind, palette: &Palette) -> LineColors {
    match kind {
        LineKind::Added => LineColors {
            background: palette.diff_added_line,
            gutter: motion::mix(palette.diff_added_line, palette.diff_added_word, 0.45),
            stripe: palette.success,
            word: palette.diff_added_word,
            marker: palette.success,
        },
        LineKind::Deleted => LineColors {
            background: palette.diff_deleted_line,
            gutter: motion::mix(palette.diff_deleted_line, palette.diff_deleted_word, 0.45),
            stripe: palette.error,
            word: palette.diff_deleted_word,
            marker: palette.error,
        },
        LineKind::Context | LineKind::Hunk => LineColors {
            background: Color32::TRANSPARENT,
            gutter: Color32::TRANSPARENT,
            stripe: Color32::TRANSPARENT,
            word: Color32::TRANSPARENT,
            marker: palette.text_secondary,
        },
    }
}

/// New only: one calm accent for every changed line, no word highlights.
fn focus_colors(kind: LineKind, palette: &Palette) -> LineColors {
    match kind {
        LineKind::Added => LineColors {
            background: focus_tint(palette),
            gutter: Color32::TRANSPARENT,
            stripe: palette.accent,
            word: Color32::TRANSPARENT,
            marker: palette.accent,
        },
        LineKind::Deleted | LineKind::Context | LineKind::Hunk => {
            line_colors(LineKind::Context, palette)
        }
    }
}

/// New only: the light accent tint of a changed line.
pub fn focus_tint(palette: &Palette) -> Color32 {
    motion::mix(palette.surface, palette.accent, if palette.is_dark { 0.10 } else { 0.05 })
}

/// New only: the red wedge in the marker column where lines were removed, pointing at the
/// boundary `y` between two rows of `pane`. Active (hovered or open) it grows a little and draws a
/// hairline across the code, so it is clear where the removed lines were.
pub fn paint_wedge(
    painter: &Painter,
    metrics: &Metrics,
    pane: Rect,
    y: f32,
    palette: &Palette,
    is_active: bool,
) {
    let hairline = Rangef::new(pane.left() + metrics.text_inset(false), pane.right());
    paint_wedge_at(painter, metrics.focus_bar_left(pane.left()), hairline, y, palette, is_active);
}

/// The red wedge of removed content with its back edge at `tip_left`, pointing at `y`; active it
/// grows a little and draws a hairline over `hairline`. [`paint_wedge`] places it in a code pane,
/// the rendered diff in its own gutter.
pub fn paint_wedge_at(
    painter: &Painter,
    tip_left: f32,
    hairline: Rangef,
    y: f32,
    palette: &Palette,
    is_active: bool,
) {
    let grow = if is_active { 1.5 } else { 0.0 };
    if is_active {
        painter.hline(hairline, y, Stroke::new(1.0, palette.error.gamma_multiply(0.6)));
    }
    let points = vec![
        pos2(tip_left, y - WEDGE_HALF_HEIGHT - grow),
        pos2(tip_left + WEDGE_WIDTH + grow, y),
        pos2(tip_left, y + WEDGE_HALF_HEIGHT + grow),
    ];
    painter.add(Shape::convex_polygon(points, palette.error, Stroke::NONE));
}

/// Paints one pane of a code row.
pub fn paint_line(context: &PaintContext<'_>, pane: Rect, line: &PaneLine<'_>) -> Painted {
    paint_flashed_line(context, pane, line, 0.0, &[])
}

/// [`paint_line`] with the flash of a line the view was sent to at `flash` strength (0 for none)
/// under its text, and the find bar's hits `finds` on it.
pub fn paint_flashed_line(
    context: &PaintContext<'_>,
    pane: Rect,
    line: &PaneLine<'_>,
    flash: f32,
    finds: &[FindMark],
) -> Painted {
    let Some(code) = line.line else {
        paint_empty(context.painter, pane, context.palette);
        return Painted { rows: 1, no_newline: None };
    };
    let colors = match line.style {
        LineStyle::Diff => line_colors(code.kind, context.palette),
        LineStyle::Focus => focus_colors(code.kind, context.palette),
        LineStyle::Plain => line_colors(LineKind::Context, context.palette),
    };
    paint_backdrop(context, pane, line, &colors);
    if flash > 0.0 {
        paint_flash(context.painter, pane, context.palette, flash);
    }
    paint_gutter(context, pane, line, &code, &colors);
    paint_code(context, pane, line, &code, &colors, finds)
}

/// Under a line: its change tint and bar, the band of a multi-line comment, the selection of the
/// whole line and the hover tint.
fn paint_backdrop(
    context: &PaintContext<'_>,
    pane: Rect,
    line: &PaneLine<'_>,
    colors: &LineColors,
) {
    let (painter, palette, metrics) = (context.painter, context.palette, context.metrics);
    if colors.background != Color32::TRANSPARENT {
        // New only tints from the change bar rightwards: the numbers stay on the plain gutter, so a
        // changed line never reads like a (full-width) gap bar.
        let stripe_left = if line.style == LineStyle::Focus {
            metrics.focus_bar_left(pane.left())
        } else {
            pane.left()
        };
        painter.rect_filled(
            Rect::from_min_max(pos2(stripe_left, pane.top()), pane.max),
            0,
            colors.background,
        );
        if colors.gutter != Color32::TRANSPARENT {
            let gutter = Rect::from_min_max(
                pane.min,
                pos2(pane.left() + metrics.text_inset(line.has_two_numbers), pane.bottom()),
            );
            painter.rect_filled(gutter, 0, colors.gutter);
        }
        painter.rect_filled(
            Rect::from_min_size(pos2(stripe_left, pane.top()), vec2(STRIPE_WIDTH, pane.height())),
            0,
            colors.stripe,
        );
    }
    if let Some(mark) = line.range {
        paint_range_band(painter, pane, palette, mark);
    }
    if line.selection == Some(RowSelection::Line) {
        painter.rect_filled(pane, 0, palette.text_selection);
    }
    if line.is_hovered {
        painter.rect_filled(pane, 0, hover_tint(palette));
    }
}

/// The line numbers of `code` on its first visual row, and its +/− in the marker column.
fn paint_gutter(
    context: &PaintContext<'_>,
    pane: Rect,
    line: &PaneLine<'_>,
    code: &CodeLine<'_>,
    colors: &LineColors,
) {
    let (painter, palette, metrics) = (context.painter, context.palette, context.metrics);
    let two_numbers = line.has_two_numbers;
    let first_row = Rect::from_min_size(pane.min, vec2(pane.width(), metrics.line_height));
    let numbers: &[u32] =
        if two_numbers { &[code.old_line, code.new_line] } else { &[code.number] };
    let is_marked = line.selection == Some(RowSelection::Line) || line.range.is_some();
    let number_color = match code.kind {
        _ if is_marked => palette.accent_text,
        LineKind::Context => palette.diff_gutter,
        _ if line.style == LineStyle::Plain => palette.diff_gutter,
        _ if line.style == LineStyle::Focus => palette.accent_text,
        _ => palette.text_secondary,
    };
    for (column, number) in numbers.iter().enumerate() {
        if *number > 0 {
            let right = pane.left() + metrics.number_width * (column as f32 + 1.0) - NUMBER_PADDING;
            let center = pos2(right, first_row.center().y);
            mono_text::text(
                painter,
                center,
                Align2::RIGHT_CENTER,
                &number.to_string(),
                &metrics.number_font,
                number_color,
            );
        }
    }
    let marker = match code.kind {
        // The change bar already marks the line in new only; a whole file needs no marks.
        _ if line.style != LineStyle::Diff => "",
        LineKind::Added => "+",
        LineKind::Deleted => "−",
        LineKind::Context | LineKind::Hunk => "",
    };
    if !marker.is_empty() && !line.hide_marker {
        let marker_rect = metrics.marker_rect(pane, two_numbers, pane.height());
        painter.text(
            marker_rect.center(),
            Align2::CENTER_CENTER,
            marker,
            metrics.font.clone(),
            colors.marker,
        );
    }
}

/// The code of `code` with its syntax colors, changed-word bands, find hits `finds` and text
/// selection, then the marks of its invisible characters.
fn paint_code(
    context: &PaintContext<'_>,
    pane: Rect,
    line: &PaneLine<'_>,
    code: &CodeLine<'_>,
    colors: &LineColors,
    finds: &[FindMark],
) -> Painted {
    let (painter, palette, metrics) = (context.painter, context.palette, context.metrics);
    let text_left = pane.left() + metrics.text_inset(line.has_two_numbers);
    let code_area = Rect::from_min_max(pos2(text_left, pane.top()), pane.max);
    let clipped = painter.with_clip_rect(code_area.intersect(painter.clip_rect()));
    let style = JobStyle {
        font: &metrics.font,
        palette,
        word_background: colors.word,
        line_height: metrics.line_height,
        wrap_width: line.wrap_width,
        syntax_mix: line.syntax_mix,
    };
    let origin = snap(pos2(text_left - line.h_offset, pane.top()));
    let per_row = code_job::wrap_columns(line.wrap_width, metrics.char_width);
    let (advance, line_height) = (metrics.char_width, metrics.line_height);
    let grid = Grid { origin, advance, line_height, per_row };
    find_paint::paint_marks(&clipped, &grid, code.text, finds, palette);
    let words = find_paint::words_outside(line.words, finds);
    let rows = match &line.selection {
        Some(RowSelection::Text { columns, through_end }) => {
            // The selection covers the changed-word bands it spans, as in an editor.
            let words = words_outside(code.text, &words, columns);
            let selected = Selected {
                columns: code_job::expanded_index(code.text, columns.start)
                    ..code_job::expanded_index(code.text, columns.end),
                newline: through_end.then(|| text_selection::newline_width(metrics.char_width)),
            };
            paint_text_selection(&clipped, pane, origin.x, *code, &words, &style, selected);
            code_job::paint(&clipped, origin, code.text, code.spans, &words, &style)
        }
        _ => code_job::paint(&clipped, origin, code.text, code.spans, &words, &style),
    };
    if line.invisibles.whitespace {
        paint_whitespace_marks(&clipped, &grid, code.text, line.words, palette.text_secondary);
    }
    let no_newline =
        paint_line_end(&clipped, &grid, code.text, &line.invisibles, colors.word, palette);
    Painted { rows, no_newline }
}

/// Where the columns of a line sit: `per_row` columns per visual row when it wraps.
pub struct Grid {
    pub origin: Pos2,
    pub advance: f32,
    pub line_height: f32,
    pub per_row: Option<usize>,
}

impl Grid {
    /// Top-left of `column` (tabs expanded).
    pub fn cell(&self, column: usize) -> Pos2 {
        let (row, column) = match self.per_row {
            Some(per_row) => (column / per_row, column % per_row),
            None => (0, column),
        };
        pos2(
            self.origin.x + column as f32 * self.advance,
            self.origin.y + row as f32 * self.line_height,
        )
    }
}

/// `·` for every space and `→` across every tab inside the changed-word bands `words`, drawn as
/// shapes (no font needs the glyphs) in `color`.
fn paint_whitespace_marks(
    painter: &Painter,
    grid: &Grid,
    text: &str,
    words: &[Range<usize>],
    color: Color32,
) {
    if words.is_empty() {
        return;
    }
    let mut column = 0;
    for (byte, character) in text.char_indices() {
        let width = if character == '\t' { code_job::tab_stop(column) } else { 1 };
        let is_changed = words.iter().any(|word| word.contains(&byte));
        if is_changed && (character == ' ' || character == '\t') {
            let cell = grid.cell(column);
            let center_y = cell.y + grid.line_height / 2.0;
            if character == ' ' {
                painter.circle_filled(
                    pos2(cell.x + grid.advance / 2.0, center_y),
                    (grid.advance * 0.12).max(1.0),
                    color,
                );
            } else {
                let stroke = Stroke::new(1.0, color);
                let left = cell.x + grid.advance * 0.25;
                let right = cell.x + grid.advance * (width as f32 - 0.25);
                painter.line_segment([pos2(left, center_y), pos2(right, center_y)], stroke);
                let head = (grid.advance * 0.3).min(4.0);
                painter.line_segment(
                    [pos2(right - head, center_y - head), pos2(right, center_y)],
                    stroke,
                );
                painter.line_segment(
                    [pos2(right - head, center_y + head), pos2(right, center_y)],
                    stroke,
                );
            }
        }
        column += width;
    }
}

/// After the text: the "CRLF"/"LF" and "BOM" tags (filled like changed words, as they are what
/// changed) and the "no newline at end of file" mark, a slashed circle as on GitHub. Returns the
/// mark's rect.
fn paint_line_end(
    painter: &Painter,
    grid: &Grid,
    text: &str,
    invisibles: &Invisibles,
    fill: Color32,
    palette: &Palette,
) -> Option<Rect> {
    let end = grid.cell(code_job::columns(text));
    let center_y = end.y + grid.line_height / 2.0;
    let mut x = end.x + TAG_GAP;
    let font = FontId::proportional((grid.line_height * 0.5).round());
    let tags = [invisibles.ending, invisibles.bom.then_some("BOM")];
    for tag in tags.into_iter().flatten() {
        let galley = painter.layout_no_wrap(tag.to_owned(), font.clone(), palette.text_secondary);
        let rect = Rect::from_min_size(
            pos2(x, center_y - grid.line_height * 0.36),
            vec2(galley.size().x + TAG_PADDING * 2.0, grid.line_height * 0.72),
        );
        let background = if fill == Color32::TRANSPARENT { palette.border } else { fill };
        painter.rect_filled(rect, 3.0, background);
        painter.galley(
            pos2(rect.left() + TAG_PADDING, rect.center().y - galley.size().y / 2.0),
            galley,
            palette.text_secondary,
        );
        x = rect.right() + TAG_GAP / 2.0;
    }
    if !invisibles.no_newline {
        return None;
    }
    let radius = (grid.line_height * 0.24).round();
    let center = pos2(x + radius, center_y);
    let stroke = Stroke::new(1.25, palette.text_secondary);
    painter.circle_stroke(center, radius, stroke);
    let slash = radius * std::f32::consts::FRAC_1_SQRT_2;
    painter.line_segment(
        [pos2(center.x - slash, center.y + slash), pos2(center.x + slash, center.y - slash)],
        stroke,
    );
    Some(Rect::from_center_size(center, vec2(radius * 2.0 + 4.0, grid.line_height)))
}

/// The laid-out columns (tabs expanded) a text selection covers on a line, and the width of its
/// newline when it runs on into the next line.
struct Selected {
    columns: Range<usize>,
    newline: Option<f32>,
}

/// A text selection on a line, under its text, in the theme's selection color: whole rows high,
/// so the selected lines of a block meet without gaps; from the first selected column (leading
/// whitespace included) and, when the selection runs on into the next line, a newline's width
/// past the end. A wrapped line gets a band per visual row. `text_x` is where the text starts.
fn paint_text_selection(
    painter: &Painter,
    pane: Rect,
    text_x: f32,
    code: CodeLine<'_>,
    words: &[Range<usize>],
    style: &JobStyle<'_>,
    selected: Selected,
) {
    let spans = code_job::selected_spans(
        painter,
        code.text,
        code.spans,
        words,
        selected.columns,
        selected.newline,
        style,
    );
    for fill in text_selection::selection_fills(
        spans,
        text_x,
        pane,
        style.line_height,
        painter.pixels_per_point(),
    ) {
        painter.rect_filled(fill, 0, style.palette.text_selection);
    }
}

/// `words` (byte ranges of `text`) without the characters `columns`.
fn words_outside(text: &str, words: &[Range<usize>], columns: &Range<usize>) -> Vec<Range<usize>> {
    let byte = |index: usize| text.char_indices().nth(index).map_or(text.len(), |(byte, _)| byte);
    let (start, end) = (byte(columns.start), byte(columns.end));
    words
        .iter()
        .flat_map(|word| [word.start..word.end.min(start), word.start.max(end)..word.end])
        .filter(|part| !part.is_empty())
        .collect()
}

/// The flash of a line the view was sent to, `strength` from 1 down to 0: an accent wash laid
/// over the line's change tint (blended, so added and removed lines keep their color under the
/// text) and the accent bar at its left edge.
fn paint_flash(painter: &Painter, pane: Rect, palette: &Palette, strength: f32) {
    let wash = if palette.is_dark { 0.30 } else { 0.20 };
    painter.rect_filled(pane, 0, palette.accent.gamma_multiply(wash * strength));
    painter.rect_filled(
        Rect::from_min_size(pane.min, vec2(STRIPE_WIDTH, pane.height())),
        0,
        palette.accent.gamma_multiply(strength),
    );
}

/// A line of a multi-line comment: a light accent wash and an accent bar along the pane's edge.
/// The draft being written is a little stronger than posted threads.
fn paint_range_band(painter: &Painter, pane: Rect, palette: &Palette, mark: RangeMark) {
    let strength = match (mark, palette.is_dark) {
        (RangeMark::Draft, true) => 0.20,
        (RangeMark::Draft, false) => 0.12,
        (RangeMark::Thread, true) => 0.13,
        (RangeMark::Thread, false) => 0.08,
    };
    painter.rect_filled(pane, 0, palette.accent.gamma_multiply(strength));
    painter.rect_filled(
        Rect::from_min_size(pane.min, vec2(STRIPE_WIDTH, pane.height())),
        0,
        palette.accent,
    );
}

/// The side of a side-by-side row that has no line (a pure addition or deletion), and of a
/// rendered Markdown diff that has no block.
pub fn paint_empty(painter: &Painter, pane: Rect, palette: &Palette) {
    painter.rect_filled(pane, 0, palette.diff_empty);
    // Fine diagonal hatching marks "nothing here" without competing with real lines.
    let spacing = 8.0;
    let stroke =
        Stroke::new(1.0, palette.border.gamma_multiply(if palette.is_dark { 0.5 } else { 0.7 }));
    let clipped = painter.with_clip_rect(pane.intersect(painter.clip_rect()));
    let height = pane.height();
    let mut x = pane.left() - height;
    // Anchor the pattern to absolute x so stacked rows line up into continuous stripes.
    x -= x.rem_euclid(spacing) - (pane.top().rem_euclid(spacing));
    while x < pane.right() {
        clipped.line_segment([pos2(x, pane.bottom()), pos2(x + height, pane.top())], stroke);
        x += spacing;
    }
}

fn hover_tint(palette: &Palette) -> Color32 {
    if palette.is_dark { Color32::from_white_alpha(10) } else { Color32::from_black_alpha(9) }
}

/// Divider between the two panes of a side-by-side row.
pub fn paint_divider(painter: &Painter, x: f32, rect: Rect, palette: &Palette) {
    painter.vline(x, rect.y_range(), Stroke::new(1.0, palette.border));
}

/// Pixel-aligned text keeps monospace glyphs crisp.
fn snap(position: Pos2) -> Pos2 {
    pos2(position.x.round(), position.y.round())
}
