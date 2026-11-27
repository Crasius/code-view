//! Screen regions that behave differently under app-wide gestures, registered while painting.
//!
//! * [`mark_code`]: a code surface (diff, the folder view's cards, file view, log, file preview).
//!   Ctrl+wheel over it, or Ctrl+Plus/Minus/0 while keyboard focus is inside it, changes the code
//!   font size instead of zooming the whole app.
//! * [`mark_horizontal_scroll`]: content that also scrolls sideways; middle-click autoscroll then
//!   moves in both directions and shows a four-way marker.
//! * [`mark_pan`]: a canvas the user drags around with the middle button (and the primary one)
//!   and zooms itself; middle-click autoscroll leaves it alone, and Ctrl+wheel over it, or
//!   Ctrl+Plus/Minus/0 over it or with keyboard focus inside it, are its own, not the app zoom's.
//! * `mark_middle_click`: a control that acts on a middle click (a link that opens its page);
//!   middle-click autoscroll does not start over it, so the click reaches the control.
//!
//! Views call these every frame they are shown; the app reads the previous frame's marks, so a
//! view that stops calling them stops counting one frame later. Call [`begin_frame`] once at the
//! start of each frame.

use egui::{Context, Id, Pos2, Rect, Ui};

const STORE_ID: &str = "review.surfaces";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Code,
    HorizontalScroll,
    Pan,
    MiddleClick,
}

#[derive(Clone, Default)]
struct Marks {
    previous: Vec<(Kind, Rect)>,
    current: Vec<(Kind, Rect)>,
}

pub fn begin_frame(ctx: &Context) {
    ctx.data_mut(|data| {
        let marks = data.get_temp_mut_or_default::<Marks>(Id::unique(STORE_ID));
        marks.previous = std::mem::take(&mut marks.current);
    });
}

/// `rect` (clipped to what `ui` shows) is code whose font size follows the code zoom.
pub fn mark_code(ui: &Ui, rect: Rect) {
    mark(ui, Kind::Code, rect);
}

/// `rect` (clipped to what `ui` shows) scrolls horizontally as well as vertically.
pub fn mark_horizontal_scroll(ui: &Ui, rect: Rect) {
    mark(ui, Kind::HorizontalScroll, rect);
}

/// `rect` (clipped to what `ui` shows) is a canvas that pans with a middle-button drag and
/// zooms itself.
pub fn mark_pan(ui: &Ui, rect: Rect) {
    mark(ui, Kind::Pan, rect);
}

/// `rect` (clipped to what `ui` shows) is a control that takes the middle click itself.
pub(crate) fn mark_middle_click(ui: &Ui, rect: Rect) {
    mark(ui, Kind::MiddleClick, rect);
}

pub fn takes_middle_click_at(ctx: &Context, pos: Pos2) -> bool {
    contains(ctx, Kind::MiddleClick, |rect| rect.contains(pos))
}

pub fn is_code_at(ctx: &Context, pos: Pos2) -> bool {
    contains(ctx, Kind::Code, |rect| rect.contains(pos))
}

/// The code surface that holds `rect` (a focused widget, for example).
pub fn is_code_around(ctx: &Context, rect: Rect) -> bool {
    contains(ctx, Kind::Code, |surface| surface.contains_rect(rect))
}

pub fn scrolls_horizontally_at(ctx: &Context, pos: Pos2) -> bool {
    contains(ctx, Kind::HorizontalScroll, |rect| rect.contains(pos))
}

pub fn pans_at(ctx: &Context, pos: Pos2) -> bool {
    contains(ctx, Kind::Pan, |rect| rect.contains(pos))
}

/// The pan canvas that holds `rect` (a focused widget, for example).
pub fn is_pan_around(ctx: &Context, rect: Rect) -> bool {
    contains(ctx, Kind::Pan, |surface| surface.contains_rect(rect))
}

/// Every code, sideways scrolling and pan region marked this frame or the last: they cut their
/// content at their edges by design, which layout checks leave alone (`app::clip_check`, built
/// with debug assertions only). Middle-click controls clip nothing.
#[cfg(all(debug_assertions, any(test, feature = "test-support")))]
pub fn clipping_regions(ctx: &Context) -> Vec<Rect> {
    ctx.data(|data| {
        data.get_temp::<Marks>(Id::unique(STORE_ID))
            .map(|marks| {
                marks
                    .previous
                    .iter()
                    .chain(&marks.current)
                    .filter(|(kind, _)| *kind != Kind::MiddleClick)
                    .map(|(_, rect)| *rect)
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn mark(ui: &Ui, kind: Kind, rect: Rect) {
    let shown = rect.intersect(ui.clip_rect());
    if !shown.is_positive() {
        return;
    }
    ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Marks>(Id::unique(STORE_ID)).current.push((kind, shown));
    });
}

fn contains(ctx: &Context, kind: Kind, test: impl Fn(Rect) -> bool) -> bool {
    ctx.data(|data| {
        data.get_temp::<Marks>(Id::unique(STORE_ID)).is_some_and(|marks| {
            marks
                .previous
                .iter()
                .chain(&marks.current)
                .any(|(mark_kind, rect)| *mark_kind == kind && test(*rect))
        })
    })
}
