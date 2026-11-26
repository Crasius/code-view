//! What the Markdown and the rich editor's formatting toolbars share, so they look the same: the
//! button size, the gaps, the hairline dividers between groups and which groups fit the row.

use cv_base::theme;
use egui::{Sense, Stroke, Ui, vec2};

/// The square icon buttons, also the mode switch's height.
pub const BUTTON_SIZE: f32 = 28.0;
pub const BUTTON_GAP: f32 = 2.0;
/// A divider's slot, its gaps to the buttons around it included.
pub const DIVIDER_WIDTH: f32 = 13.0;

/// The width of a group of `buttons` buttons.
pub fn group_width(buttons: usize) -> f32 {
    buttons as f32 * BUTTON_SIZE + buttons.saturating_sub(1) as f32 * BUTTON_GAP
}

/// How many groups fit into `width`, the rest going into the overflow menu.
pub fn visible_groups(group_widths: &[f32], width: f32) -> usize {
    let total = |count: usize, with_overflow: bool| {
        let groups: f32 = group_widths[..count].iter().sum();
        let dividers = count.saturating_sub(1) as f32 * DIVIDER_WIDTH;
        let overflow = if with_overflow { DIVIDER_WIDTH + BUTTON_SIZE } else { 0.0 };
        groups + dividers + overflow
    };
    if total(group_widths.len(), false) <= width {
        return group_widths.len();
    }
    (0..group_widths.len()).rev().find(|&count| total(count, true) <= width).unwrap_or(0)
}

/// The hairline between two groups.
pub fn divider(ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) =
        ui.allocate_exact_size(vec2(DIVIDER_WIDTH - 2.0 * BUTTON_GAP, BUTTON_SIZE), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.shrink2(vec2(0.0, 6.0)).y_range(),
        Stroke::new(1.0, palette.border),
    );
}

#[cfg(test)]
mod tests {
    use super::visible_groups;

    #[test]
    fn groups_overflow_from_the_right() {
        let widths = [118.0, 58.0, 88.0, 88.0, 28.0];
        let all: f32 = widths.iter().sum::<f32>() + 4.0 * 13.0;
        assert_eq!(visible_groups(&widths, all), 5);
        // The overflow button needs room too, so one pixel short drops two groups.
        assert_eq!(visible_groups(&widths, all - 1.0), 3);
        assert_eq!(visible_groups(&widths, 130.0), 0);
        assert_eq!(visible_groups(&widths, 170.0), 1);
    }
}
