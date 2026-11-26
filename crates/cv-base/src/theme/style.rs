use egui::style::{ScrollAnimation, ScrollStyle};
use egui::{
    CornerRadius, CursorIcon, FontFamily, FontId, Margin, Rangef, Shadow, Stroke, TextStyle,
    Visuals,
};

use super::{Palette, motion};

pub const BODY_SIZE: f32 = 14.0;
pub const SMALL_SIZE: f32 = 12.0;
pub const TITLE_SIZE: f32 = 20.0;
pub const SECTION_SIZE: f32 = 16.0;

/// Spacing scale: every gap and padding is one of these.
pub const SPACE_XS: f32 = 4.0;
pub const SPACE_S: f32 = 8.0;
pub const SPACE_M: f32 = 12.0;
pub const SPACE_L: f32 = 16.0;
/// Side padding of rows inside cards and tables (pipelines, merge status).
pub const SPACE_ROW_X: f32 = 14.0;
/// Gap between a row's name and the sub line below it.
pub const SPACE_LINE: f32 = 3.0;

/// Rows with a name and a sub line: the checks table, the overview's pipelines.
pub const ROW_TWO_LINE: f32 = 56.0;
/// Single-line rows inside cards and popups: a stage card's jobs, the run switcher.
pub const ROW_COMPACT: f32 = 38.0;

/// Full width of a scroll bar while hovered or dragged; at rest it is half as wide. Bars float
/// over the content at the area's outer edge.
pub const SCROLL_BAR_WIDTH: f32 = 8.0;
/// Room padded pages keep free on the right for the floating scroll bar, whether it shows or not
/// (CSS `scrollbar-gutter: stable`): the bar never touches a card, the gap between card and bar
/// matches the left padding, and nothing moves when the content starts to overflow.
pub const SCROLL_GUTTER: f32 = SCROLL_BAR_WIDTH;

/// Buttons, inputs, chips' hover fills.
pub const RADIUS_CONTROL: u8 = 4;
/// Cards and list rows.
pub const RADIUS_CARD: u8 = 6;
/// Menus, popups and dialogs.
pub const RADIUS_POPUP: u8 = 8;
/// Height of buttons and single-line inputs.
pub const CONTROL_HEIGHT: f32 = 32.0;
/// How strongly a row with a write of the user's on its way paints its content: clearly quieter
/// than its neighbours, yet above the 0.38 to 0.5 of a disabled control, since the row still
/// takes clicks. 0.55 is our own number.
pub const PENDING_OPACITY: f32 = 0.55;

/// Folder glyphs in file trees: the warm folder tint of the Windows shell, so folders stand apart
/// from files at a glance. Both keep at least 3:1 against `surface`.
pub fn folder_icon(palette: &Palette) -> egui::Color32 {
    if palette.is_dark {
        egui::Color32::from_rgb(0xE2, 0xB3, 0x4C)
    } else {
        egui::Color32::from_rgb(0xA8, 0x76, 0x0A)
    }
}

/// Resting elevation of cards: a soft contact shadow instead of a heavy outline.
pub fn card_shadow(palette: &Palette) -> Shadow {
    Shadow { offset: [0, 1], blur: 4, spread: 0, color: palette.shadow.gamma_multiply(0.6) }
}

/// Floating layers: menus, tooltips, dialogs.
pub fn popup_shadow(palette: &Palette) -> Shadow {
    Shadow { offset: [0, 6], blur: 20, spread: 0, color: palette.shadow }
}

/// The card frame used by overview sections and thread cards.
pub fn card_frame(palette: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(RADIUS_CARD)
        .shadow(card_shadow(palette))
}

/// Applies the palette to egui's widget visuals: Fluent-like web controls, hairline borders,
/// soft elevation, short eased transitions.
pub fn apply(ctx: &egui::Context, palette: &Palette) {
    let mut visuals = if palette.is_dark { Visuals::dark() } else { Visuals::light() };
    visuals.override_text_color = Some(palette.text);
    visuals.weak_text_color = Some(palette.text_secondary);
    visuals.panel_fill = palette.ground;
    visuals.window_fill = palette.surface;
    visuals.extreme_bg_color = palette.surface;
    visuals.text_edit_bg_color = Some(palette.surface);
    visuals.faint_bg_color = palette.surface_subtle;
    visuals.code_bg_color = palette.surface_subtle;
    visuals.window_stroke = Stroke::new(1.0, palette.border);
    visuals.window_corner_radius = CornerRadius::same(RADIUS_POPUP);
    visuals.menu_corner_radius = CornerRadius::same(RADIUS_POPUP);
    visuals.hyperlink_color = palette.accent;
    visuals.selection.bg_fill = palette.accent.gamma_multiply(0.3);
    // Also the focused outline of text inputs, like a web input's focus border.
    visuals.selection.stroke = Stroke::new(2.0, palette.accent);
    visuals.text_cursor.stroke = Stroke::new(1.5, palette.text);
    visuals.warn_fg_color = palette.warning;
    visuals.error_fg_color = palette.error;
    visuals.popup_shadow = popup_shadow(palette);
    visuals.window_shadow = popup_shadow(palette);
    visuals.window_highlight_topmost = false;
    visuals.interact_cursor = Some(CursorIcon::PointingHand);
    visuals.slider_trailing_fill = true;
    visuals.indent_has_left_vline = false;
    visuals.disabled_alpha = 0.45;

    let corner = CornerRadius::same(RADIUS_CONTROL);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = palette.surface;
    widgets.noninteractive.weak_bg_fill = palette.surface;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text);
    widgets.noninteractive.corner_radius = corner;

    widgets.inactive.bg_fill = palette.surface_subtle;
    widgets.inactive.weak_bg_fill = palette.surface;
    widgets.inactive.bg_stroke = Stroke::new(1.0, palette.border_strong);
    widgets.inactive.fg_stroke = Stroke::new(1.0, palette.text);
    widgets.inactive.corner_radius = corner;
    widgets.inactive.expansion = 0.0;

    widgets.hovered.bg_fill = palette.hover;
    widgets.hovered.weak_bg_fill = palette.hover;
    widgets.hovered.bg_stroke = Stroke::new(1.0, palette.border_strong);
    widgets.hovered.fg_stroke = Stroke::new(1.0, palette.text);
    widgets.hovered.corner_radius = corner;
    widgets.hovered.expansion = 0.0;

    // Pressed, and egui's look for a focused button: kept close to hover so a click never
    // leaves a button looking stuck; keyboard focus gets its own ring (see cv_widgets::focus).
    widgets.active.bg_fill = palette.pressed;
    widgets.active.weak_bg_fill = palette.pressed;
    widgets.active.bg_stroke = Stroke::new(1.0, palette.border_strong);
    widgets.active.fg_stroke = Stroke::new(1.0, palette.text);
    widgets.active.corner_radius = corner;
    widgets.active.expansion = 0.0;

    widgets.open = widgets.hovered;

    ctx.set_visuals(visuals);
    let animation_time = motion::duration(ctx, motion::FAST);
    ctx.global_style_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::new(SMALL_SIZE, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(BODY_SIZE, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(BODY_SIZE, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(TITLE_SIZE, super::fonts::semibold())),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
        let spacing = &mut style.spacing;
        spacing.item_spacing = egui::vec2(SPACE_S, 6.0);
        spacing.button_padding = egui::vec2(SPACE_M, SPACE_XS);
        spacing.interact_size.y = CONTROL_HEIGHT;
        spacing.menu_margin = Margin::same(6);
        spacing.window_margin = Margin::same(SPACE_L as i8);
        spacing.icon_spacing = 6.0;
        spacing.tooltip_width = 360.0;
        spacing.extra_text_line_spacing = 2.0;
        spacing.scroll = scroll_style();
        style.scroll_animation = ScrollAnimation::new(2400.0, Rangef::new(0.12, 0.3));
        style.animation_time = animation_time;
        style.interaction.tooltip_delay = 0.35;
        style.interaction.tooltip_grace_time = 0.3;
    });
}

/// Thin overlay scroll bars that fade in while the pointer is over the area and widen on hover.
fn scroll_style() -> ScrollStyle {
    let mut scroll = ScrollStyle::floating();
    scroll.bar_width = SCROLL_BAR_WIDTH;
    scroll.floating_width = SCROLL_BAR_WIDTH / 2.0;
    scroll.handle_min_length = 32.0;
    scroll.bar_inner_margin = 2.0;
    scroll.active_background_opacity = 0.0;
    scroll.interact_background_opacity = 0.25;
    scroll.active_handle_opacity = 0.45;
    scroll.interact_handle_opacity = 0.8;
    // Browsers don't fade content at the scroll edges.
    scroll.fade.strength = 0.0;
    scroll
}
