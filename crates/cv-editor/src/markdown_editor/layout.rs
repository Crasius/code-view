//! Lays out the write area's text exactly as typed: one plain format (the theme's body font and
//! text colour) for everything, and the code font for fenced code blocks so their indentation
//! lines up. Markdown is not styled while writing; Preview shows the rendered comment.
//!
//! Parse and layout are incremental per chunk of the text (see `chunks`), so an edit in a
//! 5,000-line paste lays out the chunk around the caret, and a frame without an edit reuses the
//! galley as it is.

use std::sync::Arc;

use cv_base::theme::{Palette, fonts, style};
use cv_markdown::CODE_SIZE_DELTA;
use cv_widgets::atlas_watch::AtlasWatch;
use egui::epaint::util::hash;
use egui::text::{LayoutJob, TextFormat};
use egui::{FontId, Galley, TextBuffer, Ui};

use super::chunks::Chunks;
use super::structure::Model;

/// Code is one point smaller than prose, as in the rendered view.
const CODE_SIZE: f32 = style::BODY_SIZE - CODE_SIZE_DELTA;

/// Parsed structure and galley of the editor's current text.
#[derive(Clone, Default)]
pub(crate) struct WriteCache {
    chunks: Chunks,
    /// A rebuilt font atlas invalidates the galleys kept here.
    atlas: AtlasWatch,
    /// Counts the atlas rebuilds seen through `atlas`.
    atlas_epoch: u64,
    /// The galley of the whole text, with its layout key and the text version it shows.
    galley: Option<(u64, u64, Arc<Galley>)>,
}

impl WriteCache {
    pub(crate) fn load(ui: &Ui, id: egui::Id) -> Self {
        ui.data(|data| data.get_temp(id)).unwrap_or_default()
    }

    pub(crate) fn store(self, ui: &Ui, id: egui::Id) {
        ui.data_mut(|data| data.insert_temp(id, self));
    }

    /// The structure of `text`; only the chunks an edit touched are parsed again.
    pub(crate) fn model_for(&mut self, text: &str) -> Arc<Model> {
        self.chunks.update(text);
        self.chunks.model.clone()
    }

    pub(super) fn galley_for(
        &mut self,
        ui: &Ui,
        text: &str,
        wrap_width: f32,
        palette: &Palette,
    ) -> Arc<Galley> {
        self.chunks.update(text);
        if self.atlas.is_rebuilt(ui.ctx()) {
            self.atlas_epoch += 1;
        }
        let pixels_per_point = ui.ctx().pixels_per_point();
        let key = hash((
            self.atlas_epoch,
            palette.is_dark,
            wrap_width.to_bits(),
            pixels_per_point.to_bits(),
        ));
        let version = self.chunks.version;
        if let Some((made_with, shown, galley)) = &self.galley
            && *made_with == key
            && *shown == version
        {
            return galley.clone();
        }
        let galley = self.chunks.galley(
            key,
            pixels_per_point,
            |part, part_model| {
                let mut job = build_job(part, part_model, palette);
                job.wrap.max_width = wrap_width;
                ui.fonts_mut(|fonts| fonts.layout_job(job))
            },
            // The joined galley's job only carries the text; the formats live in its rows.
            |whole| {
                let mut job = LayoutJob::default();
                job.append(whole, 0.0, text_format(palette));
                job.wrap.max_width = wrap_width;
                job
            },
        );
        self.galley = Some((key, version, galley.clone()));
        galley
    }
}

/// The layouter handed to `TextEdit`.
pub(crate) fn layouter(
    cache: &mut WriteCache,
    palette: Palette,
) -> impl FnMut(&Ui, &dyn TextBuffer, f32) -> Arc<Galley> + '_ {
    move |ui, buffer, wrap_width| cache.galley_for(ui, buffer.as_str(), wrap_width, &palette)
}

/// The text in the body format, fenced code blocks in the code format.
pub(super) fn build_job(text: &str, model: &Model, palette: &Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut written = 0;
    for code in model.code_blocks.iter().filter(|code| code.is_fenced) {
        if code.block.start > written {
            job.append(&text[written..code.block.start], 0.0, text_format(palette));
        }
        job.append(&text[code.block.clone()], 0.0, code_format(palette));
        written = code.block.end;
    }
    if written < text.len() || text.is_empty() {
        job.append(&text[written..], 0.0, text_format(palette));
    }
    job
}

fn text_format(palette: &Palette) -> TextFormat {
    TextFormat::simple(FontId::proportional(style::BODY_SIZE), palette.text)
}

fn code_format(palette: &Palette) -> TextFormat {
    TextFormat::simple(fonts::code_font(CODE_SIZE), palette.text)
}

#[cfg(test)]
mod tests {
    use cv_base::theme;
    use egui_kittest::Harness;
    use review_store::ThemePreference;

    use super::*;

    /// Every Markdown construct a renderer would style, cut into several chunks by blank lines.
    const SOURCE: &str = "## Heading\n\nSome **bold**, _italic_, ~~gone~~, `code` and [a link](https://x.test).\n\n- item\n  - nested\n- [ ] open task\n- [x] done task\n\n1. first\n\n> quoted @octocat #12 @<6A1B2C3D-0000-1111-2222-333344445555>\n\n```rust\n    fn main() {}\n```\n\n<!-- html -->\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

    #[test]
    fn write_area_shows_the_source_in_one_format() {
        let mut harness =
            Harness::builder().with_context_setup(theme::fonts::install).build_ui(|ui| {
                let palette = theme::activate(ui.ctx(), ThemePreference::Light);
                let mut cache = WriteCache::default();
                let galley = cache.galley_for(ui, SOURCE, 600.0, &palette);
                assert_eq!(galley.text(), SOURCE, "the text is shown exactly as typed");

                let fence = SOURCE.find("```rust").expect("fence")
                    ..SOURCE.rfind("```").expect("closing fence") + 3;
                let mut byte = 0;
                let (mut prose_heights, mut code_heights) = (Vec::new(), Vec::new());
                for row in &galley.rows {
                    for glyph in &row.glyphs {
                        let heights = if fence.contains(&byte) {
                            &mut code_heights
                        } else {
                            &mut prose_heights
                        };
                        heights.push(glyph.font_height.to_bits());
                        byte += glyph.chr.len_utf8();
                    }
                    byte += usize::from(row.ends_with_newline);
                    let colors = row.visuals.mesh.vertices.iter().map(|vertex| vertex.color);
                    let others: Vec<_> = colors.filter(|color| *color != palette.text).collect();
                    assert!(others.is_empty(), "no tint, dimming or panel: {others:?}");
                }
                assert_eq!(byte, SOURCE.len());
                for heights in [&mut prose_heights, &mut code_heights] {
                    heights.dedup();
                    assert_eq!(heights.len(), 1, "one font size outside and inside the fence");
                }
                assert_ne!(code_heights, prose_heights, "the fence is set in the code font");
            });
        harness.run();
    }
}
