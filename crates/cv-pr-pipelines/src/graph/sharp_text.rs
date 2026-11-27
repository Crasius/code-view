//! Crisp text on a zoomed layer. egui scales a transformed layer's shapes when it tessellates
//! them, text included, so the glyphs of a 14 px label zoomed to 200 % would be its 14 px bitmaps
//! stretched. After a frame's shapes are on the layer, [`sharpen`] swaps each text's galley for
//! one laid out at its size on screen and shrunk back by the zoom: the layer's transform then
//! scales it to exactly that size, so every glyph is rasterized for the pixels it covers.
//!
//! Re-laid-out galleys are cached per source galley and zoom level and dropped when unused for a
//! frame, so a canvas whose zoom stays put lays out nothing new. While the zoom moves, text uses
//! the nearest of a few sizes per octave instead of a new size every frame, which keeps the glyph
//! atlas small; once it rests the text is laid out at the exact zoom.

use std::collections::HashMap;
use std::sync::Arc;

use egui::emath::TSTransform;
use egui::epaint::FontsView;
use egui::epaint::TextShape;
use egui::layers::ShapeIdx;
use egui::text::LayoutJob;
use egui::{Color32, Context, Galley, Id, LayerId, Pos2, Rect, Shape};

/// Text sizes per octave of zoom while the zoom moves: each is at most 4.4 % off the size shown.
const SIZES_PER_OCTAVE: f32 = 8.0;

/// Re-laid-out galleys of the current zoom level, keyed by the source galley.
#[derive(Clone, Default)]
struct Cache {
    /// Bits of the text zoom and of `pixels_per_point` the galleys were laid out for.
    level: (u32, u32),
    /// Source galley (kept alive, so its address stays unique) and its replacement.
    galleys: HashMap<usize, (Arc<Galley>, Arc<Galley>)>,
}

/// The zoom text is laid out for: `zoom` itself at rest, a nearby fixed size while it moves.
pub fn text_zoom(zoom: f32, is_moving: bool) -> f32 {
    if is_moving {
        ((zoom.log2() * SIZES_PER_OCTAVE).round() / SIZES_PER_OCTAVE).exp2()
    } else {
        zoom
    }
}

/// Lays the text on `layer` out again for `zoom` (the scale of the layer's transform) at the
/// text zoom `text`. `id` keys the cache; at a zoom of 1 the text already fits and the cache
/// is dropped.
pub fn sharpen(ctx: &Context, layer: LayerId, id: Id, zoom: f32, text: f32) {
    let cache_id = id.with("sharp_text");
    // Zoom steps and glides end on exactly 1.0, so only an exact 1.0 is unscaled.
    let is_unscaled = |scale: f32| scale.to_bits() == 1.0_f32.to_bits();
    if is_unscaled(zoom) || is_unscaled(text) {
        ctx.data_mut(|data| data.remove_temp::<Cache>(cache_id));
        return;
    }
    let mut shapes = take_text_shapes(ctx, layer);
    if shapes.is_empty() {
        return;
    }
    let mut cache: Cache =
        ctx.data_mut(|data| std::mem::take(data.get_temp_mut_or_default::<Cache>(cache_id)));
    let level = (text.to_bits(), ctx.pixels_per_point().to_bits());
    let mut previous = std::mem::take(&mut cache.galleys);
    if cache.level != level {
        previous.clear();
        cache.level = level;
    }
    ctx.fonts_mut(|fonts| {
        for (_, _, shape) in &mut shapes {
            each_text(shape, &mut |text_shape| {
                // egui hands out one galley for equal texts, so a key may come up twice a frame.
                let key = Arc::as_ptr(&text_shape.galley) as usize;
                let entry = cache.galleys.entry(key).or_insert_with(|| {
                    previous.remove(&key).unwrap_or_else(|| {
                        let source = text_shape.galley.clone();
                        let sharp = relayout(fonts, &source, text);
                        (source, sharp)
                    })
                });
                text_shape.galley = entry.1.clone();
            });
        }
    });
    ctx.data_mut(|data| data.insert_temp(cache_id, cache));
    ctx.graphics_mut(|graphics| {
        let list = graphics.entry(layer);
        for (index, clip, shape) in shapes {
            list.set(index, clip, shape);
        }
    });
}

/// Takes every shape holding text off `layer`, leaving a no-op in its place.
fn take_text_shapes(ctx: &Context, layer: LayerId) -> Vec<(ShapeIdx, Rect, Shape)> {
    ctx.graphics_mut(|graphics| {
        let list = graphics.entry(layer);
        let mut taken = Vec::new();
        for index in 0..list.next_idx().0 {
            list.mutate_shape(ShapeIdx(index), |clipped| {
                if has_text(&clipped.shape) {
                    taken.push((
                        ShapeIdx(index),
                        clipped.clip_rect,
                        std::mem::replace(&mut clipped.shape, Shape::Noop),
                    ));
                }
            });
        }
        taken
    })
}

fn has_text(shape: &Shape) -> bool {
    match shape {
        Shape::Text(_) => true,
        Shape::Vec(shapes) => shapes.iter().any(has_text),
        _ => false,
    }
}

fn each_text(shape: &mut Shape, visit: &mut impl FnMut(&mut TextShape)) {
    match shape {
        Shape::Text(text) => visit(text),
        Shape::Vec(shapes) => shapes.iter_mut().for_each(|shape| each_text(shape, visit)),
        _ => {}
    }
}

/// `source` laid out at `zoom` times its size, then shrunk by `zoom` without touching its glyphs.
fn relayout(fonts: &mut FontsView<'_>, source: &Galley, zoom: f32) -> Arc<Galley> {
    let mut shape = TextShape::new(
        Pos2::ZERO,
        fonts.layout_job(scaled_job(&source.job, zoom)),
        Color32::PLACEHOLDER,
    );
    shape.transform(TSTransform::from_scaling(1.0 / zoom));
    shape.galley
}

/// `job` with every length in it multiplied by `zoom`.
fn scaled_job(job: &LayoutJob, zoom: f32) -> LayoutJob {
    let mut job = job.clone();
    for section in &mut job.sections {
        section.leading_space *= zoom;
        let format = &mut section.format;
        format.font_id.size *= zoom;
        format.extra_letter_spacing *= zoom;
        format.expand_bg *= zoom;
        format.line_height = format.line_height.map(|height| height * zoom);
        format.underline.width *= zoom;
        format.strikethrough.width *= zoom;
    }
    job.wrap.max_width *= zoom;
    job.first_row_min_height *= zoom;
    job
}

#[cfg(test)]
mod tests {
    use egui::FontId;
    use egui::text::TextWrapping;

    use super::*;

    #[test]
    fn moving_text_snaps_to_eighth_octaves_and_resting_text_is_exact() {
        assert_eq!(text_zoom(0.83, false), 0.83);
        let moving = text_zoom(0.83, true);
        assert_eq!(moving, 2f32.powf(-2.0 / 8.0));
        assert!((moving / 0.83 - 1.0).abs() < 0.045);
        assert_eq!(text_zoom(1.0, true), 1.0);
        assert_eq!(text_zoom(2.0, true), 2.0);
    }

    #[test]
    fn a_relaid_galley_keeps_its_size_and_cut() {
        let ctx = Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut job = LayoutJob::simple_singleline(
                "Infra Apps (integration environment)".to_owned(),
                FontId::proportional(14.0),
                Color32::WHITE,
            );
            job.wrap = TextWrapping::truncate_at_width(120.0);
            ui.ctx().fonts_mut(|fonts| {
                let source = fonts.layout_job(job);
                for zoom in [0.25, 0.5, 2.0] {
                    let sharp = relayout(fonts, &source, zoom);
                    assert!(sharp.elided, "still cut at {zoom}");
                    // The layout, shrunk back, takes the room of the original (to hinting).
                    assert!(
                        (sharp.size().x - source.size().x).abs() < 3.0 / zoom,
                        "width at {zoom}: {} vs {}",
                        sharp.size().x,
                        source.size().x
                    );
                    assert!(
                        (sharp.size().y - source.size().y).abs() < 2.0 / zoom,
                        "height at {zoom}"
                    );
                }
            });
        });
        // No renderer here to take the glyph atlas.
        output.drop_without_applying_deltas();
    }
}
