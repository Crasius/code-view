//! Azure DevOps button styles: primary (filled), secondary (outlined), danger (filled, for
//! confirming a destructive step) and subtle (command bar), plus icon-only buttons and toggles.
//! Hover eases in, press darkens immediately, disabled buttons are muted.
//!
//! Icons are painted as their own glyph at [`icons::SIZE`] in the theme's weight (see
//! [`cv_base::theme::icons`]), centred on the button, never as part of the label string. A button
//! that can be busy keeps its icon slot and swaps the icon for a spinner, so its width never
//! changes while the work runs. An icon slot also takes a platform mark ([`super::provider_icon::glyph`]),
//! in its brand color on outlined and borderless buttons, in the label color on filled ones.

use std::sync::Arc;

use cv_base::theme::{self, icons, motion, style};
use egui::{
    Color32, Galley, Rect, Response, Sense, Stroke, StrokeKind, TextStyle, TextWrapMode, Ui,
    WidgetText, pos2, vec2,
};

use super::row;

/// Height of subtle and icon-only buttons.
pub const SUBTLE_HEIGHT: f32 = 28.0;
const ICON_GAP: f32 = 6.0;
/// Horizontal padding of icon-only buttons; with a 16 px glyph this makes them square.
const ICON_ONLY_PADDING: f32 = (SUBTLE_HEIGHT - icons::SIZE) / 2.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Primary,
    Secondary,
    Danger,
    Subtle,
}

/// What a button shows: an optional icon slot and an optional label.
struct Content<'a> {
    icon: Option<&'a str>,
    is_busy: bool,
    is_active: bool,
    label: Option<Arc<Galley>>,
    /// Name for screen readers when the visible content is only an icon.
    accessible: Option<&'a str>,
}

/// The one filled accent button per screen.
pub fn primary(ui: &mut Ui, text: &str) -> Response {
    let label = label_galley(ui, text, Kind::Primary);
    let content = Content {
        icon: None,
        is_busy: false,
        is_active: false,
        label: Some(label),
        accessible: None,
    };
    paint(ui, Kind::Primary, content, style::CONTROL_HEIGHT)
}

/// Primary button with a leading icon.
pub fn primary_icon(ui: &mut Ui, icon: &str, text: &str) -> Response {
    primary_busy(ui, icon, text, false)
}

/// Primary button whose icon turns into a spinner while `is_busy`; it is disabled meanwhile and
/// keeps its width and label, so starting the work never resizes or relabels it.
pub fn primary_busy(ui: &mut Ui, icon: &str, text: &str, is_busy: bool) -> Response {
    let label = label_galley(ui, text, Kind::Primary);
    let content = Content {
        icon: Some(icon),
        is_busy,
        is_active: false,
        label: Some(label),
        accessible: None,
    };
    paint(ui, Kind::Primary, content, style::CONTROL_HEIGHT)
}

/// Filled error-coloured button that confirms a destructive step (e.g. discarding a draft).
pub fn danger(ui: &mut Ui, icon: &str, text: &str) -> Response {
    let label = label_galley(ui, text, Kind::Danger);
    let content = Content {
        icon: Some(icon),
        is_busy: false,
        is_active: false,
        label: Some(label),
        accessible: None,
    };
    paint(ui, Kind::Danger, content, style::CONTROL_HEIGHT)
}

/// Bordered neutral button.
pub fn secondary(ui: &mut Ui, text: impl Into<WidgetText>) -> Response {
    let label =
        text.into().into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Button);
    let content = Content {
        icon: None,
        is_busy: false,
        is_active: false,
        label: Some(label),
        accessible: None,
    };
    paint(ui, Kind::Secondary, content, style::CONTROL_HEIGHT)
}

/// Bordered neutral button with a leading icon.
pub fn secondary_icon(ui: &mut Ui, icon: &str, text: &str) -> Response {
    secondary_busy(ui, icon, text, false)
}

/// Bordered button whose icon turns into a spinner while `is_busy` (disabled meanwhile).
pub fn secondary_busy(ui: &mut Ui, icon: &str, text: &str, is_busy: bool) -> Response {
    let label = label_galley(ui, text, Kind::Secondary);
    let content = Content {
        icon: Some(icon),
        is_busy,
        is_active: false,
        label: Some(label),
        accessible: None,
    };
    paint(ui, Kind::Secondary, content, style::CONTROL_HEIGHT)
}

/// Bordered square icon button, e.g. the caret half of a split button.
pub fn secondary_square(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    let content = Content {
        icon: Some(icon),
        is_busy: false,
        is_active: false,
        label: None,
        accessible: Some(tooltip),
    };
    paint(ui, Kind::Secondary, content, style::CONTROL_HEIGHT).on_hover_text(tooltip)
}

/// Borderless command-bar button: icon plus optional text.
pub fn subtle(ui: &mut Ui, icon: &str, text: &str) -> Response {
    if text.is_empty() {
        let content = Content {
            icon: Some(icon),
            is_busy: false,
            is_active: false,
            label: None,
            accessible: None,
        };
        return paint(ui, Kind::Subtle, content, SUBTLE_HEIGHT);
    }
    let label = label_galley(ui, text, Kind::Subtle);
    let content = Content {
        icon: Some(icon),
        is_busy: false,
        is_active: false,
        label: Some(label),
        accessible: None,
    };
    paint(ui, Kind::Subtle, content, SUBTLE_HEIGHT)
}

/// Icon-only button with a tooltip naming the action (and its shortcut, if any).
pub fn icon(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    let content = Content {
        icon: Some(icon),
        is_busy: false,
        is_active: false,
        label: None,
        accessible: Some(tooltip),
    };
    paint(ui, Kind::Subtle, content, SUBTLE_HEIGHT).on_hover_text(tooltip)
}

/// Icon-only on/off button. On, it takes the selection tint and the Fill weight of `on_icon`;
/// off, the resting weight of `off_icon`. Same size in both states. Reported as a toggle button.
pub fn icon_toggle(
    ui: &mut Ui,
    off_icon: &str,
    on_icon: &str,
    is_on: bool,
    tooltip: &str,
) -> Response {
    let glyph = if is_on { on_icon } else { off_icon };
    let content = Content {
        icon: Some(glyph),
        is_busy: false,
        is_active: is_on,
        label: None,
        accessible: Some(tooltip),
    };
    let response = paint(ui, Kind::Subtle, content, SUBTLE_HEIGHT);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, response.enabled(), is_on, tooltip)
    });
    response.on_hover_text(tooltip)
}

/// A full-width entry of a popup menu: icon slot, label and an optional shortcut on the right.
/// `icon_color` tints the icon (vote colours); `None` uses the text colour.
pub fn menu_item(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    shortcut: Option<&str>,
    icon_color: Option<Color32>,
) -> Response {
    let palette = theme::palette(ui.ctx());
    let label_galley = label_galley(ui, label, Kind::Subtle);
    let shortcut_galley = shortcut.map(|text| {
        ui.painter().layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(style::SMALL_SIZE),
            palette.text_secondary,
        )
    });
    let shortcut_width =
        shortcut_galley.as_ref().map_or(0.0, |galley| style::SPACE_L + galley.size().x);
    let min_width =
        style::SPACE_S * 2.0 + icons::SIZE + ICON_GAP + label_galley.size().x + shortcut_width;
    // A popup's first frame is a sizing pass with egui's default area width (600) as the room;
    // filling it there would make every menu that wide. Only the content counts then, and the
    // entries fill the measured width from the next frame on.
    let width = if ui.is_sizing_pass() { min_width } else { ui.available_width().max(min_width) };
    let (rect, response) = ui.allocate_exact_size(vec2(width, SUBTLE_HEIGHT), Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::MenuItem, response.enabled(), label));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let is_enabled = response.enabled();
    let fill = if is_enabled {
        row::state_fill(ui, &response, Color32::TRANSPARENT)
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    }
    let icon_center = pos2(rect.left() + style::SPACE_S + icons::SIZE / 2.0, rect.center().y);
    let color = if is_enabled { icon_color.unwrap_or(palette.text) } else { palette.text_disabled };
    let font = icons::rest_font(&palette, icons::SIZE);
    super::provider_icon::paint_glyph(
        ui.painter(),
        &palette,
        icon_center,
        icon,
        font,
        color,
        is_enabled && icon_color.is_none(),
    );
    let text_left = icon_center.x + icons::SIZE / 2.0 + ICON_GAP;
    ui.painter().galley(
        pos2(text_left, rect.center().y - label_galley.size().y / 2.0),
        label_galley,
        palette.text,
    );
    if let Some(galley) = shortcut_galley {
        let pos = pos2(
            rect.right() - style::SPACE_S - galley.size().x,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(pos, galley, palette.text_secondary);
    }
    super::focus::ring(ui, &response, rect);
    response
}

/// Text link in the accent color with an underline on hover.
pub fn text_link(ui: &mut Ui, text: &str, tooltip: &str) -> Response {
    let palette = theme::palette(ui.ctx());
    let font = egui::FontId::proportional(style::BODY_SIZE);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, palette.accent_text);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, text));
    let hover = row::hover_amount(ui, &response);
    let color = motion::mix(palette.accent_text, palette.accent_hover, hover);
    ui.painter().galley(rect.min, galley, color);
    if response.hovered() {
        ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, color));
    }
    super::focus::ring(ui, &response, rect.expand(2.0));
    response.on_hover_text(tooltip)
}

/// Width of a primary or secondary button with an icon and `text`, for centring a row of them.
pub fn icon_button_width(ui: &Ui, text: &str) -> f32 {
    let label = label_galley(ui, text, Kind::Secondary);
    width(true, Some(label.size().x), style::CONTROL_HEIGHT)
}

/// Width of a [`subtle`] button with an icon and `text`, for layouts that plan their room before
/// drawing.
pub fn subtle_width(ui: &Ui, text: &str) -> f32 {
    let label = label_galley(ui, text, Kind::Subtle);
    width(true, Some(label.size().x), SUBTLE_HEIGHT)
}

/// The width an [`icon`] or [`icon_toggle`] button takes, for layouts that plan their room before
/// drawing. Every glyph sits in the same fixed slot, so this does not depend on the icon.
pub fn icon_width() -> f32 {
    width(true, None, SUBTLE_HEIGHT)
}

/// Button width from its content: the single rule `paint` and the planning helpers share.
fn width(has_icon: bool, label_width: Option<f32>, height: f32) -> f32 {
    let icon_width = if has_icon { icons::SIZE } else { 0.0 };
    let gap = if has_icon && label_width.is_some() { ICON_GAP } else { 0.0 };
    let padding = if label_width.is_none() { ICON_ONLY_PADDING } else { style::SPACE_M };
    (icon_width + gap + label_width.unwrap_or(0.0) + padding * 2.0).max(height)
}

fn label_galley(ui: &Ui, text: &str, kind: Kind) -> Arc<Galley> {
    let palette = theme::palette(ui.ctx());
    let color = match kind {
        Kind::Primary if ui.is_enabled() => palette.on_brand,
        Kind::Primary => palette.text_secondary,
        Kind::Danger => palette.on_accent,
        Kind::Secondary | Kind::Subtle if ui.is_enabled() => palette.text,
        Kind::Secondary | Kind::Subtle => palette.text_disabled,
    };
    ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(style::BODY_SIZE),
        color,
    )
}

/// How a button stands this frame, for its fill and its icon.
#[derive(Clone, Copy)]
struct Look {
    is_enabled: bool,
    is_pressed: bool,
    hover: f32,
    active: f32,
}

fn paint(ui: &mut Ui, kind: Kind, content: Content<'_>, height: f32) -> Response {
    let label_width = content.label.as_ref().map(|galley| galley.size().x);
    let width = width(content.icon.is_some(), label_width, height);
    // A busy button keeps its enabled look (the spinner says what is going on) but takes no clicks.
    let sense = if content.is_busy { Sense::hover() } else { Sense::click() };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
    let is_actionable = response.enabled() && !content.is_busy;
    describe(ui, &response, rect, &content, is_actionable);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let palette = theme::palette(ui.ctx());
    let look = Look {
        is_enabled: response.enabled(),
        is_pressed: is_actionable && response.is_pointer_button_down_on(),
        hover: if is_actionable { row::hover_amount(ui, &response) } else { 0.0 },
        active: motion::toggle(
            ui.ctx(),
            response.id.with("button_active"),
            content.is_active,
            motion::FAST,
        ),
    };
    let (fill, stroke) = surface(kind, look, &palette);
    ui.painter().rect(rect, style::RADIUS_CONTROL, fill, stroke, StrokeKind::Inside);
    paint_content(ui, kind, content, rect, look, &palette);
    response
}

/// Tells screen readers what the button does and, while it is busy, that the action runs. The
/// name is built only when egui asks for it (accessibility on), not every frame.
fn describe(ui: &Ui, response: &Response, rect: Rect, content: &Content<'_>, is_actionable: bool) {
    let name = || {
        content.accessible.map_or_else(
            || content.label.as_ref().map(|galley| galley.text().to_owned()).unwrap_or_default(),
            str::to_owned,
        )
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, is_actionable, name()));
    if content.is_busy {
        // Screen readers hear that the action runs, not only that the button is unavailable.
        let busy = ui.interact(rect, response.id.with("busy"), Sense::hover());
        busy.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::ProgressIndicator,
                true,
                format!("{} in progress", name()),
            )
        });
    }
}

/// Fill and border of a `kind` button in its current `look`.
fn surface(kind: Kind, look: Look, palette: &theme::Palette) -> (Color32, Stroke) {
    let Look { is_enabled, is_pressed, hover, active } = look;
    match kind {
        Kind::Primary if !is_enabled => (palette.pressed, Stroke::NONE),
        Kind::Primary if is_pressed => (palette.brand_pressed, Stroke::NONE),
        Kind::Primary => (motion::mix(palette.brand, palette.brand_hover, hover), Stroke::NONE),
        Kind::Danger if is_pressed => {
            (motion::mix(palette.error, palette.text, 0.25), Stroke::NONE)
        }
        Kind::Danger => (motion::mix(palette.error, palette.text, 0.12 * hover), Stroke::NONE),
        Kind::Secondary => {
            let fill = if is_pressed {
                palette.pressed
            } else {
                motion::mix(palette.surface, palette.hover, hover)
            };
            (
                fill,
                Stroke::new(1.0, if is_enabled { palette.border_strong } else { palette.border }),
            )
        }
        Kind::Subtle if is_pressed => (palette.pressed, Stroke::NONE),
        Kind::Subtle => {
            let rest = palette.hover.gamma_multiply(hover);
            (motion::mix(rest, palette.selection, active), Stroke::NONE)
        }
    }
}

/// The icon slot (or its spinner) and the label, centred on `rect`.
fn paint_content(
    ui: &Ui,
    kind: Kind,
    content: Content<'_>,
    rect: Rect,
    look: Look,
    palette: &theme::Palette,
) {
    let icon_width = if content.icon.is_some() { icons::SIZE } else { 0.0 };
    let label_width = content.label.as_ref().map_or(0.0, |galley| galley.size().x);
    let gap = if content.icon.is_some() && content.label.is_some() { ICON_GAP } else { 0.0 };
    let mut x = rect.center().x - (icon_width + gap + label_width) / 2.0;
    if let Some(glyph) = content.icon {
        let slot = Rect::from_center_size(
            pos2(x + icon_width / 2.0, rect.center().y),
            vec2(icon_width, icon_width),
        );
        let color = icon_color(kind, look.is_enabled, look.active, palette);
        if content.is_busy {
            super::states::spinner(ui, slot, color);
        } else {
            let font = icons::state_font(palette, icons::SIZE, content.is_active);
            // Platform marks keep their brand color, except on filled or disabled buttons.
            let is_branded = look.is_enabled && matches!(kind, Kind::Secondary | Kind::Subtle);
            super::provider_icon::paint_glyph(
                ui.painter(),
                palette,
                slot.center(),
                glyph,
                font,
                color,
                is_branded,
            );
        }
        x += icon_width + gap;
    }
    if let Some(galley) = content.label {
        let text_pos = pos2(x, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(text_pos, galley, palette.text);
    }
}

fn icon_color(kind: Kind, is_enabled: bool, active: f32, palette: &theme::Palette) -> Color32 {
    match kind {
        Kind::Primary if is_enabled => palette.on_brand,
        Kind::Danger => palette.on_accent,
        _ if !is_enabled => palette.text_disabled,
        _ => motion::mix(palette.text, palette.accent_text, active),
    }
}

#[cfg(test)]
mod tests {
    use cv_base::theme::{self, icons};
    use egui::{Context, RawInput};

    #[test]
    fn a_planned_subtle_width_is_the_painted_one() {
        let ctx = Context::default();
        theme::fonts::install(&ctx);
        let mut widths = Vec::new();
        let mut output = ctx.run_ui(RawInput::default(), |ui| {
            for text in ["Copy log", "First error"] {
                let painted = super::subtle(ui, icons::COPY, text).rect.width();
                widths.push((painted, super::subtle_width(ui, text)));
            }
        });
        output.textures_delta.clear();
        for (painted, planned) in widths {
            assert!((painted - planned).abs() < 0.01, "painted {painted}, planned {planned}");
        }
    }
}
