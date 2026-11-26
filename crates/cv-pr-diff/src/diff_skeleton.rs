//! Placeholder while a diff loads, drawn with the real row metrics: same gutters, line height,
//! gap bar and overview strip, so the diff replaces it without anything moving.

use egui::{Rect, Sense, Ui, pos2, vec2};

use crate::diff_view::OVERVIEW_WIDTH;
use crate::gap_bar;
use cv_base::theme;
use cv_code::diff_paint::Metrics;
use cv_code::diff_rows::Layout;
use cv_widgets::skeleton;

/// `shown` is how much of the placeholder to draw ([`skeleton::reveal`]). The diff's background
/// is always there; at 0 nothing else is, so a quick load swaps the next diff in directly.
pub fn show(ui: &mut Ui, layout: Layout, font_size: f32, shown: f32) {
    let palette = theme::palette(ui.ctx());
    let metrics = Metrics::new(ui, font_size, 0);
    let full = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(full, Sense::hover());
    let painter = ui.painter_at(full);
    painter.rect_filled(full, 0, palette.surface);
    let strip = Rect::from_min_max(pos2(full.right() - OVERVIEW_WIDTH, full.top()), full.max);
    painter.rect_filled(strip, 0, palette.surface_subtle);
    if shown <= 0.0 {
        return;
    }
    skeleton::announce(&response, "Loading the diff");
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(full));
    ui.set_clip_rect(full.intersect(ui.clip_rect()));
    ui.multiply_opacity(shown);
    let ui = &ui;
    let painter = ui.painter();
    let content = Rect::from_min_max(full.min, pos2(strip.left(), full.bottom()));
    let single_pane = layout.is_single_pane();
    let two_numbers = layout.has_two_numbers();
    let panes = if single_pane { 1 } else { 2 };
    let pane_width = content.width() / panes as f32;
    let bar_height = (metrics.line_height * 0.42).round();

    let gap = Rect::from_min_size(content.min, vec2(content.width(), metrics.gap_height));
    painter.rect_filled(gap, 0, gap_bar::background(&palette));
    let gap_text =
        pos2(content.left() + metrics.text_inset(two_numbers), gap.center().y - bar_height / 2.0);
    skeleton::paint_bar(ui, Rect::from_min_size(gap_text, vec2(110.0, bar_height)));

    let mut top = gap.bottom();
    let mut index = 0;
    while top < content.bottom() {
        for pane in 0..panes {
            let left = content.left() + pane as f32 * pane_width;
            let center_y = top + metrics.line_height / 2.0;
            let number_columns = if two_numbers { 2 } else { 1 };
            for column in 0..number_columns {
                let right = left + metrics.number_width * (column as f32 + 1.0) - 10.0;
                let width = metrics.char_width * if (index + column) % 3 == 0 { 3.0 } else { 2.0 };
                skeleton::paint_bar(
                    ui,
                    Rect::from_min_size(
                        pos2(right - width, center_y - bar_height / 2.0),
                        vec2(width, bar_height),
                    ),
                );
            }
            let indent =
                [0.0, 4.0, 8.0, 8.0, 12.0, 4.0, 0.0, 8.0][(index + pane) % 8] * metrics.char_width;
            let text_left = left + metrics.text_inset(two_numbers) + indent;
            let room = (left + pane_width - text_left - 24.0).max(0.0);
            let width =
                (metrics.char_width * (12 + (index * 37 + pane * 11) % 46) as f32).min(room);
            let is_blank = (index + 3) % 9 == 0;
            if !is_blank && width > 0.0 {
                skeleton::paint_bar(
                    ui,
                    Rect::from_min_size(
                        pos2(text_left, center_y - bar_height / 2.0),
                        vec2(width, bar_height),
                    ),
                );
            }
        }
        if !single_pane {
            let divider = content.left() + pane_width;
            painter.vline(
                divider,
                top..=top + metrics.line_height,
                egui::Stroke::new(1.0, palette.border),
            );
        }
        top += metrics.line_height;
        index += 1;
    }
}
