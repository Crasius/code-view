//! Syntax token colors: VS Code Light+ and Dark+, chosen by the active palette.

use egui::Color32;

use super::Palette;

/// What a piece of code is, independent of the theme. Highlighting stores kinds, not colors,
/// so switching between light and dark never re-highlights anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TokenKind {
    Plain,
    Comment,
    Keyword,
    /// Flow control and imports (`if`, `return`, `use`): purple in VS Code.
    Control,
    String,
    Number,
    Type,
    Function,
    Variable,
    Constant,
    /// Object and mapping keys (JSON keys, object literals).
    Property,
    Tag,
    /// Markup attributes and CSS property names.
    Attribute,
    /// CSS selectors.
    Selector,
    Regex,
    Escape,
    Operator,
    Punctuation,
    Heading,
    /// Markdown: bold text, as One Light and One Dark color it (the code font has no bold face).
    Strong,
    /// Markdown: italic text.
    Emphasis,
    /// Markdown structure: list bullets, quote markers, emphasis and fence delimiters, table pipes.
    Markup,
}

impl TokenKind {
    pub const ALL: [TokenKind; 22] = [
        TokenKind::Plain,
        TokenKind::Comment,
        TokenKind::Keyword,
        TokenKind::Control,
        TokenKind::String,
        TokenKind::Number,
        TokenKind::Type,
        TokenKind::Function,
        TokenKind::Variable,
        TokenKind::Constant,
        TokenKind::Property,
        TokenKind::Tag,
        TokenKind::Attribute,
        TokenKind::Selector,
        TokenKind::Regex,
        TokenKind::Escape,
        TokenKind::Operator,
        TokenKind::Punctuation,
        TokenKind::Heading,
        TokenKind::Strong,
        TokenKind::Emphasis,
        TokenKind::Markup,
    ];

    pub fn from_index(index: u8) -> TokenKind {
        Self::ALL.get(usize::from(index)).copied().unwrap_or(TokenKind::Plain)
    }
}

/// Text color of `kind` on the diff surface.
pub fn color(kind: TokenKind, palette: &Palette) -> Color32 {
    if palette.is_dark { dark_plus(kind) } else { light_plus(kind, palette) }
}

fn light_plus(kind: TokenKind, palette: &Palette) -> Color32 {
    match kind {
        TokenKind::Plain | TokenKind::Operator => palette.text,
        TokenKind::Comment => rgb(0x00_80_00),
        TokenKind::Keyword => rgb(0x00_00_FF),
        TokenKind::Control => rgb(0xAF_00_DB),
        TokenKind::String => rgb(0xA3_15_15),
        TokenKind::Number => rgb(0x09_86_58),
        TokenKind::Type => rgb(0x26_7F_99),
        TokenKind::Function => rgb(0x79_5E_26),
        TokenKind::Variable => rgb(0x00_10_80),
        TokenKind::Constant => rgb(0x00_70_C1),
        TokenKind::Property | TokenKind::Markup => rgb(0x04_51_A5),
        TokenKind::Tag | TokenKind::Selector | TokenKind::Punctuation | TokenKind::Heading => {
            rgb(0x80_00_00)
        }
        TokenKind::Attribute => rgb(0xE5_00_00),
        TokenKind::Regex => rgb(0x81_1F_3F),
        TokenKind::Escape => rgb(0xEE_00_00),
        TokenKind::Strong => rgb(0x98_68_01),
        TokenKind::Emphasis => rgb(0xA6_26_A4),
    }
}

fn dark_plus(kind: TokenKind) -> Color32 {
    match kind {
        TokenKind::Plain | TokenKind::Operator => rgb(0xD4_D4_D4),
        TokenKind::Comment => rgb(0x6A_99_55),
        TokenKind::Keyword | TokenKind::Tag | TokenKind::Heading => rgb(0x56_9C_D6),
        TokenKind::Control => rgb(0xC5_86_C0),
        TokenKind::String => rgb(0xCE_91_78),
        TokenKind::Number => rgb(0xB5_CE_A8),
        TokenKind::Type => rgb(0x4E_C9_B0),
        TokenKind::Function => rgb(0xDC_DC_AA),
        TokenKind::Variable | TokenKind::Property | TokenKind::Attribute => rgb(0x9C_DC_FE),
        TokenKind::Constant => rgb(0x4F_C1_FF),
        TokenKind::Selector | TokenKind::Escape => rgb(0xD7_BA_7D),
        TokenKind::Regex => rgb(0xD1_69_69),
        TokenKind::Punctuation => rgb(0x80_80_80),
        TokenKind::Strong => rgb(0xD1_9A_66),
        TokenKind::Emphasis => rgb(0xC6_78_DD),
        TokenKind::Markup => rgb(0x67_96_E6),
    }
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[cfg(test)]
mod tests {
    use super::TokenKind;

    #[test]
    fn kinds_round_trip_through_their_index() {
        for kind in TokenKind::ALL {
            assert_eq!(TokenKind::from_index(kind as u8), kind);
        }
        assert_eq!(TokenKind::from_index(200), TokenKind::Plain);
    }
}
