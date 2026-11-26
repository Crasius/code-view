//! The Code View mark (the app icon) at the left end of the Windows title bar, where a Windows
//! window shows its icon (a macOS title bar shows none).
//!
//! It is rasterized from the same SVG artworks as `CodeView.exe`'s icon, at the exact physical
//! pixel size it is shown at, so it stays crisp at every display scale and never drifts from the
//! taskbar icon. The artworks keep their one home in `crates/review-app/assets/icon/`. Like
//! `review-app/examples/render_icon.rs`, each size takes the smallest artwork drawn for it:
//! pixel-fitted ones up to 32 px, the master above. The background is transparent. Each size is
//! rendered once per context and cached as a texture.

use egui::{Color32, ColorImage, Context, Id, Rect, TextureHandle, TextureOptions, Ui, Vec2, pos2};

/// The largest physical pixel size each artwork is drawn for, smallest first.
const ARTWORKS: [(u32, &[u8]); 4] = [
    (20, include_bytes!("../../../../review-app/assets/icon/codeview-16.svg")),
    (24, include_bytes!("../../../../review-app/assets/icon/codeview-24.svg")),
    (32, include_bytes!("../../../../review-app/assets/icon/codeview-32.svg")),
    (u32::MAX, include_bytes!("../../../../review-app/assets/icon/codeview.svg")),
];

/// Paints the mark into `rect` (its shorter side, centred), snapped to whole physical pixels.
/// Decorative: the text beside it (the window title, the breadcrumbs) names the app.
/// `tint` multiplies it: white as drawn, translucent white to dim it (an inactive window).
pub fn paint(ui: &Ui, rect: Rect, tint: Color32) {
    let pixels_per_point = ui.ctx().pixels_per_point();
    let pixels = (rect.width().min(rect.height()) * pixels_per_point).round().max(1.0) as u32;
    let side = pixels as f32 / pixels_per_point;
    let min =
        ((rect.center() - Vec2::splat(side / 2.0)) * pixels_per_point).round() / pixels_per_point;
    let target = Rect::from_min_size(min, Vec2::splat(side));
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    ui.painter().image(texture(ui.ctx(), pixels).id(), target, uv, tint);
}

fn texture(ctx: &Context, pixels: u32) -> TextureHandle {
    let id = Id::unique(("app_mark", pixels));
    if let Some(texture) = ctx.data(|data| data.get_temp::<TextureHandle>(id)) {
        return texture;
    }
    let texture =
        ctx.load_texture(format!("app-mark-{pixels}"), render(pixels), TextureOptions::NEAREST);
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

fn artwork(pixels: u32) -> &'static [u8] {
    ARTWORKS
        .iter()
        .find(|(max, _)| pixels <= *max)
        .map(|(_, svg)| *svg)
        .expect("the master covers every size")
}

fn render(pixels: u32) -> ColorImage {
    let tree = resvg::usvg::Tree::from_data(artwork(pixels), &resvg::usvg::Options::default())
        .expect("the bundled app mark is valid SVG");
    let scale = pixels as f32 / tree.size().width();
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(pixels, pixels).expect("the mark is at least one pixel");
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    ColorImage::from_rgba_premultiplied([pixels as usize; 2], pixmap.data())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_size_takes_the_artwork_drawn_for_it() {
        let viewbox = |pixels| {
            let tree =
                resvg::usvg::Tree::from_data(artwork(pixels), &resvg::usvg::Options::default())
                    .unwrap();
            tree.size().width() as u32
        };
        assert_eq!([16, 20, 24, 32, 40, 256].map(viewbox), [16, 16, 24, 32, 64, 64]);
    }

    #[test]
    fn the_16_px_mark_is_pixel_sharp_on_a_transparent_background() {
        let image = render(16);
        let at = |x: usize, y: usize| image.pixels[y * 16 + x];
        assert_eq!(at(0, 0), Color32::TRANSPARENT, "no tile behind the mark");
        assert_eq!(
            at(2, 2),
            Color32::from_rgb(0xEF, 0x44, 0x44),
            "the removed bar fills whole pixels"
        );
        assert_eq!(
            at(2, 5),
            Color32::from_rgb(0x22, 0xC5, 0x5E),
            "the added bar fills whole pixels"
        );
        assert_eq!(at(13, 10), Color32::WHITE, "the check is drawn on whole pixels");
        assert_eq!(
            at(12, 14),
            Color32::from_rgb(0x3B, 0x82, 0xF6),
            "the badge is solid blue below the check"
        );
    }
}
