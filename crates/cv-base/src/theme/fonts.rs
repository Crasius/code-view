//! System fonts read at start-up instead of being embedded (the platform names them, see
//! [`platform::font_sources`]: Segoe UI Variable and Cascadia Mono on Windows, San Francisco and
//! SF Mono or Menlo on macOS, Cantarell or Ubuntu or Noto Sans and DejaVu Sans Mono on Linux), plus
//! the Phosphor icon font as a fallback in every family so icons render inline. A variable UI font
//! serves both weights from one file, so it is loaded once.
//!
//! Every family ends with the same fallbacks: Phosphor, the system's emoji and symbol fonts where it
//! has them, then egui's bundled monochrome emoji fonts. So a system without symbol or emoji fonts
//! (a bare Linux, a test machine) still draws "•", "→", "✓" and "🚀" in every family, and a chain
//! never names a font whose file was not found.
//!
//! Phosphor's Regular, Bold and Fill weights are bundled. They share one set of codepoints, so only
//! one weight can be the inline fallback: Bold, the resting weight of icons at text size (see
//! [`super::icons`]). Each weight also gets its own family ([`super::icons::Weight::family`]) that
//! icon painters select explicitly.

use std::sync::Arc;

use egui::epaint::text::{FontTweak, VariationCoords};
use egui::{FontData, FontDefinitions, FontFamily, FontId};

use super::icons::Weight;
use crate::platform::{self, FontFace, FontSources, VariableUiFont};

/// The bundled Phosphor weights: (weight, font name, bytes).
const ICON_FONTS: [(Weight, &str, &[u8]); 3] = [
    (Weight::Regular, "phosphor", include_bytes!("../../assets/Phosphor.ttf")),
    (Weight::Bold, "phosphor-bold", include_bytes!("../../assets/Phosphor-Bold.ttf")),
    (Weight::Fill, "phosphor-fill", include_bytes!("../../assets/Phosphor-Fill.ttf")),
];
/// The weight inline icons (`"{icon}  Label"`) fall back to.
const INLINE_ICON_FONT: &str = "phosphor-bold";
/// egui's bundled monochrome emoji fonts (the `monochrome_emoji_fonts` feature): the last resort
/// of every family, after the system's own emoji and symbol fonts.
const BUNDLED_EMOJI_FONTS: [&str; 2] = ["NotoEmoji-Regular", "emoji-icon-font"];
/// egui's bundled monospace face, which also has the arrows, bullets and shapes Ubuntu Light lacks.
const BUNDLED_SYMBOL_FONT: &str = "Hack";
const SEMIBOLD_FAMILY: &str = "semibold";
/// The two instances of a variable UI font.
const REGULAR_WEIGHT: f32 = 400.0;
const SEMIBOLD_WEIGHT: f32 = 600.0;

pub fn semibold() -> FontFamily {
    FontFamily::Name(SEMIBOLD_FAMILY.into())
}

pub fn semibold_font(size: f32) -> FontId {
    FontId::new(size, semibold())
}

pub fn code_font(size: f32) -> FontId {
    FontId::monospace(size)
}

/// How far below the top of a line of `font` its words read as centred: midway between the cap
/// height and the baseline, so descenders, dots and the font's line gap leave it out. Text
/// centred on its line box sits low instead (Segoe UI at 12 px: a 16 px box, its baseline at 13
/// and its capitals from 5, so 1 px), and a chip around it shows less room below the words than
/// above them. Half the line's height for a font without an "H".
pub fn optical_center(painter: &egui::Painter, font: &FontId) -> f32 {
    let probe = painter.layout_no_wrap("H".to_owned(), font.clone(), egui::Color32::WHITE);
    let glyph =
        probe.rows.first().and_then(|placed| Some((placed.pos.y, placed.row.glyphs.first()?)));
    match glyph {
        Some((row_top, glyph)) if glyph.uv_rect.size.y > 0.0 => {
            let baseline = row_top + glyph.pos.y;
            (baseline + glyph.uv_rect.offset.y).midpoint(baseline)
        }
        _ => probe.size().y / 2.0,
    }
}

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions(&platform::font_sources()));
}

/// The app's fonts and family chains for the system fonts `sources` names (those that can be read).
pub fn definitions(sources: &FontSources) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let without_bundled_emoji = |family: &FontFamily| -> Vec<String> {
        let defaults = fonts.families.get(family).cloned().unwrap_or_default();
        defaults.into_iter().filter(|name| !BUNDLED_EMOJI_FONTS.contains(&name.as_str())).collect()
    };
    let mut defaults_proportional = without_bundled_emoji(&FontFamily::Proportional);
    let defaults_monospace = without_bundled_emoji(&FontFamily::Monospace);

    for (_, name, bytes) in ICON_FONTS {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }

    let (ui_font, semibold_font) = if sources
        .variable_ui
        .as_ref()
        .is_some_and(|variable| load_variable_ui(&mut fonts, variable))
    {
        (Some("ui".to_owned()), Some("ui-semibold".to_owned()))
    } else {
        (
            load_first(&mut fonts, "ui", &sources.ui),
            load_first(&mut fonts, "ui-semibold", &sources.semibold),
        )
    };
    let code_font = load_first(&mut fonts, "code", &sources.code);
    let emoji_font = load_first(&mut fonts, "emoji", &sources.emoji);
    let symbol_font = load_first(&mut fonts, "symbol", &sources.symbol);
    platform::register_chrome_fonts(&mut fonts);

    // Icons, then the system's emoji and symbols, then egui's bundled Hack (arrows, bullets,
    // geometric shapes) and emoji: every family ends so.
    let mut fallbacks: Vec<String> = vec![INLINE_ICON_FONT.to_owned()];
    fallbacks.extend(emoji_font);
    fallbacks.extend(symbol_font);
    fallbacks.push(BUNDLED_SYMBOL_FONT.to_owned());
    fallbacks.extend(BUNDLED_EMOJI_FONTS.map(str::to_owned));

    let mut proportional: Vec<String> = ui_font.into_iter().collect();
    proportional.append(&mut defaults_proportional);
    proportional.extend(fallbacks.iter().cloned());

    let mut semibold_chain: Vec<String> = semibold_font.into_iter().collect();
    semibold_chain.extend(proportional.iter().cloned());

    let mut monospace: Vec<String> = code_font.into_iter().collect();
    monospace.extend(defaults_monospace);
    monospace.extend(fallbacks);

    for (weight, name, _) in ICON_FONTS {
        fonts.families.insert(weight.family(), icon_chain(&proportional, name));
    }

    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Monospace, monospace);
    fonts.families.insert(semibold(), semibold_chain);
    // A name without data would make egui panic on the first text of that family; a name twice
    // (Hack in the monospace chain) only costs a lookup.
    let loaded = fonts.font_data.clone();
    for chain in fonts.families.values_mut() {
        let mut seen = std::collections::HashSet::new();
        chain.retain(|name| loaded.contains_key(name) && seen.insert(name.clone()));
    }
    fonts
}

/// The text font first (Phosphor maps Latin letters to the blank glyphs its ligatures are built
/// from), then the icon weight, so every codepoint of the icon set resolves to that weight ahead of
/// egui's own symbol fonts.
fn icon_chain(proportional: &[String], icon_font: &str) -> Vec<String> {
    let mut chain: Vec<String> =
        proportional.iter().filter(|name| *name != INLINE_ICON_FONT).cloned().collect();
    chain.insert(chain.len().min(1), icon_font.to_owned());
    chain
}

/// Registers "ui" and "ui-semibold" as two instances of the variable font sharing one buffer.
fn load_variable_ui(fonts: &mut FontDefinitions, variable: &VariableUiFont) -> bool {
    let Ok(bytes) = std::fs::read(&variable.face.path) else {
        return false;
    };
    let shared = FontData::from_blob(Arc::new(bytes), variable.face.index);
    for (name, weight) in [("ui", REGULAR_WEIGHT), ("ui-semibold", SEMIBOLD_WEIGHT)] {
        let coords = match variable.optical_size {
            Some(optical_size) => {
                VariationCoords::new([(b"wght", weight), (b"opsz", optical_size)])
            }
            None => VariationCoords::new([(b"wght", weight)]),
        };
        let tweak = FontTweak { coords, ..FontTweak::default() };
        fonts.font_data.insert(name.to_owned(), Arc::new(shared.clone().tweak(tweak)));
    }
    true
}

/// Registers the first of `faces` that can be read as the font `name`; returns the name.
pub fn load_first(fonts: &mut FontDefinitions, name: &str, faces: &[FontFace]) -> Option<String> {
    let (bytes, index) = faces
        .iter()
        .find_map(|face| std::fs::read(&face.path).ok().map(|bytes| (bytes, face.index)))?;
    fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_blob(Arc::new(bytes), index)));
    Some(name.to_owned())
}

#[cfg(test)]
mod tests {
    use egui::{FontFamily, FontId};
    use egui_kittest::Harness;

    use super::{BUNDLED_EMOJI_FONTS, definitions, optical_center, semibold};
    use crate::platform::{FontFace, FontSources};
    use crate::theme::icons::{self, Weight};

    /// The words' optical centre lies between their capitals' top and their baseline; with the
    /// system UI font it is below the line box's middle, which the font's line gap lifts.
    #[test]
    fn words_centre_between_cap_height_and_baseline() {
        let mut harness = Harness::builder().with_context_setup(super::install).build_ui(|_| {});
        harness.run_steps(1);
        let painter = egui::Painter::new(
            harness.ctx.clone(),
            egui::LayerId::background(),
            egui::Rect::EVERYTHING,
        );
        for font in [FontId::proportional(12.0), super::semibold_font(12.0)] {
            let line = painter.layout_no_wrap("H".into(), font.clone(), egui::Color32::WHITE);
            let glyph = &line.rows[0].row.glyphs[0];
            let (baseline, cap_top) = (glyph.pos.y, glyph.pos.y + glyph.uv_rect.offset.y);
            let center = optical_center(&painter, &font);
            assert!(cap_top < center && center < baseline, "{center} in {cap_top}..{baseline}");
            #[cfg(windows)]
            assert!(center > line.size().y / 2.0, "{center} against {:?}", line.size());
        }
    }

    /// Every non-ASCII character the app's own strings use on every system (the bullets of
    /// rendered Markdown, the arrows of the focus strip and the whitespace marks among them), and
    /// emoji as comments carry them.
    const SAMPLE: &str = "· × Üßáäíïü – — “ ” • … ↑ → ↓ ↔ − ▪ ● ◦ ✔ ★ ⚠ 🚀 👍 🎉 😀 ❤";
    /// The modifier symbols of macOS shortcut labels (`platform::shortcut!`); Apple Symbols has them.
    #[cfg(target_os = "macos")]
    const MAC_KEYS: &str = "⌃ ⌥ ⇧ ⌘ ↩";

    fn families() -> Vec<FontFamily> {
        let mut families = vec![FontFamily::Proportional, FontFamily::Monospace, semibold()];
        families.extend([Weight::Regular, Weight::Bold, Weight::Fill].map(Weight::family));
        families
    }

    /// The Linux path without system fonts (and any system whose fonts are missing): egui's
    /// bundled fonts alone draw every symbol and emoji in every family.
    fn assert_draws(sources: &FontSources, sample: &str) {
        let fonts = definitions(sources);
        for (family, chain) in &fonts.families {
            for name in chain {
                assert!(
                    fonts.font_data.contains_key(name),
                    "{family:?} names {name:?}, which has no data"
                );
            }
        }
        // The text families; the window chrome's own (Windows caption glyphs) draw only themselves.
        for family in families() {
            let chain = &fonts.families[&family];
            for bundled in BUNDLED_EMOJI_FONTS {
                assert!(
                    chain.iter().any(|name| name == bundled),
                    "{family:?} ends without {bundled}: {chain:?}"
                );
            }
        }
        let text = format!("{sample} {} {} {}", icons::CHECK, icons::WARNING, icons::CARET_RIGHT);
        let probe =
            Harness::builder().with_context_setup(move |ctx| ctx.set_fonts(fonts)).build_ui(|_| {});
        let missing: Vec<String> = families()
            .into_iter()
            .flat_map(|family| {
                let font = FontId::new(14.0, family.clone());
                text.chars()
                    .filter(|glyph| {
                        !glyph.is_whitespace()
                            && !probe.ctx.fonts_mut(|fonts| fonts.has_glyph(&font, *glyph))
                    })
                    .map(move |glyph| format!("{glyph} (U+{:04X}) in {family:?}", u32::from(glyph)))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(missing.is_empty(), "no font draws {missing:#?}");
    }

    #[test]
    fn bundled_fonts_alone_draw_symbols_and_emoji_in_every_family() {
        assert_draws(&FontSources::default(), SAMPLE);
    }

    #[test]
    fn faces_that_cannot_be_read_leave_no_names_behind() {
        let missing = |file: &str| {
            vec![FontFace {
                path: std::env::temp_dir().join("no-such-font-dir").join(file),
                index: 0,
            }]
        };
        let sources = FontSources {
            variable_ui: None,
            ui: missing("ui.ttf"),
            semibold: missing("semibold.ttf"),
            code: missing("code.ttf"),
            emoji: missing("emoji.ttf"),
            symbol: missing("symbol.ttf"),
        };
        assert_draws(&sources, SAMPLE);
    }

    #[test]
    fn emoji_in_semibold_text_resolves_with_the_system_fonts() {
        // The real sources of the system the test runs on (the macOS report: 🚀 in "semibold").
        assert_draws(&crate::platform::font_sources(), SAMPLE);
        #[cfg(target_os = "macos")]
        assert_draws(&crate::platform::font_sources(), MAC_KEYS);
    }
}
