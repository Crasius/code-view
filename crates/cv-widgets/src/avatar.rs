use egui::{Align2, Color32, Painter, Pos2, Response, Sense, Stroke, Ui, vec2};

use cv_base::theme::{Palette, icons};
use review_core::{ReviewVote, UserIdentity};

/// Row fill between an avatar and its vote ring, so the ring never melts into a persona color.
const RING_GAP: f32 = 1.0;
const RING_WIDTH: f32 = 2.0;
/// How far a voted avatar's ring reaches past its own circle: the gap and the ring.
pub const RING_REACH: f32 = RING_GAP + RING_WIDTH;
/// The badge's center from the avatar's, along each axis, as a share of the avatar's size: on the
/// ring at 45°, so the badge overlaps avatar and ring alike.
const BADGE_OFFSET: f32 = 0.375;
/// The badge's radius and its icon's size, as shares of the avatar's size.
const BADGE_RADIUS: f32 = 0.27;
const BADGE_ICON: f32 = 0.42;
/// The row fill around the badge, cutting it out of ring and avatar.
const BADGE_CUT: f32 = 1.5;

/// How far a voted avatar of `size` reaches right of and below its center: its badge's edge.
pub const fn badge_reach(size: f32) -> f32 {
    size * (BADGE_OFFSET + BADGE_RADIUS) + BADGE_CUT
}

/// How a vote marks an avatar: a ring and a badge in `color`, the badge with `icon`, filled or
/// (approved with suggestions) outlined, so shape and icon tell the votes apart, not color alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VoteMark {
    pub icon: &'static str,
    pub color: Color32,
    pub is_filled: bool,
}

/// The mark of `vote`; none for a reviewer who has not voted.
pub(crate) fn vote_mark(vote: ReviewVote, palette: &Palette) -> Option<VoteMark> {
    let (icon, color, is_filled) = match vote {
        ReviewVote::None => return None,
        ReviewVote::Approved => (icons::CHECK, palette.success, true),
        ReviewVote::ApprovedWithSuggestions => (icons::CHECK, palette.success, false),
        ReviewVote::WaitingForAuthor => (icons::CLOCK, palette.warning, true),
        ReviewVote::ChangesRequested => (icons::X, palette.error, true),
    };
    Some(VoteMark { icon, color, is_filled })
}

/// Initials in a colored circle. No image downloads: zero network and texture memory per person.
pub fn avatar(ui: &mut Ui, identity: &UserIdentity, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint(ui.painter(), rect.center(), size, identity);
    response.on_hover_text(&identity.display_name)
}

/// [`avatar`] with its vote's ring and badge, cut out of `palette.surface` (the card or row it sits
/// on); the ring reaches [`RING_REACH`] past the allocated square into the item spacing.
pub fn voted(ui: &mut Ui, identity: &UserIdentity, size: f32, vote: ReviewVote) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let palette = cv_base::theme::palette(ui.ctx());
    paint_voted(ui.painter(), &palette, palette.surface, rect.center(), size, identity, vote);
    response.on_hover_text(&identity.display_name)
}

/// The avatar at `center` with its vote's ring hugging it and a badge at its lower right, both cut
/// out of `fill`. Without a vote only the avatar is drawn.
pub fn paint_voted(
    painter: &Painter,
    palette: &Palette,
    fill: Color32,
    center: Pos2,
    size: f32,
    identity: &UserIdentity,
    vote: ReviewVote,
) {
    paint(painter, center, size, identity);
    let Some(mark) = vote_mark(vote, palette) else {
        return;
    };
    let radius = size / 2.0;
    painter.circle_stroke(
        center,
        radius + RING_GAP + RING_WIDTH / 2.0,
        Stroke::new(RING_WIDTH, mark.color),
    );
    let badge = center + vec2(1.0, 1.0) * (size * BADGE_OFFSET);
    let badge_radius = size * BADGE_RADIUS;
    painter.circle_filled(badge, badge_radius + BADGE_CUT, fill);
    let glyph = if mark.is_filled {
        painter.circle_filled(badge, badge_radius, mark.color);
        fill
    } else {
        painter.circle(badge, badge_radius - 0.75, fill, Stroke::new(1.5, mark.color));
        mark.color
    };
    painter.text(
        badge,
        Align2::CENTER_CENTER,
        mark.icon,
        icons::font(size * BADGE_ICON, icons::Weight::Bold),
        glyph,
    );
}

/// The avatar circle at `center`, for custom-painted rows.
pub fn paint(painter: &Painter, center: Pos2, size: f32, identity: &UserIdentity) {
    let fill = color_for(&identity.id, &identity.display_name);
    painter.circle_filled(center, size / 2.0, fill);
    painter.text(
        center,
        Align2::CENTER_CENTER,
        initials(&identity.display_name),
        cv_base::theme::fonts::semibold_font(size * 0.4),
        Color32::WHITE,
    );
}

/// The first letters of the first and last word. Directory names carry notes in brackets
/// (`Weber, Jonas (Contractor)`, `[Web]\Contributors`); those are left out, and a word's letter is
/// its first letter or digit, never the punctuation around it.
pub(crate) fn initials(name: &str) -> String {
    let mut depth = 0_usize;
    let unbracketed: String = name
        .chars()
        .filter(|character| match character {
            '(' | '[' => {
                depth += 1;
                false
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .collect();
    let has_letters = |text: &str| text.chars().any(char::is_alphanumeric);
    let name = if has_letters(&unbracketed) { unbracketed.as_str() } else { name };
    let mut words = name
        .split(|character: char| {
            character.is_whitespace() || matches!(character, '.' | '-' | '_' | ',' | '\\' | '/')
        })
        .filter(|word| has_letters(word));
    let initial = |word: &str| word.chars().find(|character| character.is_alphanumeric());
    let first = words.next().and_then(initial);
    let last = words.next_back().and_then(initial);
    let letters: String = [first, last].into_iter().flatten().collect();
    if letters.is_empty() {
        return "?".into();
    }
    letters.to_uppercase()
}

/// Fluent's persona colors, picked by a stable hash so a person keeps one color.
const PERSONA_COLORS: [Color32; 10] = [
    Color32::from_rgb(0x75, 0x0B, 0x1C),
    Color32::from_rgb(0xA4, 0x26, 0x2C),
    Color32::from_rgb(0xCA, 0x50, 0x10),
    Color32::from_rgb(0x8E, 0x56, 0x2E),
    Color32::from_rgb(0x49, 0x82, 0x05),
    Color32::from_rgb(0x03, 0x83, 0x87),
    Color32::from_rgb(0x00, 0x5B, 0x70),
    Color32::from_rgb(0x00, 0x78, 0xD4),
    Color32::from_rgb(0x4F, 0x6B, 0xED),
    Color32::from_rgb(0x88, 0x17, 0x98),
];

fn color_for(id: &str, name: &str) -> Color32 {
    let key = if id.is_empty() { name } else { id };
    let hash = key.bytes().fold(5381u32, |hash, byte| hash.wrapping_mul(33) ^ u32::from(byte));
    PERSONA_COLORS[hash as usize % PERSONA_COLORS.len()]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use cv_base::theme::Palette;
    use cv_base::theme::palette::contrast_ratio;
    use review_core::ReviewVote;

    use super::{initials, vote_mark};

    #[test]
    fn votes_differ_in_shape_not_only_in_color() {
        let palette = Palette::LIGHT;
        assert_eq!(vote_mark(ReviewVote::None, &palette), None);
        let shapes: HashSet<(&str, bool)> = [
            ReviewVote::Approved,
            ReviewVote::ApprovedWithSuggestions,
            ReviewVote::WaitingForAuthor,
            ReviewVote::ChangesRequested,
        ]
        .into_iter()
        .map(|vote| vote_mark(vote, &palette).expect("a vote has a mark"))
        .map(|mark| (mark.icon, mark.is_filled))
        .collect();
        assert_eq!(shapes.len(), 4, "{shapes:?}");
    }

    /// Ring, badge and the glyph cut out of the badge keep 3:1 (WCAG non-text contrast) on every
    /// fill a list row or a card gives them, in both themes.
    #[test]
    fn vote_marks_have_three_to_one_contrast_on_row_fills() {
        let votes = [
            ReviewVote::Approved,
            ReviewVote::ApprovedWithSuggestions,
            ReviewVote::WaitingForAuthor,
            ReviewVote::ChangesRequested,
        ];
        for palette in [Palette::LIGHT, Palette::DARK] {
            for vote in votes {
                let color = vote_mark(vote, &palette).map(|mark| mark.color);
                let color = color.expect("a vote has a mark");
                for (fill, background) in [
                    ("surface", palette.surface),
                    ("hover", palette.hover),
                    ("pressed", palette.pressed),
                ] {
                    let ratio = contrast_ratio(color, background);
                    assert!(
                        ratio >= 3.0,
                        "{vote:?} on {fill} (dark {}): {ratio:.2}",
                        palette.is_dark
                    );
                }
            }
        }
    }

    #[test]
    fn builds_initials() {
        assert_eq!(initials("Mara Kühn"), "MK");
        assert_eq!(initials("jonas.weber"), "JW");
        assert_eq!(initials("priya"), "P");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn initials_skip_bracketed_notes_and_punctuation() {
        assert_eq!(initials("Weber, Jonas (Contractor)"), "WJ");
        assert_eq!(initials("Jonas Weber (ACME GmbH)"), "JW");
        assert_eq!(initials("[Web]\\Contributors"), "C");
        assert_eq!(initials("[Web]\\Project Administrators"), "PA");
        assert_eq!(initials("\"Mara\" 'Kühn'"), "MK");
        assert_eq!(initials("(bot)"), "B");
        assert_eq!(initials("()"), "?");
    }
}
