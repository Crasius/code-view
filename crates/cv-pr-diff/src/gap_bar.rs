//! The bar standing in for collapsed unchanged lines: expand buttons in the gutter, the hidden
//! line count and the enclosing function git names for the hunk below.

use egui::{Align2, Color32, CursorIcon, FontId, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

use cv_base::theme::{self, Palette, icons, style};
use cv_code::diff_paint::{Metrics, mix};
use cv_code::gaps::{EXPAND_STEP, Expand, Gap, Reveal};
use cv_widgets::numbers::group_digits;

const BUTTON_WIDTH: f32 = 30.0;

pub struct GapBar<'a> {
    pub gap: &'a Gap,
    pub reveal: Reveal,
    /// Function or section heading of the hunk below, empty when git found none.
    pub section: &'a str,
    /// File contents are loaded, so hidden lines can be shown.
    pub can_expand: bool,
    /// Inline gutter: base and head numbers before the text.
    pub has_two_numbers: bool,
}

pub fn background(palette: &Palette) -> Color32 {
    mix(palette.surface, palette.accent, if palette.is_dark { 0.10 } else { 0.06 })
}

/// A compact diff's unchanged lines left out: a thin band in the bar's tint, dots in both number
/// columns.
pub fn paint_separator(painter: &egui::Painter, rect: Rect, metrics: &Metrics, palette: &Palette) {
    painter.rect_filled(rect, 0, background(palette));
    let edge = Stroke::new(1.0, palette.border);
    painter.hline(rect.x_range(), rect.top() + 0.5, edge);
    painter.hline(rect.x_range(), rect.bottom() - 0.5, edge);
    for column in 0..2 {
        let center =
            pos2(rect.left() + metrics.number_width * (column as f32 + 0.5), rect.center().y);
        painter.text(
            center,
            Align2::CENTER_CENTER,
            "\u{22EF}",
            metrics.number_font.clone(),
            palette.text_secondary,
        );
    }
}

pub fn show(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    bar: &GapBar<'_>,
    metrics: &Metrics,
) -> Option<Expand> {
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0, background(&palette));
    let edge = Stroke::new(
        1.0,
        mix(palette.surface, palette.accent, if palette.is_dark { 0.22 } else { 0.16 }),
    );
    painter.hline(rect.x_range(), rect.top() + 0.5, edge);
    painter.hline(rect.x_range(), rect.bottom() - 0.5, edge);

    let hidden = bar.reveal.hidden(bar.gap);
    let mut expand = None;
    let mut x = rect.left() + style::SPACE_S;
    if bar.can_expand {
        for (action, icon, tooltip) in buttons(bar.gap, hidden) {
            let button = Rect::from_min_size(
                pos2(x, rect.top() + 1.0),
                vec2(BUTTON_WIDTH, rect.height() - 2.0),
            );
            if expand_button(ui, button, id.with(icon), icon, &tooltip, &palette) {
                expand = Some(action);
            }
            x += BUTTON_WIDTH;
        }
    }

    let text_left = rect.left() + metrics.text_inset(bar.has_two_numbers);
    let lines = if hidden == 1 { "line" } else { "lines" };
    let label = format!("{} hidden {lines}", group_digits(hidden as usize));
    let label_font = FontId::proportional(style::SMALL_SIZE);
    let label_rect = painter.text(
        pos2(text_left, rect.center().y),
        Align2::LEFT_CENTER,
        &label,
        label_font,
        palette.text_secondary,
    );
    if bar.can_expand {
        let response = ui
            .interact(
                label_rect.expand2(vec2(style::SPACE_S, style::SPACE_XS)),
                id.with("all"),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!("Show all {} lines", group_digits(hidden as usize)));
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Show {label}"))
        });
        if response.hovered() {
            painter.hline(
                label_rect.x_range(),
                label_rect.bottom() + 1.0,
                Stroke::new(1.0, palette.text_secondary),
            );
        }
        cv_widgets::focus::ring(ui, &response, label_rect.expand(3.0));
        if response.clicked() {
            expand = Some(Expand::All);
        }
    }
    if !bar.section.is_empty() {
        painter.text(
            pos2(label_rect.right() + style::SPACE_XL, rect.center().y),
            Align2::LEFT_CENTER,
            bar.section,
            metrics.font.clone(),
            palette.text_secondary,
        );
    }
    expand
}

fn buttons(gap: &Gap, hidden: u32) -> Vec<(Expand, &'static str, String)> {
    let all = (
        Expand::All,
        icons::ARROWS_OUT_LINE_VERTICAL,
        format!("Show all {} hidden lines", group_digits(hidden as usize)),
    );
    if hidden <= EXPAND_STEP {
        return vec![all];
    }
    let mut buttons = Vec::with_capacity(2);
    if gap.can_expand_up() {
        buttons.push((
            Expand::Up,
            icons::ARROW_LINE_UP,
            format!("Expand up: show {EXPAND_STEP} more lines above the change"),
        ));
    }
    if gap.can_expand_down() {
        buttons.push((
            Expand::Down,
            icons::ARROW_LINE_DOWN,
            format!("Expand down: show {EXPAND_STEP} more lines below the change"),
        ));
    }
    buttons
}

fn expand_button(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    icon: &str,
    tooltip: &str,
    palette: &Palette,
) -> bool {
    let response = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tooltip);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tooltip));
    if response.hovered() {
        ui.painter().rect_filled(
            rect.shrink2(vec2(2.0, 2.0)),
            style::RADIUS_CONTROL,
            mix(palette.surface, palette.accent, 0.22),
        );
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        FontId::proportional(icons::SIZE_SMALL),
        palette.accent,
    );
    cv_widgets::focus::ring(ui, &response, rect);
    response.clicked()
}
