//! Which part of the integrated title bar a point is over, as the window procedure answers
//! `WM_NCHITTEST`. Pure: the regions come from egui's last frame in physical client pixels, the
//! frame state from the window, so every case is testable without a window.

use egui::Rect;

/// Most interactive widgets (breadcrumb trail, search box) the caption carves out of its drag area.
pub(crate) const MAX_HOLES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionButton {
    Minimize,
    Maximize,
    Close,
}

impl CaptionButton {
    pub(crate) const ALL: [CaptionButton; 3] =
        [CaptionButton::Minimize, CaptionButton::Maximize, CaptionButton::Close];
}

/// The answer to a hit test, mapped to an `HT*` code by the window procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// egui handles the pointer.
    Client,
    /// Drag to move, double-click to maximize, right-click for the system menu.
    Caption,
    /// The app icon: click for the system menu, double-click to close.
    SystemMenu,
    Button(CaptionButton),
    Top,
    TopLeft,
    TopRight,
}

/// A rectangle in physical client pixels; contains `left..right` × `top..bottom`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PxRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PxRect {
    /// `rect` in egui points, rounded to the pixel grid egui paints on.
    pub(crate) fn from_points(rect: Rect, pixels_per_point: f32) -> Self {
        let px = |value: f32| (value * pixels_per_point).round() as i32;
        Self {
            left: px(rect.left()),
            top: px(rect.top()),
            right: px(rect.right()),
            bottom: px(rect.bottom()),
        }
    }

    pub(crate) fn contains(self, x: i32, y: i32) -> bool {
        (self.left..self.right).contains(&x) && (self.top..self.bottom).contains(&y)
    }
}

/// What egui published for the title bar in its last frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptionRegions {
    /// The whole title bar.
    pub caption: PxRect,
    pub icon: PxRect,
    /// Minimize, maximize, close.
    pub buttons: [PxRect; 3],
    holes: [PxRect; MAX_HOLES],
    hole_count: usize,
}

impl CaptionRegions {
    /// Regions from egui rects in points. Holes beyond [`MAX_HOLES`] are dropped (stay draggable).
    pub(crate) fn from_points(
        caption: Rect,
        icon: Rect,
        buttons: [Rect; 3],
        holes: &[Rect],
        pixels_per_point: f32,
    ) -> Self {
        let mut regions = Self {
            caption: PxRect::from_points(caption, pixels_per_point),
            icon: PxRect::from_points(icon, pixels_per_point),
            buttons: buttons.map(|button| PxRect::from_points(button, pixels_per_point)),
            ..Self::default()
        };
        for hole in holes.iter().filter(|hole| hole.is_positive()).take(MAX_HOLES) {
            regions.holes[regions.hole_count] = PxRect::from_points(*hole, pixels_per_point);
            regions.hole_count += 1;
        }
        regions
    }

    /// The interactive widgets carved out of the drag area: the trail, the search box and the
    /// open lists, in that order.
    pub fn holes(&self) -> &[PxRect] {
        &self.holes[..self.hole_count]
    }
}

/// The window's state for one hit test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowFrame {
    pub is_maximized: bool,
    /// Height of the top resize band inside the client area (the frame the caption removed), px.
    pub resize_border: i32,
    /// Client width, px.
    pub width: i32,
}

/// The part of the window under the client point (`x`, `y`) in physical pixels.
///
/// Order: the top resize band (not while maximized, so the buttons reach the screen edge), the
/// caption buttons, the interactive holes, the icon, then the drag area; everything else is client.
pub fn hit_test(x: i32, y: i32, regions: &CaptionRegions, frame: WindowFrame) -> Hit {
    if !frame.is_maximized && y >= 0 && y < frame.resize_border {
        let corner = frame.resize_border * 2;
        return if x < corner {
            Hit::TopLeft
        } else if x >= frame.width - corner {
            Hit::TopRight
        } else {
            Hit::Top
        };
    }
    if let Some(index) = regions.buttons.iter().position(|button| button.contains(x, y)) {
        return Hit::Button(CaptionButton::ALL[index]);
    }
    if regions.holes().iter().any(|hole| hole.contains(x, y)) {
        return Hit::Client;
    }
    if regions.icon.contains(x, y) {
        return Hit::SystemMenu;
    }
    if regions.caption.contains(x, y) {
        return Hit::Caption;
    }
    Hit::Client
}

#[cfg(test)]
mod tests {
    use egui::{pos2, vec2};

    use super::*;

    /// A 1440 × 900 point window: 40 pt title bar, icon at the left, a breadcrumb trail and a
    /// search box as holes, 46 pt caption buttons at the right.
    fn regions(pixels_per_point: f32) -> CaptionRegions {
        let width = 1440.0;
        let caption = Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 40.0));
        let icon = Rect::from_min_size(pos2(12.0, 12.0), vec2(16.0, 16.0));
        let buttons = [2.0, 1.0, 0.0].map(|from_right: f32| {
            Rect::from_min_size(pos2(width - 46.0 * (from_right + 1.0), 0.0), vec2(46.0, 40.0))
        });
        let trail = Rect::from_min_max(pos2(36.0, 6.0), pos2(300.0, 34.0));
        let search = Rect::from_min_max(pos2(480.0, 6.0), pos2(960.0, 34.0));
        CaptionRegions::from_points(caption, icon, buttons, &[trail, search], pixels_per_point)
    }

    fn frame(pixels_per_point: f32, is_maximized: bool) -> WindowFrame {
        WindowFrame {
            is_maximized,
            resize_border: (8.0 * pixels_per_point) as i32,
            width: (1440.0 * pixels_per_point) as i32,
        }
    }

    fn at(x: f32, y: f32, pixels_per_point: f32, is_maximized: bool) -> Hit {
        let px = |value: f32| (value * pixels_per_point) as i32;
        hit_test(px(x), px(y), &regions(pixels_per_point), frame(pixels_per_point, is_maximized))
    }

    #[test]
    fn empty_title_bar_space_drags_the_window() {
        for scale in [1.0, 1.5, 2.0] {
            assert_eq!(
                at(400.0, 20.0, scale, false),
                Hit::Caption,
                "between trail and search at {scale}x"
            );
            assert_eq!(
                at(1100.0, 20.0, scale, false),
                Hit::Caption,
                "right of the search at {scale}x"
            );
        }
    }

    #[test]
    fn widgets_in_the_title_bar_stay_clickable() {
        for scale in [1.0, 1.25, 2.0] {
            assert_eq!(at(100.0, 20.0, scale, false), Hit::Client, "breadcrumb at {scale}x");
            assert_eq!(at(700.0, 20.0, scale, false), Hit::Client, "search box at {scale}x");
        }
    }

    #[test]
    fn caption_buttons_answer_their_codes() {
        assert_eq!(at(1440.0 - 23.0, 20.0, 1.0, false), Hit::Button(CaptionButton::Close));
        assert_eq!(
            at(1440.0 - 46.0 - 23.0, 20.0, 1.0, false),
            Hit::Button(CaptionButton::Maximize)
        );
        assert_eq!(
            at(1440.0 - 92.0 - 23.0, 20.0, 1.0, false),
            Hit::Button(CaptionButton::Minimize)
        );
        // 4K at 200 %: the same points, twice the pixels.
        assert_eq!(at(1440.0 - 46.0 - 1.0, 39.0, 2.0, false), Hit::Button(CaptionButton::Maximize));
    }

    #[test]
    fn the_top_edge_resizes_unless_maximized() {
        assert_eq!(at(700.0, 2.0, 1.0, false), Hit::Top, "even over the search box");
        assert_eq!(
            at(1440.0 - 23.0, 2.0, 1.0, false),
            Hit::Top,
            "the close button's top edge resizes"
        );
        assert_eq!(at(1440.0 - 4.0, 2.0, 1.0, false), Hit::TopRight);
        assert_eq!(at(4.0, 2.0, 1.0, false), Hit::TopLeft);
        assert_eq!(at(700.0, 9.0, 1.0, false), Hit::Client, "below the band");
        // Maximized: no resizing, the buttons reach the screen edge (Fitts's law).
        assert_eq!(at(1440.0 - 1.0, 0.0, 1.0, true), Hit::Button(CaptionButton::Close));
        assert_eq!(at(700.0, 20.0, 1.0, true), Hit::Client);
        assert_eq!(at(400.0, 0.0, 1.0, true), Hit::Caption);
    }

    #[test]
    fn the_resize_band_follows_the_dpi() {
        // 8 pt band: 12 px at 150 %, 16 px at 200 %.
        assert_eq!(at(400.0, 7.5, 1.5, false), Hit::Top);
        assert_eq!(at(400.0, 8.5, 1.5, false), Hit::Caption);
        assert_eq!(at(400.0, 7.5, 2.0, false), Hit::Top);
    }

    #[test]
    fn the_icon_opens_the_system_menu() {
        assert_eq!(at(20.0, 20.0, 1.0, false), Hit::SystemMenu);
        assert_eq!(at(20.0, 20.0, 1.0, true), Hit::SystemMenu);
    }

    #[test]
    fn below_the_title_bar_is_client() {
        assert_eq!(at(400.0, 41.0, 1.0, false), Hit::Client);
        assert_eq!(at(1440.0 - 23.0, 41.0, 1.0, false), Hit::Client);
        assert_eq!(
            hit_test(10, 10, &CaptionRegions::default(), frame(1.0, true)),
            Hit::Client,
            "nothing published yet"
        );
    }

    #[test]
    fn extra_holes_are_dropped_not_overflowed() {
        let hole = Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0));
        let holes = vec![hole; MAX_HOLES + 3];
        let regions = CaptionRegions::from_points(
            Rect::NOTHING,
            Rect::NOTHING,
            [Rect::NOTHING; 3],
            &holes,
            1.0,
        );
        assert_eq!(regions.holes().len(), MAX_HOLES);
    }
}
