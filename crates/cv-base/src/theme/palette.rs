use egui::Color32;

/// Semantic color tokens. Widgets never use raw colors; they ask for a role.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub is_dark: bool,
    /// Page background behind cards and lists.
    pub ground: Color32,
    /// Cards, bars, inputs: the raised layer.
    pub surface: Color32,
    pub surface_subtle: Color32,
    pub rail: Color32,
    pub rail_text: Color32,
    /// Hairline dividers and card outlines.
    pub border: Color32,
    /// Outlines of controls (inputs, secondary buttons) that must read as interactive.
    pub border_strong: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_disabled: Color32,
    /// Links, indicators and accent text; readable on `surface`.
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    /// Accent for text on `selection` fills, dark enough for 4.5:1 there.
    pub accent_text: Color32,
    /// Fill of primary buttons and solid state chips, with `on_brand` text.
    pub brand: Color32,
    pub brand_hover: Color32,
    pub brand_pressed: Color32,
    pub on_brand: Color32,
    pub selection: Color32,
    /// Behind selected text in code (the diff): a clear blue under the syntax colors, opaque so
    /// the block reads the same over plain, added and removed lines.
    pub text_selection: Color32,
    /// Behind the other occurrences of the text selected in code: a translucent grey-blue that
    /// reads on plain, added and removed lines and apart from the find widget's amber hits, which
    /// show at the same time (see `find::paint`).
    pub occurrence: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub success: Color32,
    /// Behind a success message, with `success` text on it.
    pub success_surface: Color32,
    /// The fill of a resolved badge, with the `success_on_badge` check on it.
    pub success_badge: Color32,
    pub success_on_badge: Color32,
    pub warning: Color32,
    pub warning_surface: Color32,
    /// Warning text on `warning_surface`: the dark theme's `warning` is too bright for small text.
    pub warning_text: Color32,
    pub error: Color32,
    /// Behind an error message, with `error` text on it.
    pub error_surface: Color32,
    /// ANSI magenta in pipeline logs: 4.5:1 or more on `ground` and `surface`.
    pub log_magenta: Color32,
    /// The filled star of a favorite repository: amber, 3:1 or more on `surface`, `hover` and `selection`.
    pub favorite: Color32,
    /// Azure DevOps' brand blue for its platform mark: 3:1 or more on `surface`, `hover` and `selection`.
    pub platform_azure: Color32,
    /// GitLab's tanuki orange for its platform mark, same contrast. GitHub's mark takes `text`.
    pub platform_gitlab: Color32,
    /// Base color of elevation shadows.
    pub shadow: Color32,
    pub diff_added_line: Color32,
    pub diff_added_word: Color32,
    pub diff_deleted_line: Color32,
    pub diff_deleted_word: Color32,
    pub diff_hunk: Color32,
    pub diff_gutter: Color32,
    pub diff_empty: Color32,
    /// Tints of file-type icons, indexed by [`FileHue`]: at least 3:1 on `surface`, `hover`,
    /// `pressed` and `selection`, so an icon stays legible on every state of a tree row.
    pub file_hues: [Color32; FileHue::COUNT],
}

/// Color family of a file-type icon: a language or format and its relatives share one, so a
/// Rust crate reads orange from source to manifest to lock file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileHue {
    Red,
    Orange,
    Amber,
    Yellow,
    Green,
    Teal,
    Cyan,
    Blue,
    Purple,
    Pink,
    /// Plain text, binaries and unknown files.
    Slate,
}

impl FileHue {
    pub(crate) const COUNT: usize = 11;
    /// For the contrast check of every hue.
    #[cfg(test)]
    pub(crate) const ALL: [FileHue; FileHue::COUNT] = [
        FileHue::Red,
        FileHue::Orange,
        FileHue::Amber,
        FileHue::Yellow,
        FileHue::Green,
        FileHue::Teal,
        FileHue::Cyan,
        FileHue::Blue,
        FileHue::Purple,
        FileHue::Pink,
        FileHue::Slate,
    ];
}

impl Palette {
    pub fn file_hue(&self, hue: FileHue) -> Color32 {
        self.file_hues[hue as usize]
    }

    pub const LIGHT: Palette = Palette {
        is_dark: false,
        ground: Color32::from_rgb(0xF7, 0xF7, 0xF7),
        surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        surface_subtle: Color32::from_rgb(0xF5, 0xF5, 0xF5),
        rail: Color32::from_rgb(0xEE, 0xEE, 0xEE),
        rail_text: Color32::from_rgb(0x42, 0x42, 0x42),
        border: Color32::from_rgb(0xE5, 0xE5, 0xE5),
        border_strong: Color32::from_rgb(0xC7, 0xC7, 0xC7),
        text: Color32::from_rgb(0x24, 0x24, 0x24),
        text_secondary: Color32::from_rgb(0x61, 0x61, 0x61),
        text_disabled: Color32::from_rgb(0x8F, 0x8F, 0x8F),
        accent: Color32::from_rgb(0x00, 0x78, 0xD4),
        accent_hover: Color32::from_rgb(0x10, 0x6E, 0xBE),
        on_accent: Color32::WHITE,
        accent_text: Color32::from_rgb(0x00, 0x5A, 0x9E),
        brand: Color32::from_rgb(0x00, 0x78, 0xD4),
        brand_hover: Color32::from_rgb(0x10, 0x6E, 0xBE),
        brand_pressed: Color32::from_rgb(0x00, 0x5A, 0x9E),
        on_brand: Color32::WHITE,
        selection: Color32::from_rgb(0xEB, 0xF3, 0xFC),
        text_selection: Color32::from_rgb(0xAD, 0xD6, 0xFF),
        occurrence: Color32::from_rgba_premultiplied(21, 33, 51, 76),
        hover: Color32::from_rgb(0xF2, 0xF2, 0xF2),
        pressed: Color32::from_rgb(0xE6, 0xE6, 0xE6),
        success: Color32::from_rgb(0x10, 0x7C, 0x10),
        success_surface: Color32::from_rgb(0xE6, 0xF4, 0xEA),
        // `success` at 18 % (`gamma_multiply(0.18)`) over the white `surface`.
        success_badge: Color32::from_rgb(0xD4, 0xE7, 0xD4),
        success_on_badge: Color32::from_rgb(0x10, 0x7C, 0x10),
        warning: Color32::from_rgb(0x8A, 0x5A, 0x00),
        warning_surface: Color32::from_rgb(0xFF, 0xF4, 0xCE),
        warning_text: Color32::from_rgb(0x8A, 0x5A, 0x00),
        error: Color32::from_rgb(0xC4, 0x31, 0x4B),
        error_surface: Color32::from_rgb(0xFB, 0xE9, 0xEB),
        log_magenta: Color32::from_rgb(0x9C, 0x2F, 0xB0),
        favorite: Color32::from_rgb(0xB4, 0x53, 0x09),
        platform_azure: Color32::from_rgb(0x00, 0x78, 0xD4),
        platform_gitlab: Color32::from_rgb(0xE2, 0x43, 0x29),
        shadow: Color32::from_black_alpha(36),
        diff_added_line: Color32::from_rgb(0xE6, 0xF4, 0xEA),
        diff_added_word: Color32::from_rgb(0xAB, 0xE2, 0xB8),
        diff_deleted_line: Color32::from_rgb(0xFB, 0xE9, 0xEB),
        diff_deleted_word: Color32::from_rgb(0xF4, 0xB9, 0xC1),
        diff_hunk: Color32::from_rgb(0xF3, 0xF2, 0xF1),
        diff_gutter: Color32::from_rgb(0x6E, 0x6E, 0x6E),
        diff_empty: Color32::from_rgb(0xF6, 0xF6, 0xF6),
        // Deep 700-level tints: saturated enough to tell apart, dark enough for the light rows.
        file_hues: [
            Color32::from_rgb(0xC4, 0x31, 0x4B),
            Color32::from_rgb(0xC2, 0x41, 0x0C),
            Color32::from_rgb(0xB4, 0x53, 0x09),
            Color32::from_rgb(0x94, 0x62, 0x00),
            Color32::from_rgb(0x1F, 0x7A, 0x3A),
            Color32::from_rgb(0x0F, 0x76, 0x6E),
            Color32::from_rgb(0x0E, 0x74, 0x90),
            Color32::from_rgb(0x1F, 0x5F, 0xD1),
            Color32::from_rgb(0x74, 0x48, 0xD6),
            Color32::from_rgb(0xBE, 0x18, 0x5D),
            Color32::from_rgb(0x5B, 0x66, 0x70),
        ],
    };

    pub const DARK: Palette = Palette {
        is_dark: true,
        ground: Color32::from_rgb(0x1A, 0x1A, 0x1A),
        surface: Color32::from_rgb(0x24, 0x24, 0x24),
        surface_subtle: Color32::from_rgb(0x2C, 0x2C, 0x2C),
        rail: Color32::from_rgb(0x14, 0x14, 0x14),
        rail_text: Color32::from_rgb(0xD6, 0xD6, 0xD6),
        border: Color32::from_rgb(0x36, 0x36, 0x36),
        border_strong: Color32::from_rgb(0x52, 0x52, 0x52),
        text: Color32::from_rgb(0xF0, 0xF0, 0xF0),
        text_secondary: Color32::from_rgb(0xAD, 0xAD, 0xAD),
        text_disabled: Color32::from_rgb(0x7A, 0x7A, 0x7A),
        accent: Color32::from_rgb(0x4C, 0xA2, 0xF0),
        accent_hover: Color32::from_rgb(0x6C, 0xB4, 0xF4),
        on_accent: Color32::from_rgb(0x10, 0x10, 0x10),
        accent_text: Color32::from_rgb(0x6C, 0xB4, 0xF4),
        brand: Color32::from_rgb(0x10, 0x6E, 0xBE),
        brand_hover: Color32::from_rgb(0x18, 0x7B, 0xCF),
        brand_pressed: Color32::from_rgb(0x0C, 0x5A, 0x9C),
        on_brand: Color32::WHITE,
        selection: Color32::from_rgb(0x17, 0x33, 0x4F),
        text_selection: Color32::from_rgb(0x26, 0x4F, 0x78),
        occurrence: Color32::from_rgba_premultiplied(42, 51, 64, 76),
        hover: Color32::from_rgb(0x2F, 0x2F, 0x2F),
        pressed: Color32::from_rgb(0x38, 0x38, 0x38),
        success: Color32::from_rgb(0x54, 0xB0, 0x54),
        success_surface: Color32::from_rgb(0x1F, 0x33, 0x24),
        success_badge: Color32::from_rgb(0x2E, 0x5D, 0x33),
        success_on_badge: Color32::from_rgb(0x9F, 0xE0, 0x9F),
        warning: Color32::from_rgb(0xFC, 0xE1, 0x00),
        warning_surface: Color32::from_rgb(0x43, 0x35, 0x19),
        warning_text: Color32::from_rgb(0xE8, 0xC5, 0x5A),
        error: Color32::from_rgb(0xF1, 0x70, 0x7B),
        error_surface: Color32::from_rgb(0x3D, 0x23, 0x26),
        log_magenta: Color32::from_rgb(0xD9, 0x7C, 0xE6),
        favorite: Color32::from_rgb(0xF5, 0xB5, 0x4A),
        platform_azure: Color32::from_rgb(0x3A, 0x96, 0xDD),
        platform_gitlab: Color32::from_rgb(0xFC, 0x6D, 0x26),
        shadow: Color32::from_black_alpha(110),
        diff_added_line: Color32::from_rgb(0x1F, 0x33, 0x24),
        diff_added_word: Color32::from_rgb(0x2E, 0x5A, 0x37),
        diff_deleted_line: Color32::from_rgb(0x3D, 0x23, 0x26),
        diff_deleted_word: Color32::from_rgb(0x6E, 0x2E, 0x36),
        diff_hunk: Color32::from_rgb(0x2D, 0x2C, 0x2B),
        diff_gutter: Color32::from_rgb(0x8A, 0x88, 0x86),
        diff_empty: Color32::from_rgb(0x21, 0x20, 0x1F),
        // Softened 400-level tints: light enough for the selected row, never neon on dark grey.
        file_hues: [
            Color32::from_rgb(0xF4, 0x7C, 0x7C),
            Color32::from_rgb(0xF5, 0x9A, 0x5B),
            Color32::from_rgb(0xF5, 0xB5, 0x4A),
            Color32::from_rgb(0xE8, 0xCF, 0x5A),
            Color32::from_rgb(0x5C, 0xC7, 0x7E),
            Color32::from_rgb(0x3C, 0xC8, 0xB4),
            Color32::from_rgb(0x4C, 0xC3, 0xE0),
            Color32::from_rgb(0x6A, 0xA6, 0xF8),
            Color32::from_rgb(0xAE, 0x94, 0xF6),
            Color32::from_rgb(0xF0, 0x7A, 0xB4),
            Color32::from_rgb(0xA3, 0xAE, 0xB8),
        ],
    };
}

/// The WCAG 2 contrast ratio of two opaque colours, 1.0 to 21.0.
#[cfg(any(test, feature = "test-support"))]
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let luminance = |color: Color32| {
        let linear = egui::Rgba::from(color);
        0.2126 * linear.r() + 0.7152 * linear.g() + 0.0722 * linear.b()
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `color` painted over the opaque `background`, blended in gamma space as egui paints.
#[cfg(any(test, feature = "test-support"))]
pub fn over(color: Color32, background: Color32) -> Color32 {
    let keep = 1.0 - f32::from(color.a()) / 255.0;
    let blend = |top: u8, under: u8| (f32::from(top) + f32::from(under) * keep).round() as u8;
    Color32::from_rgb(
        blend(color.r(), background.r()),
        blend(color.g(), background.g()),
        blend(color.b(), background.b()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every hue keeps 3:1 (WCAG non-text contrast) on each fill a tree row can have.
    #[test]
    fn every_hue_has_three_to_one_contrast_on_tree_rows() {
        for palette in [Palette::LIGHT, Palette::DARK] {
            for hue in FileHue::ALL {
                for (row, background) in [
                    ("surface", palette.surface),
                    ("hover", palette.hover),
                    ("pressed", palette.pressed),
                    ("selection", palette.selection),
                ] {
                    let ratio = contrast_ratio(palette.file_hue(hue), background);
                    assert!(
                        ratio >= 3.0,
                        "{hue:?} on {row} (dark {}): {ratio:.2}",
                        palette.is_dark
                    );
                }
            }
        }
    }

    /// Text on a tinted surface keeps 4.5:1 (WCAG text contrast); the check on a resolved badge
    /// is a glyph and keeps 3:1.
    #[test]
    fn tinted_surfaces_keep_their_foreground_readable() {
        for palette in [Palette::LIGHT, Palette::DARK] {
            for (pair, foreground, background, least) in [
                ("success text", palette.success, palette.success_surface, 4.5),
                ("error text", palette.error, palette.error_surface, 4.5),
                ("warning text", palette.warning_text, palette.warning_surface, 4.5),
                ("log magenta on ground", palette.log_magenta, palette.ground, 4.5),
                ("log magenta on surface", palette.log_magenta, palette.surface, 4.5),
                ("resolved check", palette.success_on_badge, palette.success_badge, 3.0),
            ] {
                let ratio = contrast_ratio(foreground, background);
                assert!(ratio >= least, "{pair} (dark {}): {ratio:.2}", palette.is_dark);
            }
        }
    }

    #[test]
    fn contrast_ratio_spans_one_to_twenty_one() {
        assert!((contrast_ratio(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(Color32::WHITE, Color32::BLACK) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(Palette::LIGHT.text, Palette::LIGHT.text) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn over_blends_a_premultiplied_colour_onto_its_background() {
        let light = Palette::LIGHT;
        assert_eq!(over(light.success.gamma_multiply(0.18), light.surface), light.success_badge);
        assert_eq!(over(light.text, light.surface), light.text, "opaque: the colour itself");
        assert_eq!(over(Color32::TRANSPARENT, light.selection), light.selection);
    }
}
