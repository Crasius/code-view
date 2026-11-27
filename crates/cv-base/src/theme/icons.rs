//! Phosphor icons (MIT, see assets/Phosphor-LICENSE-MIT.txt): codepoints of the icons the app uses,
//! the weights and sizes they are drawn at, and which icon stands for which action.
//!
//! # Weights
//!
//! Regular, Bold and Fill share one set of codepoints; [`Weight::family`] picks the font. Inline
//! text such as `"{icon}  Label"` falls back to Bold, the weight for text-sized icons. Painters
//! choose with [`weight`] / [`state_font`]:
//!
//! | State                      | Light theme              | Dark theme |
//! |----------------------------|--------------------------|------------|
//! | At rest, 14–16 px          | Bold                     | Bold       |
//! | At rest, 20 px and up      | Regular                  | Bold       |
//! | Selected, active, toggled  | Fill                     | Fill       |
//!
//! Regular's stroke (1.5 of 24 units) is 1 px at 16 px and renders grey next to 14 px text; Bold's
//! matches the stem of Segoe UI, so small icons read as crisply as their labels. From 20 px on,
//! Regular is crisp in the light theme. On dark surfaces light strokes lose their anti-aliased
//! edge, so the dark theme stays on Bold at every size. Fill marks the one selected or switched-on
//! item of a group (the current section, a pressed toggle), never a plain button.
//!
//! # Sizes
//!
//! [`SIZE_SMALL`] (14) beside 12–13 px text, [`SIZE`] (16) in buttons, menus and rows,
//! [`SIZE_LARGE`] (20) in the navigation rail. Icons are painted centred on their slot, never
//! baseline-aligned with a label, so they stay optically centred whatever the text font.
//!
//! # Action map
//!
//! One icon per action across the app. Shortcuts appear in the tooltip as "(Key)".
//!
//! | Action                              | Icon                      | Shortcut      |
//! |-------------------------------------|---------------------------|---------------|
//! | Back                                | `ARROW_LEFT`              | Alt+Left      |
//! | Forward                             | `ARROW_RIGHT`             | Alt+Right     |
//! | Refresh, reload, retry, check again | `ARROW_CLOCKWISE`         | F5 (pages)    |
//! | Rerun pipeline jobs                 | `ARROWS_CLOCKWISE`        |               |
//! | Open on GitHub / GitLab / Azure DevOps | platform mark (`provider_icon::glyph`) |  |
//! | Open externally (docs, token page)  | `ARROW_SQUARE_OUT`        |               |
//! | Copy text, path or command          | `COPY`                    |               |
//! | Copy link                           | `LINK`                    |               |
//! | More actions (overflow menu)        | `DOTS_THREE`              |               |
//! | Settings, connection setup          | `GEAR`                    |               |
//! | Add or fix a token                  | `KEY`                     |               |
//! | Check access                        | `SHIELD_CHECK`            |               |
//! | Edit                                | `PENCIL_SIMPLE`           |               |
//! | Remove, delete, discard             | `TRASH`                   |               |
//! | Close, dismiss, cancel, clear       | `X`                       | Esc           |
//! | Add                                 | `PLUS`                    |               |
//! | Add reviewer                        | `USER_PLUS`               |               |
//! | Done editing, approve               | `CHECK`                   |               |
//! | Complete / merge                    | `GIT_MERGE`               |               |
//! | Mark as draft                       | `FILE_DASHED`             |               |
//! | Publish draft                       | `PAPER_PLANE_TILT`        |               |
//! | Abandon / close pull request        | `PROHIBIT`                |               |
//! | Reactivate                          | `ARROW_COUNTER_CLOCKWISE` |               |
//! | Reset to default                    | `ARROW_COUNTER_CLOCKWISE` | Ctrl+0 (zoom) |
//! | Reply                               | `ARROW_BEND_UP_LEFT`      |               |
//! | Resolve thread                      | `CHECK_CIRCLE`            |               |
//! | Comment on a line or the selection  | `CHAT_CIRCLE_TEXT`        | Alt+C         |
//! | Post comment                        | `PAPER_PLANE_RIGHT`       | Ctrl+Enter    |
//! | Show all changes                    | `GIT_DIFF`                |               |
//! | Filter files by category            | `FUNNEL_SIMPLE`           |               |
//! | Hide viewed files (toggle)          | `EYE_SLASH` / `EYE`       |               |
//! | Ignore whitespace (toggle)          | `PARAGRAPH`               | Alt+W         |
//! | Hide / show whitespace changes      | `EYE_SLASH` / `EYE`       | Alt+W         |
//! | Next / previous match               | `ARROW_DOWN` / `ARROW_UP` | Enter / Shift+Enter |
//! | Jump to the first error             | `WARNING_CIRCLE`          |               |
//! | Previous / next check               | `CARET_UP` / `CARET_DOWN` | K / J         |
//! | Back to the checks                  | `ARROW_LEFT`              | Esc           |
//! | Search                              | `MAGNIFYING_GLASS`        |               |
//! | Add to / remove from favorites      | `STAR` (Fill when starred)|               |
//! | Paste a new token                   | `CLIPBOARD_TEXT`          |               |
//! | Continue (wizard)                   | `ARROW_RIGHT`             | Enter         |
//! | Save                                | `FLOPPY_DISK`             |               |
//! | Color anyway                        | `PAINT_BRUSH`             |               |
//! | Expand hidden lines                 | `ARROWS_OUT_LINE_VERTICAL`|               |
//! | Rendered Markdown full width        | `ARROWS_OUT_LINE_HORIZONTAL`|             |
//! | Download                            | `DOWNLOAD_SIMPLE`         |               |
//! | Zoom a canvas in / out              | `PLUS` / `MINUS`          | Ctrl+= / Ctrl+- |
//! | Fit the whole canvas in view        | `CORNERS_OUT`             |               |
//! | Enter / exit focus mode             | `ARROWS_OUT_SIMPLE` / `ARROWS_IN_SIMPLE` | F11, Esc |
//! | Show / hide the file tree in focus mode | `SIDEBAR_SIMPLE`      | Ctrl+B        |
//! | Move the file tree to the right / left | `ARROW_LINE_RIGHT` / `ARROW_LINE_LEFT` |  |
//! | Play / pause a video; play it again | `PLAY` / `PAUSE`; `ARROW_COUNTER_CLOCKWISE` | Space |
//! | Settings: network activity          | `PULSE`                   |               |
//! | Settings: keyboard and mouse        | `KEYBOARD`                |               |
//!
//! File categories: Code `CODE`, Tests `FLASK`, Docs `BOOK_OPEN_TEXT`, Config & build
//! `SLIDERS_HORIZONTAL`, Generated `ROBOT`, Assets `IMAGE`. File types in the file tree map to
//! icons in `cv_pr_tree::file_icon`.

use egui::{FontFamily, FontId};

use super::Palette;

/// Beside small (12–13 px) text: counts, metadata, chips.
pub const SIZE_SMALL: f32 = 14.0;
/// Buttons, menu items, list rows.
pub const SIZE: f32 = 16.0;
/// The navigation rail.
pub const SIZE_LARGE: f32 = 20.0;

/// The bundled Phosphor weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Bold,
    Fill,
}

impl Weight {
    /// The font family that draws this weight (registered by [`super::fonts::install`]).
    pub(crate) fn family(self) -> FontFamily {
        FontFamily::Name(
            match self {
                Weight::Regular => "icon-regular",
                Weight::Bold => "icon-bold",
                Weight::Fill => "icon-fill",
            }
            .into(),
        )
    }
}

/// Icons below this size take the Bold weight at rest in the light theme too.
const BOLD_BELOW: f32 = 18.0;

/// Weight for an icon of `size` in the current theme: Fill when it marks the selected or
/// switched-on item, else the resting weight of the module docs' table.
pub(crate) fn weight(palette: &Palette, size: f32, is_active: bool) -> Weight {
    if is_active {
        Weight::Fill
    } else if palette.is_dark || size < BOLD_BELOW {
        Weight::Bold
    } else {
        Weight::Regular
    }
}

/// Font for an icon glyph at `size` in `weight`.
pub fn font(size: f32, weight: Weight) -> FontId {
    FontId::new(size, weight.family())
}

/// Resting icon font at `size` for the current theme.
pub fn rest_font(palette: &Palette, size: f32) -> FontId {
    font(size, weight(palette, size, false))
}

/// Icon font at `size` for the current theme, Fill when `is_active`.
pub fn state_font(palette: &Palette, size: f32, is_active: bool) -> FontId {
    font(size, weight(palette, size, is_active))
}

pub const ARROWS_CLOCKWISE: &str = "\u{E094}";
pub const ARROWS_IN_SIMPLE: &str = "\u{E09E}";
pub const ARROWS_OUT_LINE_HORIZONTAL: &str = "\u{E534}";
pub const ARROWS_OUT_LINE_VERTICAL: &str = "\u{E536}";
pub const ARROWS_OUT_SIMPLE: &str = "\u{E0A6}";
pub const ARROW_BEND_UP_LEFT: &str = "\u{E024}";
pub const ARROW_CLOCKWISE: &str = "\u{E036}";
pub const ARROW_COUNTER_CLOCKWISE: &str = "\u{E038}";
pub const ARROW_DOWN: &str = "\u{E03E}";
pub const ARROW_ELBOW_DOWN_LEFT: &str = "\u{E044}";
pub const ARROW_LEFT: &str = "\u{E058}";
pub const ARROW_LINE_DOWN: &str = "\u{E05C}";
pub const ARROW_LINE_LEFT: &str = "\u{E062}";
pub const ARROW_LINE_RIGHT: &str = "\u{E064}";
pub const ARROW_LINE_UP: &str = "\u{E066}";
pub const ARROW_RIGHT: &str = "\u{E06C}";
pub const ARROW_SQUARE_OUT: &str = "\u{E5DE}";
pub const ARROW_UP: &str = "\u{E08E}";
pub const AT: &str = "\u{E0AC}";
pub const BIRD: &str = "\u{E72C}";
pub const BOOKS: &str = "\u{E758}";
pub const BOOK_OPEN_TEXT: &str = "\u{E8F2}";
pub const BRACKETS_ANGLE: &str = "\u{E862}";
pub const BRACKETS_CURLY: &str = "\u{E860}";
pub const BRAIN: &str = "\u{E74E}";
pub const CARET_DOWN: &str = "\u{E136}";
pub const CARET_RIGHT: &str = "\u{E13A}";
pub const CARET_UP: &str = "\u{E13C}";
pub const CERTIFICATE: &str = "\u{E766}";
pub const CHAT_CIRCLE_TEXT: &str = "\u{E16E}";
pub const CHECK: &str = "\u{E182}";
pub const CHECK_CIRCLE: &str = "\u{E184}";
pub const CHECK_SQUARE: &str = "\u{E186}";
pub const CIRCLE_DASHED: &str = "\u{E602}";
pub const CIRCLE_NOTCH: &str = "\u{EB44}";
pub const CLIPBOARD_TEXT: &str = "\u{E198}";
pub const CLOCK: &str = "\u{E19A}";
pub const CLOCK_COUNTER_CLOCKWISE: &str = "\u{E1A0}";
pub const CLOUD: &str = "\u{E1AA}";
pub const CODE: &str = "\u{E1BC}";
pub const CODE_BLOCK: &str = "\u{EAFE}";
pub const COFFEE: &str = "\u{E1C2}";
pub const COLUMNS: &str = "\u{E546}";
pub const COPY: &str = "\u{E1CA}";
pub const CORNERS_OUT: &str = "\u{E1D0}";
pub const CPU: &str = "\u{E610}";
pub const CUBE: &str = "\u{E1DA}";
pub const DATABASE: &str = "\u{E1DE}";
pub const DIAMOND: &str = "\u{E1EC}";
pub const DISC: &str = "\u{E564}";
pub const DOTS_THREE: &str = "\u{E1FE}";
pub const DOWNLOAD_SIMPLE: &str = "\u{E20C}";
pub const EYE: &str = "\u{E220}";
pub const EYE_SLASH: &str = "\u{E224}";
pub const FILE: &str = "\u{E230}";
pub const FILE_ARCHIVE: &str = "\u{EB2A}";
pub const FILE_AUDIO: &str = "\u{EA20}";
pub const FILE_C: &str = "\u{EB32}";
pub const FILE_CODE: &str = "\u{E914}";
pub const FILE_CPP: &str = "\u{EB2E}";
pub const FILE_CSS: &str = "\u{EB34}";
pub const FILE_CSV: &str = "\u{EB1C}";
pub const FILE_C_SHARP: &str = "\u{EB30}";
pub const FILE_DASHED: &str = "\u{E704}";
pub const FILE_DOC: &str = "\u{EB1E}";
pub const FILE_HTML: &str = "\u{EB38}";
pub const FILE_IMAGE: &str = "\u{EA24}";
pub const FILE_JPG: &str = "\u{EB1A}";
pub const FILE_JS: &str = "\u{EB24}";
pub const FILE_JSX: &str = "\u{EB3A}";
pub const FILE_LOCK: &str = "\u{E95C}";
pub const FILE_MD: &str = "\u{ED50}";
pub const FILE_MINUS: &str = "\u{E234}";
pub const FILE_PDF: &str = "\u{E702}";
pub const FILE_PLUS: &str = "\u{E236}";
pub const FILE_PNG: &str = "\u{EB18}";
pub const FILE_PPT: &str = "\u{EB20}";
pub const FILE_PY: &str = "\u{EB2C}";
pub const FILE_RS: &str = "\u{EB28}";
pub const FILE_SQL: &str = "\u{ED4E}";
pub const FILE_SVG: &str = "\u{ED08}";
pub const FILE_TEXT: &str = "\u{E23A}";
pub const FILE_TS: &str = "\u{EB26}";
pub const FILE_TSX: &str = "\u{EB3C}";
pub const FILE_TXT: &str = "\u{EB35}";
pub const FILE_VIDEO: &str = "\u{EA22}";
pub const FILE_VUE: &str = "\u{EB3E}";
pub const FILE_XLS: &str = "\u{EB22}";
pub const FILE_ZIP: &str = "\u{E958}";
pub const FLASK: &str = "\u{E79E}";
pub const FLOPPY_DISK: &str = "\u{E248}";
pub const FOLDER: &str = "\u{E24A}";
pub const FOLDER_OPEN: &str = "\u{E256}";
pub const FUNNEL: &str = "\u{E266}";
pub const FUNNEL_SIMPLE: &str = "\u{E268}";
pub const GAME_CONTROLLER: &str = "\u{E26E}";
pub const GEAR: &str = "\u{E270}";
pub const GEAR_SIX: &str = "\u{E272}";
pub const GIT_BRANCH: &str = "\u{E278}";
/// GitHub's Octocat mark; drawn through `cv_widgets::provider_icon`, which picks weight and color.
pub const GITHUB_LOGO: &str = "\u{E576}";
/// GitLab's tanuki; drawn through `cv_widgets::provider_icon`.
pub const GITLAB_LOGO: &str = "\u{E694}";
pub const GIT_COMMIT: &str = "\u{E27A}";
pub const GIT_DIFF: &str = "\u{E27C}";
pub const GIT_MERGE: &str = "\u{E280}";
pub const GIT_PULL_REQUEST: &str = "\u{E282}";
pub const GRAPH: &str = "\u{EB58}";
pub const HAMMER: &str = "\u{E80E}";
pub const IMAGE: &str = "\u{E2CA}";
pub const INFO: &str = "\u{E2CE}";
pub const KEY: &str = "\u{E2D6}";
pub const KEYBOARD: &str = "\u{E2D8}";
pub const LINK: &str = "\u{E2E2}";
pub const LIST_BULLETS: &str = "\u{E2F2}";
pub const LIST_CHECKS: &str = "\u{EADC}";
pub const LIST_NUMBERS: &str = "\u{E2F6}";
pub const MAGNIFYING_GLASS: &str = "\u{E30C}";
pub const MINUS: &str = "\u{E32A}";
pub const MINUS_CIRCLE: &str = "\u{E32C}";
pub const MINUS_SQUARE: &str = "\u{E32E}";
pub const MOON: &str = "\u{E330}";
pub const MUSIC_NOTES: &str = "\u{E340}";
pub const NOTEBOOK: &str = "\u{E34E}";
pub const PACKAGE: &str = "\u{E390}";
pub const PAINT_BRUSH: &str = "\u{E6F0}";
pub const PALETTE: &str = "\u{E6C8}";
pub const PAPER_PLANE_RIGHT: &str = "\u{E396}";
pub const PAPER_PLANE_TILT: &str = "\u{E398}";
pub const PARAGRAPH: &str = "\u{E960}";
pub const PAUSE: &str = "\u{E39E}";
pub const PENCIL_SIMPLE: &str = "\u{E3B4}";
pub const PLAY: &str = "\u{E3D0}";
pub const PLUGS: &str = "\u{EB56}";
pub const PLUS: &str = "\u{E3D4}";
pub const PLUS_MINUS: &str = "\u{E3D8}";
pub const PROHIBIT: &str = "\u{E3DE}";
pub const PULSE: &str = "\u{E000}";
pub const QUOTES: &str = "\u{E660}";
pub const ROBOT: &str = "\u{E762}";
pub const ROCKET_LAUNCH: &str = "\u{E3FE}";
pub const ROWS: &str = "\u{E5A2}";
pub const SCALES: &str = "\u{E750}";
pub const SCROLL: &str = "\u{EB7A}";
pub const SHARE_NETWORK: &str = "\u{E408}";
pub const SHIELD_CHECK: &str = "\u{E40C}";
pub const SHIPPING_CONTAINER: &str = "\u{E78C}";
pub const SIDEBAR_SIMPLE: &str = "\u{EC24}";
pub const SKIP_FORWARD: &str = "\u{E5A6}";
pub const SLIDERS_HORIZONTAL: &str = "\u{E434}";
pub const SQUARE: &str = "\u{E45E}";
pub const STACK: &str = "\u{E466}";
pub const STAR: &str = "\u{E46A}";
pub const TABLE: &str = "\u{E476}";
pub const TARGET: &str = "\u{E47C}";
pub const TERMINAL_WINDOW: &str = "\u{EAE8}";
pub const TEXT_AA: &str = "\u{E6EE}";
pub const TEXT_B: &str = "\u{E5BE}";
pub const TEXT_H: &str = "\u{E6BA}";
pub const TEXT_ITALIC: &str = "\u{E5C0}";
pub const TEXT_STRIKETHROUGH: &str = "\u{E5C2}";
pub const TICKET: &str = "\u{E490}";
pub const TRASH: &str = "\u{E4A6}";
pub const TREE_STRUCTURE: &str = "\u{E67C}";
pub const USERS: &str = "\u{E4D6}";
pub const USER_PLUS: &str = "\u{E4D0}";
pub const WARNING: &str = "\u{E4E0}";
pub const WARNING_CIRCLE: &str = "\u{E4E2}";
pub const X: &str = "\u{E4F6}";
pub const X_CIRCLE: &str = "\u{E4F8}";
// The rich comment editor's toolbar.
pub const LINK_BREAK: &str = "\u{E2E4}";
pub const TEXT_H_ONE: &str = "\u{E6BC}";
pub const TEXT_H_THREE: &str = "\u{E6C0}";
pub const TEXT_H_TWO: &str = "\u{E6BE}";
pub const TEXT_SUBSCRIPT: &str = "\u{EC98}";
pub const TEXT_SUPERSCRIPT: &str = "\u{EC9A}";
pub const TEXT_UNDERLINE: &str = "\u{E5C4}";

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;

    use super::*;
    use crate::theme;

    /// Every `pub const NAME: &str = "\u{XXXX}";` of this module, read from its own source, so a
    /// new icon is checked without a hand-kept list. The sample list this replaced missed
    /// `ARROWS_OUT_LINE_HORIZONTAL`, the width switch of the file header.
    fn every_icon() -> Vec<(&'static str, char)> {
        include_str!("icons.rs")
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("pub const ")?;
                let (name, value) = rest.split_once(": &str = \"\\u{")?;
                let hex = value.strip_suffix("}\";")?;
                let code = u32::from_str_radix(hex, 16).expect("hex codepoint");
                Some((name, char::from_u32(code).expect("valid codepoint")))
            })
            .collect()
    }

    #[test]
    fn every_weight_family_has_the_icon_glyphs() {
        let mut harness =
            Harness::builder().with_context_setup(theme::fonts::install).build_ui(|_| {});
        harness.run_steps(1);
        let icons = every_icon();
        assert!(icons.len() > 100, "the source scan found only {} icons", icons.len());
        assert!(icons.contains(&("ARROWS_OUT_LINE_HORIZONTAL", '\u{E534}')));
        for weight in [Weight::Regular, Weight::Bold, Weight::Fill] {
            let font = font(SIZE, weight);
            for &(name, glyph) in &icons {
                let has_glyph = harness.ctx.fonts_mut(|fonts| fonts.has_glyph(&font, glyph));
                assert!(has_glyph, "{name} missing in {weight:?}");
            }
        }
    }
}
