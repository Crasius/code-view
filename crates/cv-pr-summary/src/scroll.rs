//! The scroll area the cards scroll in: its margins, what scrolls it (the wheel and the scroll
//! bar; a drag on the code selects it, as in the file view) and the offsets the summary places
//! it at (a key, a jump to a card, the card at the top kept in place while heights change).

use egui::scroll_area::{ScrollAreaOutput, ScrollSource};
use egui::{Rect, ScrollArea, Ui, UiBuilder};

use crate::MARGIN;
use cv_base::theme::style;

/// Shows `add_contents` scrolled in `area`, one area per folder; at `placed` when the summary
/// places the view this frame.
pub fn show<R>(
    ui: &mut Ui,
    area: Rect,
    folder: &str,
    placed: Option<f32>,
    add_contents: impl FnOnce(&mut Ui, Rect) -> R,
) -> ScrollAreaOutput<R> {
    let margin = egui::Margin {
        left: MARGIN as i8,
        right: (MARGIN + style::SCROLL_GUTTER) as i8,
        top: MARGIN as i8,
        bottom: MARGIN as i8,
    };
    let mut scroll = ScrollArea::vertical()
        .id_salt(("folder_summary", folder))
        .auto_shrink(false)
        .scroll_source(ScrollSource::SCROLL_BAR | ScrollSource::MOUSE_WHEEL)
        .content_margin(margin);
    if let Some(offset) = placed {
        scroll = scroll.vertical_scroll_offset(offset);
    }
    ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
        // egui measures the edge from last frame's content, and the cards may have grown since
        // (a jump to the end, then a card above the view measured). A placed offset past that
        // edge is where the view goes, not a pull past it: rubber-banding would spring it back
        // and leave the view coasting, so that every later key missed its target.
        if placed.is_some() {
            ui.spacing_mut().scroll.kinetic.rubber_band = false;
        }
        scroll.show_viewport(ui, add_contents)
    })
    .inner
}
