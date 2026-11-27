//! Vertical scroll areas for padded pages (cards, forms): the content keeps its padding on all
//! sides and a stable gutter on the right for the floating scroll bar, so the bar never sits on
//! the content and the layout does not shift when the bar appears. And [`sideways`] for blocks
//! wider than their card (code, tables), and [`placed`] for a view that puts its scroll offset
//! where it wants it.

use cv_base::theme::style;
use egui::{Margin, ScrollArea, Ui};

use crate::surfaces;

/// Content that keeps its width (code lines, table columns) and scrolls sideways when its card is
/// narrower. While it overflows, middle-click autoscroll moves it sideways too.
pub fn sideways<R>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    let output = ScrollArea::horizontal().id_salt(id_salt).show(ui, add_contents);
    if output.content_size.x > output.inner_rect.width() {
        // The output's inner rect does not follow the content's height on a sideways-only area.
        let shown = egui::Rect::from_min_size(
            output.inner_rect.min,
            egui::vec2(output.inner_rect.width(), output.content_size.y),
        );
        surfaces::mark_horizontal_scroll(ui, shown);
    }
    output.inner
}

/// A full-size vertical scroll area whose content is padded by `padding` plus the scroll gutter
/// on the right. The padding scrolls with the content, as in a browser page.
pub fn page(id_salt: &str, padding: Margin) -> ScrollArea {
    ScrollArea::vertical().id_salt(id_salt).auto_shrink(false).content_margin(with_gutter(padding))
}

/// `padding` with the scroll gutter added on the right.
pub fn with_gutter(padding: Margin) -> Margin {
    Margin { right: padding.right + style::SCROLL_GUTTER as i8, ..padding }
}

/// `area` at the vertical `offset` the view places it at this frame, if it does, shown in `ui`, a
/// scope that holds only the area. egui measures the area's end from last frame's content, and
/// the content may have grown since (cards measured, comments shown, a longer log): a placed
/// offset past that end is where the view goes, not a pull past it. Rubber-banding would spring
/// it back and leave the view coasting, so it is off in `ui` for that frame (scroll areas nested in
/// the content inherit that) and egui clamps the offset to this frame's content instead. The
/// wheel, a drag and a fling keep it.
pub fn placed(ui: &mut Ui, area: ScrollArea, offset: Option<f32>) -> ScrollArea {
    let Some(offset) = offset else {
        return area;
    };
    ui.spacing_mut().scroll.kinetic.rubber_band = false;
    area.vertical_scroll_offset(offset)
}

#[cfg(test)]
mod tests {
    use egui::{ScrollArea, Ui};
    use egui_kittest::Harness;

    /// Content `height` tall, placed at `offset` in the next frame when set; `shown` is where the
    /// area stood after each frame and how fast it moved.
    struct Area {
        height: f32,
        offset: Option<f32>,
        shown: (f32, f32),
    }

    fn show(ui: &mut Ui, area: &mut Area) {
        let scroll = ScrollArea::vertical().id_salt("placed").auto_shrink(false);
        let offset = area.offset.take();
        let height = area.height;
        let output = ui
            .scope(|ui| super::placed(ui, scroll, offset).show(ui, |ui| ui.set_min_height(height)))
            .inner;
        area.shown = (output.state.unclamped_offset().y, output.state.velocity().y);
    }

    /// The content grows in the frame an offset past its old end is placed: the area stays at that
    /// offset, without a spring back towards the old end and the coast that follows it.
    #[test]
    fn an_offset_placed_past_the_old_end_stays_where_it_was_placed() {
        let area = Area { height: 1_000.0, offset: None, shown: (0.0, 0.0) };
        let mut harness = Harness::builder().with_size([300.0, 200.0]).build_ui_state(show, area);
        harness.run();
        let area = harness.state_mut();
        area.height = 2_000.0;
        area.offset = Some(1_500.0);
        for step in 0..12 {
            harness.step();
            assert_eq!(harness.state().shown, (1_500.0, 0.0), "step {step} after the placement");
        }
    }
}
