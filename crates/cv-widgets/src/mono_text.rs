//! Fast monospace text for code and log views: printable-ASCII text is stamped together from
//! cached glyphs instead of going through egui's text layout.
//!
//! egui shapes every text section with `HarfRust` and builds a fresh shaping plan per section, so a
//! syntax-colored line of ~15 tokens costs ~65 µs to lay out. A 4K diff that scrolls fast shows
//! ~140 new lines per frame: 9 ms of layout alone. In a monospace font ASCII needs no shaping;
//! column `n` sits at `n × advance`. Here each glyph is laid out through egui once per frame per
//! sub-pixel bin (galley-cache hits after the first frame), and a line becomes one quad per
//! character, placed exactly where egui would place it (same pixel, same sub-pixel bitmap). The
//! quads are wrapped in a galley, so egui still snaps it to pixels, normalizes the atlas
//! coordinates and applies opacity as for any other text.
//!
//! Anything else (non-ASCII, tabs in jobs, wrapping in jobs, italics, underlines) goes through
//! egui's layout unchanged.

use std::ops::Range;
use std::sync::Arc;

use egui::emath::GuiRounding as _;
use egui::epaint::text::{ByteRangeExt as _, Galley, LayoutJob, PlacedRow, TextFormat};
use egui::{Align2, Color32, Context, FontId, Id, Mesh, Painter, Pos2, Rect, pos2, vec2};

const FIRST: u8 = b' ';
const LAST: u8 = b'~';
const GLYPHS: usize = (LAST - FIRST + 1) as usize;

/// egui renders glyphs at quarter-pixel offsets ("sub-pixel bins"), one bitmap per bin.
const BINS: usize = 4;
const BIN_OFFSETS: [f32; BINS] = [0.0, 0.25, 0.5, 0.75];

/// True when every character can be stamped: printable ASCII and tabs (callers expand tabs).
pub fn is_stampable(text: &str) -> bool {
    text.bytes().all(|byte| (FIRST..=LAST).contains(&byte) || byte == b'\t')
}

/// One character's quad, relative to the whole pixel its pen sits on and the row top.
#[derive(Clone, Copy, Debug)]
struct Quad {
    rect: Rect,
    /// Texel corners in the font atlas.
    uv: Rect,
}

/// Per bin: `None` until looked up this pass (only the bins a line uses are rasterized), then
/// the quad, or `None` for a glyph without pixels (space).
type Slot = [Option<Option<Quad>>; BINS];

/// The glyphs of one font at one line height, valid for the pass they were looked up in (the
/// font atlas is only ever rebuilt between passes).
#[derive(Clone, Default)]
pub struct Glyphs {
    pass: u64,
    pixels_per_point: f32,
    font: Option<FontId>,
    line_height: Option<f32>,
    /// Advance of one column in physical pixels.
    advance_px: f32,
    /// Height of a row, and the vertical extent of a glyph's logical rect (for backgrounds).
    row_height: f32,
    logical_y: (f32, f32),
    /// False when the font renders at whole pixels only.
    has_bins: bool,
    /// Layout of a single character: the shell every stamped galley is cloned from.
    template: Option<Arc<Galley>>,
    quads: Vec<Slot>,
}

impl Glyphs {
    /// Runs `paint` with this pass's glyphs of `font` at `line_height` (`None`: the font's own).
    pub fn with<R>(
        ctx: &Context,
        font: &FontId,
        line_height: Option<f32>,
        paint: impl FnOnce(&mut Glyphs) -> R,
    ) -> R {
        let id = Id::unique(("mono_text_glyphs", font, line_height.map(f32::to_bits)));
        let pass = ctx.cumulative_pass_nr();
        let pixels_per_point = ctx.pixels_per_point();
        let mut glyphs: Glyphs = ctx.data_mut(|data| data.remove_temp(id)).unwrap_or_default();
        // Bitwise: the glyph table is a cache keyed on the exact scale egui reports.
        let is_current = glyphs.pass == pass
            && glyphs.pixels_per_point.to_bits() == pixels_per_point.to_bits()
            && glyphs.font.as_ref() == Some(font);
        if !is_current {
            glyphs = Glyphs::look_up(ctx, font, line_height, pass, pixels_per_point);
        }
        let result = paint(&mut glyphs);
        ctx.data_mut(|data| data.insert_temp(id, glyphs));
        result
    }

    fn look_up(
        ctx: &Context,
        font: &FontId,
        line_height: Option<f32>,
        pass: u64,
        pixels_per_point: f32,
    ) -> Self {
        let template = layout_char(ctx, font, line_height, '0', 0.0);
        let first = template.rows.first();
        let glyph = first.and_then(|row| row.row.glyphs.first());
        let logical = glyph.map_or(Rect::NOTHING, egui::epaint::text::Glyph::logical_rect);
        let mut glyphs = Self {
            pass,
            pixels_per_point,
            font: Some(font.clone()),
            line_height,
            advance_px: glyph.map_or(0.0, |glyph| glyph.advance_width) * pixels_per_point,
            row_height: first.map_or(0.0, |row| row.row.size.y),
            logical_y: (logical.top(), logical.bottom()),
            has_bins: true,
            template: Some(template),
            quads: vec![[None; BINS]; GLYPHS],
        };
        // A half-pixel offset gets its own bitmap only when the font is binned.
        glyphs.has_bins = match (glyphs.quad(ctx, b'0', 0), glyphs.quad(ctx, b'0', 2)) {
            (Some(zero), Some(half)) => zero.uv != half.uv,
            _ => true,
        };
        glyphs
    }

    /// Width of one column in points.
    pub fn advance(&self) -> f32 {
        self.advance_px / self.pixels_per_point
    }

    /// The quad of `byte` rendered at sub-pixel `bin`.
    fn quad(&mut self, ctx: &Context, byte: u8, bin: usize) -> Option<Quad> {
        let index = usize::from(byte.checked_sub(FIRST)?);
        if let Some(known) = *self.quads.get(index)?.get(bin)? {
            return known;
        }
        let font = self.font.as_ref()?;
        let galley = layout_char(
            ctx,
            font,
            self.line_height,
            char::from(byte),
            BIN_OFFSETS[bin] / self.pixels_per_point,
        );
        let quad = galley.rows.first().and_then(|row| row.row.glyphs.first()).and_then(|glyph| {
            let uv = glyph.uv_rect;
            (!uv.is_nothing()).then(|| Quad {
                rect: Rect::from_min_size(pos2(0.0, glyph.pos.y) + uv.offset, uv.size),
                uv: Rect::from_min_max(
                    pos2(f32::from(uv.min[0]), f32::from(uv.min[1])),
                    pos2(f32::from(uv.max[0]), f32::from(uv.max[1])),
                ),
            })
        });
        self.quads[index][bin] = Some(quad);
        quad
    }

    /// The whole pixel and bin egui places the pen at for `column` (as epaint's `SubpixelBin`).
    fn place(&self, column: usize) -> (f32, usize) {
        let pen = column as f32 * self.advance_px;
        if !self.has_bins {
            return (pen.round(), 0);
        }
        let whole = pen.trunc();
        match pen - whole {
            fraction if fraction < 0.125 => (whole, 0),
            fraction if fraction < 0.375 => (whole, 1),
            fraction if fraction < 0.625 => (whole, 2),
            fraction if fraction < 0.875 => (whole, 3),
            _ => (whole + 1.0, 0),
        }
    }

    /// Left edge of the glyph in `column`, in points, as egui positions it.
    fn glyph_x(&self, column: usize) -> f32 {
        self.place(column).0 / self.pixels_per_point
    }

    /// X of the pen at `column`, in points (the text width after `column` columns).
    fn pen_x(&self, column: usize) -> f32 {
        column as f32 * self.advance_px / self.pixels_per_point
    }
}

fn layout_char(
    ctx: &Context,
    font: &FontId,
    line_height: Option<f32>,
    character: char,
    leading_space: f32,
) -> Arc<Galley> {
    let format = TextFormat {
        font_id: font.clone(),
        color: Color32::WHITE,
        line_height,
        ..TextFormat::default()
    };
    let mut job = LayoutJob::single_section(character.to_string(), format);
    if let Some(section) = job.sections.first_mut() {
        section.leading_space = leading_space;
    }
    ctx.fonts_mut(|fonts| fonts.layout_job(job))
}

/// Collects the quads of one text, row by row, then becomes a galley.
pub struct Stamp<'g> {
    glyphs: &'g mut Glyphs,
    /// Per visual row: backgrounds first, then glyphs (egui's order, so selection overlays work).
    backgrounds: Vec<Mesh>,
    texts: Vec<Mesh>,
    columns: usize,
}

impl<'g> Stamp<'g> {
    pub fn new(glyphs: &'g mut Glyphs) -> Self {
        Self { glyphs, backgrounds: Vec::new(), texts: Vec::new(), columns: 0 }
    }

    fn ensure_row(&mut self, row: usize) {
        while self.texts.len() <= row {
            self.texts.push(Mesh::default());
            self.backgrounds.push(Mesh::default());
        }
    }

    /// Character `byte` (printable ASCII) at `column` of visual row `row`.
    pub fn char(&mut self, ctx: &Context, row: usize, column: usize, byte: u8, color: Color32) {
        self.ensure_row(row);
        self.columns = self.columns.max(column + 1);
        let (pixel, bin) = self.glyphs.place(column);
        if let Some(quad) = self.glyphs.quad(ctx, byte, bin) {
            let rect = quad.rect.translate(vec2(pixel / self.glyphs.pixels_per_point, 0.0));
            self.texts[row].add_rect_with_uv(rect, quad.uv, color);
        }
    }

    /// Marks `columns` of row `row` as taken without drawing (trailing spaces still count).
    pub fn extend_to(&mut self, columns: usize) {
        self.columns = self.columns.max(columns);
    }

    /// A filled band behind `columns` of visual row `row`, as egui draws section backgrounds.
    pub fn background(&mut self, row: usize, columns: Range<usize>, color: Color32, expand: f32) {
        if columns.is_empty() || color == Color32::TRANSPARENT {
            return;
        }
        self.ensure_row(row);
        let left = self.glyphs.glyph_x(columns.start);
        let right = self.glyphs.glyph_x(columns.end - 1) + self.glyphs.advance();
        let (top, bottom) = self.glyphs.logical_y;
        let rect = Rect::from_x_y_ranges(left..=right, top..=bottom)
            .expand(expand)
            .round_to_pixels(self.glyphs.pixels_per_point);
        // Texel (0, 0) of the font atlas is white, as `epaint::WHITE_UV` relies on.
        self.backgrounds[row].add_rect_with_uv(
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::ZERO),
            color,
        );
    }

    /// Makes sure the text has at least `rows` visual rows (empty lines still take a row).
    pub fn rows_at_least(&mut self, rows: usize) {
        if rows > 0 {
            self.ensure_row(rows - 1);
        }
    }

    /// The galley to paint; `None` if the glyphs could not be looked up.
    pub fn finish(self) -> Option<Arc<Galley>> {
        let template = self.glyphs.template.as_ref()?;
        let template_row = template.rows.first()?.row.clone();
        let row_height = self.glyphs.row_height;
        let width = self.glyphs.pen_x(self.columns);
        let mut galley = Galley::clone(template);
        let mut num_vertices = 0;
        let mut num_indices = 0;
        let mut mesh_bounds = Rect::NOTHING;
        galley.rows = self
            .backgrounds
            .into_iter()
            .zip(self.texts)
            .enumerate()
            .map(|(index, (mut mesh, text))| {
                let glyph_start = mesh.vertices.len();
                let glyph_index_start = mesh.indices.len();
                mesh.append(text);
                let mut row = Arc::unwrap_or_clone(template_row.clone());
                row.glyphs.clear();
                row.size = vec2(width, row_height);
                row.visuals.mesh_bounds = mesh.calc_bounds();
                row.visuals.glyph_index_start = glyph_index_start;
                row.visuals.glyph_vertex_range = glyph_start..mesh.vertices.len();
                num_vertices += mesh.vertices.len();
                num_indices += mesh.indices.len();
                let pos = pos2(0.0, index as f32 * row_height);
                if !mesh.is_empty() {
                    mesh_bounds |= row.visuals.mesh_bounds.translate(pos.to_vec2());
                }
                row.visuals.mesh = mesh;
                PlacedRow { pos, row: Arc::new(row), ends_with_newline: false }
            })
            .collect();
        let rows = galley.rows.len().max(1);
        galley.rect = Rect::from_min_size(Pos2::ZERO, vec2(width, rows as f32 * row_height));
        galley.mesh_bounds = mesh_bounds;
        galley.num_vertices = num_vertices;
        galley.num_indices = num_indices;
        galley.elided = false;
        Some(Arc::new(galley))
    }
}

/// Drop-in for `Fonts::layout_job`: stamps single-line, unwrapped, monospace ASCII jobs whose
/// sections differ only in color and background; lays out everything else with egui.
pub fn layout_job(ctx: &Context, job: LayoutJob) -> Arc<Galley> {
    match stampable_font(&job) {
        Some((font, line_height)) => {
            Glyphs::with(ctx, &font, line_height, |glyphs| stamp_job(ctx, glyphs, &job))
        }
        None => None,
    }
    .unwrap_or_else(|| ctx.fonts_mut(|fonts| fonts.layout_job(job)))
}

/// Drop-in for `Painter::text` with the same stamping as [`layout_job`].
pub fn text(
    painter: &Painter,
    pos: Pos2,
    anchor: Align2,
    text: &str,
    font: &FontId,
    color: Color32,
) -> Rect {
    let job = LayoutJob::simple_singleline(text.to_owned(), font.clone(), color);
    let galley = layout_job(painter.ctx(), job);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

/// The font and line height every section of `job` shares, if the job can be stamped.
fn stampable_font(job: &LayoutJob) -> Option<(FontId, Option<f32>)> {
    let first = job.sections.first()?;
    let font = first.format.font_id.clone();
    let line_height = first.format.line_height;
    let is_plain = |format: &TextFormat| {
        let plain = TextFormat {
            font_id: font.clone(),
            line_height,
            color: format.color,
            background: format.background,
            expand_bg: format.expand_bg,
            ..TextFormat::default()
        };
        *format == plain
    };
    let is_stampable = font.family == egui::FontFamily::Monospace
        && job.wrap.max_width == f32::INFINITY
        && job.first_row_min_height == 0.0
        && job.halign == egui::Align::LEFT
        && !job.text.contains('\t')
        && is_stampable(&job.text)
        && job
            .sections
            .iter()
            .all(|section| section.leading_space == 0.0 && is_plain(&section.format));
    is_stampable.then_some((font, line_height))
}

fn stamp_job(ctx: &Context, glyphs: &mut Glyphs, job: &LayoutJob) -> Option<Arc<Galley>> {
    let mut stamp = Stamp::new(glyphs);
    stamp.rows_at_least(1);
    // Adjacent sections with the same background share one band, as in egui.
    let mut band: Option<(usize, usize, Color32, f32)> = None;
    for section in &job.sections {
        // ASCII: byte offsets are columns.
        let range = section.byte_range.as_usize();
        let text = job.text.get(range.clone())?;
        let format = &section.format;
        band = match band {
            Some((start, _, color, expand))
                if color == format.background && expand.to_bits() == format.expand_bg.to_bits() =>
            {
                Some((start, range.end, color, expand))
            }
            previous => {
                if let Some((start, end, color, expand)) = previous {
                    stamp.background(0, start..end, color, expand);
                }
                Some((range.start, range.end, format.background, format.expand_bg))
            }
        };
        for (offset, byte) in text.bytes().enumerate() {
            if byte != b' ' {
                stamp.char(ctx, 0, range.start + offset, byte, format.color);
            }
        }
        stamp.extend_to(range.end);
    }
    if let Some((start, end, color, expand)) = band {
        stamp.background(0, start..end, color, expand);
    }
    stamp.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_printable_ascii_is_stamped() {
        assert!(is_stampable("let x = \"a\";\t// ok"));
        assert!(!is_stampable("naïve"));
        assert!(!is_stampable("tab\u{b}"));
    }

    fn mesh_of(galley: &Galley) -> Vec<(i32, i32, i32, i32, Color32)> {
        let quantize = |value: f32| (value * 64.0).round() as i32;
        galley
            .rows
            .iter()
            .flat_map(|row| {
                let offset = row.pos.to_vec2();
                row.row.visuals.mesh.vertices.iter().map(move |vertex| {
                    (
                        quantize(vertex.pos.x + offset.x),
                        quantize(vertex.pos.y + offset.y),
                        quantize(vertex.uv.x),
                        quantize(vertex.uv.y),
                        vertex.color,
                    )
                })
            })
            .collect()
    }

    /// Lays out `job` both ways in a frame at `pixels_per_point` with the app's fonts.
    fn both_ways(pixels_per_point: f32, job: &LayoutJob) -> (Arc<Galley>, Arc<Galley>) {
        let ctx = Context::default();
        cv_base::theme::fonts::install(&ctx);
        ctx.set_pixels_per_point(pixels_per_point);
        let mut result = None;
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let ctx = ui.ctx().clone();
                let stamped = layout_job(&ctx, job.clone());
                let laid_out = ctx.fonts_mut(|fonts| fonts.layout_job(job.clone()));
                result = Some((stamped, laid_out));
            });
            output.textures_delta.clear();
        }
        result.expect("ran")
    }

    /// Stamped quads land where egui's own layout puts the same glyphs, with the same atlas
    /// texels, colors and backgrounds, at the scales of the target monitors.
    #[test]
    fn matches_egui_layout() {
        let font = FontId::monospace(13.0);
        let format = |color: Color32, background: Color32| TextFormat {
            font_id: font.clone(),
            color,
            background,
            line_height: Some(20.0),
            ..TextFormat::default()
        };
        let mut job = LayoutJob::default();
        job.append(
            "    pub fn handle_event_42(&mut self, ",
            0.0,
            format(Color32::RED, Color32::TRANSPARENT),
        );
        job.append("event: &Event", 0.0, format(Color32::BLUE, Color32::LIGHT_GREEN));
        job.append(
            ") -> Result<(), Error> { return x.y[3] * 7; } // done  ",
            0.0,
            format(Color32::GRAY, Color32::TRANSPARENT),
        );
        for pixels_per_point in [1.0, 1.25, 1.5, 2.0] {
            let (stamped, laid_out) = both_ways(pixels_per_point, &job);
            assert_eq!(mesh_of(&stamped), mesh_of(&laid_out), "at {pixels_per_point}");
            assert_eq!(stamped.rows.len(), 1);
            let delta = (stamped.size() - laid_out.size()).length();
            assert!(delta < 0.05, "sizes differ by {delta} at {pixels_per_point}");
        }
    }

    #[test]
    fn leaves_other_jobs_to_egui() {
        let plain = LayoutJob::simple_singleline(
            "abc".to_owned(),
            FontId::proportional(13.0),
            Color32::RED,
        );
        assert!(stampable_font(&plain).is_none(), "proportional");
        let unicode =
            LayoutJob::simple_singleline("naïve".to_owned(), FontId::monospace(13.0), Color32::RED);
        assert!(stampable_font(&unicode).is_none(), "non-ASCII");
        let mut italic =
            LayoutJob::simple_singleline("abc".to_owned(), FontId::monospace(13.0), Color32::RED);
        italic.sections[0].format.italics = true;
        assert!(stampable_font(&italic).is_none(), "italics");
        let simple =
            LayoutJob::simple_singleline("1234".to_owned(), FontId::monospace(12.0), Color32::RED);
        assert!(stampable_font(&simple).is_some());
    }
}
