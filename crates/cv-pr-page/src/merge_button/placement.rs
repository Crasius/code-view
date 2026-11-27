//! Where the merge dropdown opens, for the room the window leaves around the button.

use egui::{Rect, RectAlign};

use super::MENU_WIDTH;

/// The menu opens below the button when this much room is left there (or more than above it);
/// otherwise above it. It never covers the button and scrolls when its side is shorter.
const MENU_MIN_HEIGHT: f32 = 240.0;

/// Where the dropdown opens: right-aligned with the button, below it while the room there is
/// [`MENU_MIN_HEIGHT`] or more (or more than above it), else above it. Either way it stays inside
/// the window, keeps off the button (whose click must not land on a menu row) and scrolls within
/// the room of its side; in a narrow window it narrows too.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MenuPlacement {
    pub align: RectAlign,
    pub max_height: f32,
    pub width: f32,
}

impl MenuPlacement {
    /// `chrome`: what the menu's frame, the gap and the margin to the window's edges add around
    /// its content, sideways and in height.
    pub(crate) fn new(window: Rect, button: Rect, chrome: egui::Vec2) -> Self {
        let below = window.bottom() - button.bottom() - chrome.y;
        let above = button.top() - window.top() - chrome.y;
        let (align, room) = if below >= MENU_MIN_HEIGHT || below >= above {
            (RectAlign::BOTTOM_END, below)
        } else {
            (RectAlign::TOP_END, above)
        };
        Self {
            align,
            max_height: room.max(0.0),
            width: MENU_WIDTH.min(window.width() - chrome.x).max(0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Rect, RectAlign, pos2, vec2};

    use super::super::MENU_WIDTH;
    use super::{MENU_MIN_HEIGHT, MenuPlacement};

    const CHROME: egui::Vec2 = vec2(28.0, 24.0);

    fn window(width: f32, height: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(width, height))
    }

    fn button_at(top: f32) -> Rect {
        Rect::from_min_size(pos2(700.0, top), vec2(190.0, 32.0))
    }

    #[test]
    fn the_menu_opens_below_the_button_where_it_fits() {
        let placement = MenuPlacement::new(window(1280.0, 900.0), button_at(56.0), CHROME);
        assert_eq!(placement.align, RectAlign::BOTTOM_END);
        assert_eq!(
            placement.max_height,
            900.0 - 88.0 - CHROME.y,
            "all the room down to the window's edge"
        );
        assert_eq!(placement.width, MENU_WIDTH);
    }

    #[test]
    fn near_the_window_bottom_the_menu_opens_above_and_keeps_off_the_button() {
        let button = button_at(480.0);
        let placement = MenuPlacement::new(window(960.0, 540.0), button, CHROME);
        assert_eq!(placement.align, RectAlign::TOP_END);
        assert_eq!(
            placement.max_height,
            button.top() - CHROME.y,
            "no taller than the room above, so it never covers the button"
        );
    }

    #[test]
    fn a_short_window_takes_the_side_with_more_room() {
        let tight = window(960.0, 300.0);
        let high = MenuPlacement::new(tight, button_at(40.0), CHROME);
        assert_eq!(
            high.align,
            RectAlign::BOTTOM_END,
            "less than the minimum below, but more than above"
        );
        assert!(
            high.max_height < MENU_MIN_HEIGHT && high.max_height > 0.0,
            "it scrolls in what is there: {}",
            high.max_height
        );
        let low = MenuPlacement::new(tight, button_at(200.0), CHROME);
        assert_eq!(low.align, RectAlign::TOP_END);
        assert_eq!(low.max_height, 200.0 - CHROME.y);
    }

    #[test]
    fn a_narrow_window_narrows_the_menu() {
        let placement = MenuPlacement::new(window(380.0, 900.0), button_at(56.0), CHROME);
        assert_eq!(placement.width, 380.0 - CHROME.x);
    }
}
