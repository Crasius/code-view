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
    pub warning: Color32,
    pub warning_surface: Color32,
    pub error: Color32,
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
    pub const COUNT: usize = 11;
    /// For the contrast check of every hue.
    #[cfg(any(test, feature = "test-support"))]
    pub const ALL: [FileHue; FileHue::COUNT] = [
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
        warning: Color32::from_rgb(0x8A, 0x5A, 0x00),
        warning_surface: Color32::from_rgb(0xFF, 0xF4, 0xCE),
        error: Color32::from_rgb(0xC4, 0x31, 0x4B),
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
        warning: Color32::from_rgb(0xFC, 0xE1, 0x00),
        warning_surface: Color32::from_rgb(0x43, 0x35, 0x19),
        error: Color32::from_rgb(0xF1, 0x70, 0x7B),
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
