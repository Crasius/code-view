//! How the merge button and its dropdown's rows look: the split button's fills, icon, label,
//! caret and the draining confirm bar (a clock for auto-complete); the strategy radios with their "Default" chip.
//! The page's merge button and the Settings preview of it both paint with these, so the module and
//! the button's measures live in `cv-pr-session`, below both.

use cv_base::theme::{self, icons, motion, style};
use cv_widgets::{focus, merge_strategy_icon, row, states};
use egui::epaint::CornerRadius;
use egui::{Align2, Color32, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use review_core::{MergeStrategy, ProviderKind};

use crate::merge_choice;

/// The width of the split button's chevron part.
pub const CHEVRON_WIDTH: f32 = 30.0;
/// The padding left and right of the main part's content.
pub const PADDING_X: f32 = style::SPACE_M;
/// The gap between the main part's icon and its label.
pub const ICON_GAP: f32 = 6.0;
const DRAIN_HEIGHT: f32 = 3.0;
/// How far the divider between the two parts stays off the button's top and bottom.
const DIVIDER_INSET: f32 = 6.0;
const ROW_ICON: f32 = 20.0;
const BADGE_PADDING: egui::Vec2 = vec2(6.0, 1.0);
const BADGE_RADIUS: u8 = 8;

pub struct Look<'a> {
    pub strategy: Option<MergeStrategy>,
    pub label: &'a str,
    /// Share of the confirm window left while armed.
    pub remaining: Option<f32>,
    pub is_enabled: bool,
    pub is_busy: bool,
    pub is_open: bool,
}

/// The button's tone: as usual, or reporting a refused merge.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    /// The platform refused the last merge: error fill and a warning icon.
    Failed,
    /// The button sets or cancels auto-complete: a clock instead of the strategy's icon.
    Auto,
}

/// The split button in `tone`: both parts' fills, the label, the caret and the drain bar.
pub fn paint_in(
    ui: &Ui,
    main: &Response,
    chevron: &Response,
    rect: Rect,
    look: &Look<'_>,
    tone: Tone,
) {
    let is_failed = tone == Tone::Failed;
    if !ui.is_rect_visible(rect) {
        return;
    }
    let palette = theme::palette(ui.ctx());
    let armed =
        motion::toggle(ui.ctx(), main.id.with("armed"), look.remaining.is_some(), motion::FAST);
    let failed = motion::toggle(ui.ctx(), main.id.with("failed"), is_failed, motion::FAST);
    let rest = motion::mix(palette.brand, palette.error, failed);
    let rest_hover =
        motion::mix(palette.brand_hover, motion::mix(palette.error, palette.text, 0.12), failed);
    let rest_pressed =
        motion::mix(palette.brand_pressed, motion::mix(palette.error, palette.text, 0.25), failed);
    let base = motion::mix(rest, palette.success, armed);
    let hover = motion::mix(rest_hover, motion::mix(palette.success, palette.text, 0.12), armed);
    let pressed =
        motion::mix(rest_pressed, motion::mix(palette.success, palette.text, 0.25), armed);
    let foreground = if look.is_enabled {
        motion::mix(palette.on_brand, palette.on_accent, armed.max(failed))
    } else {
        palette.text_secondary
    };
    let fill = |response: &Response| {
        if !look.is_enabled {
            palette.pressed
        } else if response.is_pointer_button_down_on() {
            pressed
        } else {
            motion::mix(base, hover, row::hover_amount(ui, response))
        }
    };
    let radius = style::RADIUS_CONTROL;
    let main_fill = fill(main);
    ui.painter().rect_filled(
        main.rect,
        CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 },
        main_fill,
    );
    ui.painter().rect_filled(
        chevron.rect,
        CornerRadius { nw: 0, sw: 0, ne: radius, se: radius },
        fill(chevron),
    );
    let divider_x = main.rect.right();
    ui.painter().vline(
        divider_x,
        rect.y_range().shrink(DIVIDER_INSET),
        Stroke::new(1.0, foreground.gamma_multiply(0.4)),
    );
    paint_label(ui, main.rect, look, (foreground, main_fill), tone);
    let caret = if look.is_open { icons::CARET_UP } else { icons::CARET_DOWN };
    ui.painter().text(
        chevron.rect.center(),
        Align2::CENTER_CENTER,
        caret,
        icons::rest_font(&palette, icons::SIZE_SMALL),
        foreground,
    );
    if let Some(remaining) = look.remaining {
        let bar = Rect::from_min_size(
            pos2(rect.left(), rect.bottom() - DRAIN_HEIGHT),
            vec2(rect.width() * remaining, DRAIN_HEIGHT),
        );
        let corners = CornerRadius {
            nw: 0,
            ne: 0,
            sw: radius,
            se: if remaining > 0.98 { radius } else { 0 },
        };
        ui.painter().rect_filled(bar, corners, foreground.gamma_multiply(0.75));
    }
    focus::ring(ui, chevron, chevron.rect);
}

/// The strategy's icon (a spinner while busy) and the label, centered in the main part.
fn paint_label(
    ui: &Ui,
    main: Rect,
    look: &Look<'_>,
    (foreground, fill): (Color32, Color32),
    tone: Tone,
) {
    let palette = theme::palette(ui.ctx());
    let label = ui.painter().layout_no_wrap(
        look.label.to_owned(),
        egui::FontId::proportional(style::BODY_SIZE),
        foreground,
    );
    let content = icons::SIZE + ICON_GAP + label.size().x;
    let left = main.center().x - content / 2.0;
    let slot = Rect::from_center_size(
        pos2(left + icons::SIZE / 2.0, main.center().y),
        vec2(icons::SIZE, icons::SIZE),
    );
    match look.strategy {
        _ if look.is_busy => {
            states::spinner(ui, slot, foreground);
        }
        _ if tone != Tone::Normal => {
            let icon = if tone == Tone::Failed { icons::WARNING_CIRCLE } else { icons::CLOCK };
            ui.painter().text(
                slot.center(),
                Align2::CENTER_CENTER,
                icon,
                icons::rest_font(&palette, icons::SIZE),
                foreground,
            );
        }
        Some(strategy) => merge_strategy_icon::paint(
            ui.painter(),
            &palette,
            slot.center(),
            icons::SIZE,
            strategy,
            foreground,
            fill,
        ),
        None => {
            ui.painter().text(
                slot.center(),
                Align2::CENTER_CENTER,
                icons::GIT_MERGE,
                icons::rest_font(&palette, icons::SIZE),
                foreground,
            );
        }
    }
    ui.painter().galley(
        pos2(left + icons::SIZE + ICON_GAP, main.center().y - label.size().y / 2.0),
        label,
        foreground,
    );
}

pub fn divider(ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), style::SPACE_S), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, palette.border));
}

pub struct Row {
    pub strategy: MergeStrategy,
    pub kind: ProviderKind,
    pub is_selected: bool,
    pub is_default: bool,
    /// The number key that picks it; `None` when there is nothing to pick from.
    pub number: Option<usize>,
    pub explanation: String,
}

/// A strategy option (menu item radio): glyph, name, "Default" badge, explanation, number key.
pub fn strategy_row(ui: &mut Ui, shown: &Row) -> Response {
    let palette = theme::palette(ui.ctx());
    let width = ui.available_width();
    let text_left = style::SPACE_S + ROW_ICON + style::SPACE_M;
    let hint_width = if shown.number.is_some() { style::SPACE_L + style::SPACE_S } else { 0.0 };
    let wrap = (width - text_left - style::SPACE_S - hint_width).max(80.0);
    let name = merge_choice::button_label(shown.strategy, shown.kind);
    let name_galley = ui.painter().layout_no_wrap(
        name.to_owned(),
        theme::fonts::semibold_font(style::BODY_SIZE),
        palette.text,
    );
    let explain_galley = ui.painter().layout(
        shown.explanation.clone(),
        egui::FontId::proportional(style::SMALL_SIZE),
        palette.text_secondary,
        wrap,
    );
    let height =
        style::SPACE_S * 2.0 + name_galley.size().y + style::SPACE_LINE + explain_galley.size().y;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::MenuItemRadio, true, shown.is_selected, name)
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let selected =
        motion::toggle(ui.ctx(), response.id.with("selected"), shown.is_selected, motion::FAST);
    let fill = motion::mix(
        row::state_fill(ui, &response, Color32::TRANSPARENT),
        palette.selection,
        selected,
    );
    let ground = if fill == Color32::TRANSPARENT { ui.visuals().window_fill } else { fill };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    }
    if selected > 0.0 {
        let bar = Rect::from_center_size(
            pos2(rect.left() + 1.5, rect.center().y),
            vec2(3.0, (rect.height() - style::SPACE_M) * selected),
        );
        ui.painter().rect_filled(bar, 2, palette.accent.gamma_multiply(selected));
    }
    let name_top = rect.top() + style::SPACE_S;
    let name_height = name_galley.size().y;
    let name_center = name_top + name_height / 2.0;
    let icon_color = motion::mix(palette.text, palette.accent_text, selected);
    merge_strategy_icon::paint(
        ui.painter(),
        &palette,
        pos2(rect.left() + style::SPACE_S + ROW_ICON / 2.0, name_center),
        ROW_ICON,
        shown.strategy,
        icon_color,
        ground,
    );
    let name_right = rect.left() + text_left + name_galley.size().x;
    ui.painter().galley(pos2(rect.left() + text_left, name_top), name_galley, palette.text);
    if shown.is_default {
        default_badge(ui, pos2(name_right + style::SPACE_S, name_center));
    }
    ui.painter().galley(
        pos2(rect.left() + text_left, name_top + name_height + style::SPACE_LINE),
        explain_galley,
        palette.text_secondary,
    );
    if let Some(number) = shown.number {
        ui.painter().text(
            pos2(rect.right() - style::SPACE_M, name_center),
            Align2::RIGHT_CENTER,
            number.to_string(),
            egui::FontId::proportional(style::SMALL_SIZE),
            palette.text_secondary,
        );
    }
    focus::ring(ui, &response, rect);
    response
}

/// "Default" in a quiet outlined chip, left edge at `left_center`.
fn default_badge(ui: &Ui, left_center: egui::Pos2) {
    let palette = theme::palette(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        "Default".to_owned(),
        egui::FontId::proportional(style::SMALL_SIZE),
        palette.text_secondary,
    );
    let rect = Rect::from_min_size(
        pos2(left_center.x, left_center.y - galley.size().y / 2.0 - BADGE_PADDING.y),
        galley.size() + BADGE_PADDING * 2.0,
    );
    ui.painter().rect(
        rect,
        BADGE_RADIUS,
        palette.surface_subtle,
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(rect.min + BADGE_PADDING, galley, palette.text_secondary);
}
