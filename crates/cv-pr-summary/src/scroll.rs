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
    let scroll = ScrollArea::vertical()
        .id_salt(("folder_summary", folder))
        .auto_shrink(false)
        .scroll_source(ScrollSource::SCROLL_BAR | ScrollSource::MOUSE_WHEEL)
        .content_margin(margin);
    ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
        cv_widgets::scroll::placed(ui, scroll, placed).show_viewport(ui, add_contents)
    })
    .inner
}
