//! Vertical scroll areas for padded pages (cards, forms): the content keeps its padding on all
//! sides and a stable gutter on the right for the floating scroll bar, so the bar never sits on
//! the content and the layout does not shift when the bar appears. And [`sideways`] for blocks
//! wider than their card (code, tables).

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
