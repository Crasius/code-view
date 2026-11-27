//! Visual identity: semantic color tokens, fonts, widget style and icon codepoints.

pub mod fonts;
pub mod icons;
pub mod motion;
pub mod palette;
pub mod style;
pub mod syntax;

use egui::Id;
pub use palette::{FileHue, Palette};
use review_store::ThemePreference;

const PALETTE_ID: &str = "review.palette";

/// The palette for this frame, set once per frame by [`activate`].
pub fn palette(ctx: &egui::Context) -> Palette {
    ctx.data(|data| data.get_temp(Id::unique(PALETTE_ID))).unwrap_or(Palette::LIGHT)
}

/// Picks the palette from the preference (following the OS for `System`) and applies it when it changed.
pub fn activate(ctx: &egui::Context, preference: ThemePreference) -> Palette {
    let wants_dark = match preference {
        ThemePreference::Dark => true,
        ThemePreference::Light => false,
        ThemePreference::System => ctx.system_theme() == Some(egui::Theme::Dark),
    };
    let palette = if wants_dark { Palette::DARK } else { Palette::LIGHT };
    let current: Option<Palette> = ctx.data(|data| data.get_temp(Id::unique(PALETTE_ID)));
    if current != Some(palette) {
        style::apply(ctx, &palette);
        ctx.data_mut(|data| data.insert_temp(Id::unique(PALETTE_ID), palette));
    }
    palette
}
