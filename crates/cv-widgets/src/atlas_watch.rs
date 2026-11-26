//! Notices when egui rebuilt its font atlas, for caches that keep galleys across frames.
//!
//! A galley holds texture coordinates into the atlas. egui starts the atlas over when it runs full
//! and when the text options change (the GPU's texture limit, hinting), and then drops every galley
//! it cached; one kept elsewhere paints whatever glyphs now sit at its old coordinates: fragments
//! of other letters, or nothing. The atlas's fill ratio does not show a rebuild reliably (by the
//! time a cache looks, the new atlas may have refilled to the same row), egui's own galley cache
//! does: a tiny galley laid out every frame comes back as the same allocation until that cache is
//! dropped.

use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Color32, Context, FontId, Galley};

#[derive(Clone, Default)]
pub struct AtlasWatch {
    probe: Option<Arc<Galley>>,
}

impl AtlasWatch {
    /// True on the first call, and when the atlas was rebuilt since the last one (also after a
    /// frame without a call, as egui drops galleys a pass did not use). Call once per frame.
    pub fn is_rebuilt(&mut self, ctx: &Context) -> bool {
        let job = LayoutJob::simple_singleline(
            "probe".to_owned(),
            FontId::proportional(10.0),
            Color32::PLACEHOLDER,
        );
        let probe = ctx.fonts_mut(|fonts| fonts.layout_job(job));
        let is_same = self.probe.as_ref().is_some_and(|previous| Arc::ptr_eq(previous, &probe));
        self.probe = Some(probe);
        !is_same
    }
}

#[cfg(test)]
mod tests {
    use egui::{Color32, Context, FontDefinitions, FontFamily, FontId, RawInput};

    use super::AtlasWatch;

    /// egui starts a fresh atlas (here: new font definitions, as at start-up). The same text fills
    /// it to the same row before the cache looks again, so the fill ratio cannot tell; the watch
    /// does.
    #[test]
    fn sees_an_atlas_rebuilt_to_the_same_fill() {
        let ctx = Context::default();
        let mut watch = AtlasWatch::default();
        let mut fills = Vec::new();
        let mut rebuilt = Vec::new();
        for frame in 0..3 {
            if frame == 2 {
                // The same faces with one more family: egui builds its fonts and atlas anew.
                let mut fonts = FontDefinitions::default();
                fonts.families.insert(FontFamily::Name("extra".into()), vec!["Hack".to_owned()]);
                ctx.set_fonts(fonts);
            }
            let mut output = ctx.run_ui(RawInput::default(), |ui| {
                let text = "Pull requests › contoso/app › !42".to_owned();
                ui.fonts_mut(|fonts| {
                    fonts.layout_no_wrap(text, FontId::proportional(14.0), Color32::WHITE)
                });
                #[expect(
                    clippy::redundant_closure_for_method_calls,
                    reason = "the suggested method path does not type-check: `fonts` lends a \
                              higher-ranked borrow"
                )]
                fills.push(ui.ctx().fonts(|fonts| fonts.font_atlas_fill_ratio()));
                rebuilt.push(watch.is_rebuilt(ui.ctx()));
            });
            // No renderer here to take the atlas.
            output.textures_delta.clear();
        }
        assert_eq!(rebuilt, [true, false, true], "first look, steady frame, new atlas");
        assert!(
            fills[2] >= fills[1],
            "the fill ratio does not drop, so it misses the rebuild: {fills:?}"
        );
    }
}
