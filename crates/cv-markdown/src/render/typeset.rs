//! How rendered text is set: the text format of each run, one layout job per stretch of runs
//! with links resolved and changed words filled, and the one-line sizes table columns plan with.

use std::ops::Range;

use cv_base::theme::{self, Palette};
use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Color32, FontFamily, FontId, Stroke, vec2};

use super::links::is_url;
use super::{CODE_SIZE_DELTA, Marks};
use crate::images::{self, ImageBase};
use crate::parse::{HtmlMark, Inline, Run, Style};
use crate::{InApp, LinkResolver, ResolvedLink};

/// Subscript and superscript are set at three quarters of the text around them.
pub const SCRIPT_SCALE: f32 = 0.75;

/// How the text of one block looks.
#[derive(Clone, Copy)]
pub(super) struct Look {
    pub size: f32,
    pub strong: bool,
    pub color: Color32,
}

/// One block's text as laid out: per run where it ends in the galley text and how its link resolves.
pub(super) struct Laid {
    pub job: LayoutJob,
    pub run_ends: Vec<usize>,
}

/// The text of `runs` as screen readers get it: the galley's text also holds the links' icon
/// glyphs, which read as nothing.
pub(super) fn spoken_text(runs: &[Run]) -> String {
    runs.iter().map(|run| run.text.as_str()).collect()
}

/// What setting text needs from the renderer, borrowed for one block.
pub(super) struct Typesetter<'a> {
    pub palette: &'a Palette,
    /// Multiplies every size.
    pub scale: f32,
    /// What the screen knows about link targets; `None` opens URLs in the browser.
    pub links: Option<&'a dyn LinkResolver>,
    /// Where relative image paths point (a repository file); `None` for comments.
    pub images: Option<ImageBase>,
}

impl Typesetter<'_> {
    /// A design-token length at this scale.
    fn px(&self, length: f32) -> f32 {
        length * self.scale
    }

    /// How each run's link shows; `None` for text and for references nothing resolves.
    pub(super) fn resolve_links(&self, runs: &[Run]) -> Vec<Option<ResolvedLink>> {
        runs.iter()
            .map(|run| {
                let target = run.style.link.as_deref()?;
                let resolved = self.links.and_then(|links| links.resolve(target));
                resolved.or_else(|| {
                    is_url(target).then(|| ResolvedLink {
                        web_url: target.to_owned(),
                        in_app: InApp::No,
                        icon: None,
                        title: None,
                    })
                })
            })
            .collect()
    }

    /// The galley text of `runs`; a link with an icon gets the glyph and a thin space before it.
    /// `marks` (byte ranges of the runs' text) get their fills.
    pub(super) fn layout_job(
        &self,
        ctx: &egui::Context,
        runs: &[Run],
        look: Look,
        hovered: Option<usize>,
        links: &[Option<ResolvedLink>],
        marks: &[Fill],
    ) -> Laid {
        let font = FontId::proportional(look.size);
        let mut job = LayoutJob {
            // A line of only code is as tall as a line of prose, so it sits on the same baseline.
            first_row_min_height: ctx.fonts_mut(|fonts| fonts.row_height(&font)),
            ..LayoutJob::default()
        };
        let mut run_ends = Vec::with_capacity(runs.len());
        let mut offset = 0;
        for (index, run) in runs.iter().enumerate() {
            let link = links.get(index).and_then(Option::as_ref);
            let format = self.format(&run.style, look, hovered == Some(index), link.is_some());
            if let Some(icon) = link.and_then(|link| link.icon) {
                let mut icon_format = format.clone();
                icon_format.underline = Stroke::NONE;
                job.append(&format!("{icon}\u{2009}"), 0.0, icon_format);
            }
            append_marked(&mut job, &run.text, offset, marks, &format, self.px(1.0));
            offset += run.text.len();
            run_ends.push(job.text.len());
        }
        Laid { job, run_ends }
    }

    pub(super) fn format(
        &self,
        style: &Style,
        look: Look,
        hovered: bool,
        is_link: bool,
    ) -> TextFormat {
        let family = if look.strong || style.strong {
            theme::fonts::semibold()
        } else {
            FontFamily::Proportional
        };
        let color = if is_link { self.palette.accent } else { look.color };
        let is_script =
            style.html.has(HtmlMark::Subscript) || style.html.has(HtmlMark::Superscript);
        let size = if is_script { look.size * SCRIPT_SCALE } else { look.size };
        let mut format = TextFormat {
            font_id: FontId::new(size, family),
            color,
            italics: style.emphasis,
            // egui has no baseline shift: superscript sits at the row's top, subscript at its bottom.
            valign: if style.html.has(HtmlMark::Superscript) { Align::TOP } else { Align::BOTTOM },
            ..TextFormat::default()
        };
        if style.strike {
            format.strikethrough = Stroke::new(1.0, color);
        }
        if hovered || style.html.has(HtmlMark::Underline) {
            format.underline = Stroke::new(1.0, color);
        }
        let is_kbd = style.html.has(HtmlMark::Kbd);
        if style.code || is_kbd {
            format.font_id = theme::fonts::code_font(size - self.px(CODE_SIZE_DELTA));
            format.background = if is_kbd { self.palette.pressed } else { self.palette.hover };
            format.expand_bg = self.px(2.0);
        }
        format
    }

    /// The size of a table cell on one line: its text stretches and its images side by side.
    /// The changed-word fill does not change it.
    pub(super) fn measure_cell(
        &self,
        ctx: &egui::Context,
        cell: &Inline,
        look: Look,
        image_width: f32,
    ) -> egui::Vec2 {
        let mut size = egui::Vec2::ZERO;
        let mut add = |part: egui::Vec2| size = vec2(size.x + part.x, size.y.max(part.y));
        for stretch in
            cell.runs.split(|run| run.style.image.is_some()).filter(|stretch| !stretch.is_empty())
        {
            let links = self.resolve_links(stretch);
            let job = self.layout_job(ctx, stretch, look, None, &links, &[]).job;
            add(ctx.fonts_mut(|fonts| fonts.layout_job(job)).size());
        }
        for spec in cell.runs.iter().filter_map(|run| run.style.image.as_deref()) {
            add(images::measure(ctx, spec, self.images, image_width, self.scale));
        }
        size
    }

    /// How wide a table cell's longest word is (by characters, in its run's font), so a column
    /// keeps identifiers whole where it can; images count whole.
    pub(super) fn longest_word(
        &self,
        ctx: &egui::Context,
        cell: &Inline,
        look: Look,
        image_width: f32,
    ) -> f32 {
        let word = cell
            .runs
            .iter()
            .filter(|run| run.style.image.is_none())
            .flat_map(|run| run.text.split_whitespace().map(move |word| (word, run)))
            .max_by_key(|(word, _)| word.chars().count());
        let text = word.map_or(0.0, |(word, run)| {
            let mut job = LayoutJob::default();
            job.append(word, 0.0, self.format(&run.style, look, false, run.style.link.is_some()));
            ctx.fonts_mut(|fonts| fonts.layout_job(job)).size().x
        });
        cell.runs
            .iter()
            .filter_map(|run| run.style.image.as_deref())
            .map(|spec| images::measure(ctx, spec, self.images, image_width, self.scale).x)
            .fold(text, f32::max)
    }
}

/// A part of a block's text and its fill.
pub(super) type Fill = (Range<usize>, Color32);

/// The fills of the layers `marks` over `span` of a block's text, relative to its start, in order
/// and not overlapping: where layers overlap, a later one is blended over an earlier one.
pub(super) fn layer(marks: &[Marks], span: Range<usize>) -> Vec<Fill> {
    let ranges = || marks.iter().flat_map(|layer| layer.ranges.iter());
    let mut cuts: Vec<usize> = ranges()
        .map(|range| range.start.max(span.start)..range.end.min(span.end))
        .filter(|range| !range.is_empty())
        .flat_map(|range| [range.start, range.end])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut fills: Vec<Fill> = Vec::new();
    for window in cuts.windows(2) {
        let (from, to) = (window[0], window[1]);
        let covering = marks.iter().filter(|layer| {
            layer.ranges.iter().any(|range| range.start <= from && range.end >= to)
        });
        let Some(color) = covering.fold(None, |below: Option<Color32>, layer| {
            Some(below.map_or(layer.color, |below| below.blend(layer.color)))
        }) else {
            continue;
        };
        let range = from - span.start..to - span.start;
        match fills.last_mut() {
            Some((last, fill)) if last.end == range.start && *fill == color => last.end = range.end,
            _ => fills.push((range, color)),
        }
    }
    fills
}

/// Appends `text`, whose first byte is byte `offset` of its block's text, with the parts inside
/// `marks` filled.
fn append_marked(
    job: &mut LayoutJob,
    text: &str,
    offset: usize,
    marks: &[Fill],
    format: &TextFormat,
    expand: f32,
) {
    let end = offset + text.len();
    let mut position = offset;
    for (range, color) in marks.iter().filter(|(range, _)| range.start < end && range.end > offset)
    {
        let (start, stop) = (range.start.max(offset), range.end.min(end));
        if start < position
            || !text.is_char_boundary(start - offset)
            || !text.is_char_boundary(stop - offset)
        {
            continue;
        }
        if start > position {
            job.append(&text[position - offset..start - offset], 0.0, format.clone());
        }
        let marked = TextFormat { background: *color, expand_bg: expand, ..format.clone() };
        job.append(&text[start - offset..stop - offset], 0.0, marked);
        position = stop;
    }
    if position < end {
        job.append(&text[position - offset..], 0.0, format.clone());
    }
}

#[cfg(test)]
mod tests {
    use cv_base::theme::{self, Palette, style as tokens};
    use egui::Color32;
    use egui_kittest::Harness;

    use super::{Look, Typesetter, layer};
    use crate::Marks;
    use crate::parse::{Block, parse};

    /// One layer: `range` filled with `color`.
    fn layer_of(range: std::ops::Range<usize>, color: Color32) -> Marks {
        Marks { ranges: vec![range], color }
    }

    /// A later layer of marks is blended over an earlier one where they overlap, and the fills
    /// come relative to the part of the text asked for.
    #[test]
    fn later_layers_blend_over_earlier_ones() {
        let (red, amber) =
            (Color32::from_rgb(200, 0, 0), Color32::from_rgba_unmultiplied(250, 180, 0, 90));
        let marks = [layer_of(0..6, red), layer_of(4..8, amber)];
        let both = red.blend(amber);
        assert_eq!(layer(&marks, 0..10), [(0..4, red), (4..6, both), (6..8, amber)]);
        assert_eq!(layer(&marks, 5..7), [(0..1, both), (1..2, amber)]);
        assert_eq!(layer(&marks[..1], 6..9), [], "nothing marked there");
    }

    /// Prose, bold, links and code spans share one baseline in body text and in headings.
    #[test]
    fn mixed_fonts_share_the_baseline() {
        let mut harness =
            Harness::builder().with_context_setup(theme::fonts::install).build_ui(|ui| {
                ui.label("fonts");
            });
        harness.run();
        let ctx = harness.ctx.clone();
        let palette = Palette::LIGHT;
        let typesetter = Typesetter { palette: &palette, scale: 1.0, links: None, images: None };
        let blocks = parse("Text **bold** `code` [link](https://x.test) *it* end", None);
        let [Block::Paragraph(inline)] = blocks.as_slice() else { panic!("{blocks:?}") };
        for size in [tokens::BODY_SIZE, tokens::SECTION_SIZE, tokens::TITLE_SIZE] {
            for strong in [false, true] {
                let look = Look { size, strong, color: Palette::LIGHT.text };
                let links = typesetter.resolve_links(&inline.runs);
                let job = typesetter.layout_job(&ctx, &inline.runs, look, None, &links, &[]).job;
                let galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));
                let row = &galley.rows[0].row;
                let baselines: Vec<f32> = row.glyphs.iter().map(|glyph| glyph.pos.y).collect();
                let (min, max) = baselines
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(lo, hi), &y| (lo.min(y), hi.max(y)));
                assert!(max - min <= 1.0, "size {size}, strong {strong}: baselines {baselines:?}");
            }
        }
    }
}
