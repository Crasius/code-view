//! Minimize, maximize/restore and close, drawn like Windows 11's: 46 px wide, the full height of
//! the title bar, Segoe Fluent Icons glyphs (Segoe MDL2 Assets on Windows 10, drawn shapes without
//! either), a subtle wash on hover, the red close button, dimmed glyphs while the window is inactive.
//!
//! The pointer never reaches egui over these buttons: they answer `HTMINBUTTON` / `HTMAXBUTTON` /
//! `HTCLOSE` (the maximize code opens Snap Layouts), so the hover and press state comes from the
//! window procedure ([`super::caption::pointer`]). Screen readers still get three buttons whose
//! default action runs the same command.

use std::sync::atomic::{AtomicBool, Ordering};

use egui::{
    Color32, FontDefinitions, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Ui, ViewportCommand,
    pos2, vec2,
};

use super::caption;
use super::hit_test::CaptionButton;
use crate::theme::{self, Palette, fonts, motion};

const BUTTON_WIDTH: f32 = 46.0;
/// Glyph size of the system's caption buttons.
const GLYPH_SIZE: f32 = 10.0;
/// Segoe Fluent Icons (Windows 11), else Segoe MDL2 Assets (Windows 10): the caption button glyphs.
const GLYPH_FONT_FILES: [&str; 2] = ["SegoeIcons.ttf", "segmdl2.ttf"];
const GLYPH_FAMILY: &str = "caption-glyphs";
static HAS_GLYPH_FONT: AtomicBool = AtomicBool::new(false);
const CLOSE_RED: Color32 = Color32::from_rgb(0xC4, 0x2B, 0x1C);
/// Windows 11 shows the button's name after this long a hover.
const TOOLTIP_DELAY: f64 = 0.5;

/// Which caption button the pointer is over and which one it holds down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ButtonPointer {
    pub hovered: Option<CaptionButton>,
    pub pressed: Option<CaptionButton>,
}

impl ButtonPointer {
    pub(crate) const NONE: ButtonPointer = ButtonPointer { hovered: None, pressed: None };
}

/// Adds the system's caption glyph font as its own family, when one is installed.
pub(crate) fn register_glyph_font(fonts: &mut FontDefinitions) {
    let faces = GLYPH_FONT_FILES.map(super::system_font);
    if let Some(name) = fonts::load_first(fonts, GLYPH_FAMILY, &faces) {
        fonts.families.insert(FontFamily::Name(GLYPH_FAMILY.into()), vec![name]);
        HAS_GLYPH_FONT.store(true, Ordering::Relaxed);
    }
}

/// The caption glyph font at `size`; `None` when neither font is installed.
fn glyph_font(size: f32) -> Option<FontId> {
    HAS_GLYPH_FONT
        .load(Ordering::Relaxed)
        .then(|| FontId::new(size, FontFamily::Name(GLYPH_FAMILY.into())))
}

/// Paints the three buttons at the right end of `bar`; returns their rects (minimize, maximize,
/// close) for the hit test.
pub(crate) fn show(ui: &mut Ui, bar: Rect) -> [Rect; 3] {
    let ctx = ui.ctx().clone();
    let (is_maximized, is_active) = ctx.input(|input| {
        let viewport = input.viewport();
        (viewport.maximized.unwrap_or(false), viewport.focused.unwrap_or(true))
    });
    let pointer = caption::pointer();
    let hover_started = hover_started(&ctx, pointer.hovered);
    let rects = CaptionButton::ALL.map(|button| {
        let from_right = match button {
            CaptionButton::Minimize => 3.0,
            CaptionButton::Maximize => 2.0,
            CaptionButton::Close => 1.0,
        };
        Rect::from_min_max(
            pos2(bar.right() - BUTTON_WIDTH * from_right, bar.top()),
            pos2(bar.right() - BUTTON_WIDTH * (from_right - 1.0), bar.bottom()),
        )
    });
    for (button, rect) in CaptionButton::ALL.into_iter().zip(rects) {
        let label = match button {
            CaptionButton::Minimize => "Minimize",
            CaptionButton::Maximize if is_maximized => "Restore",
            CaptionButton::Maximize => "Maximize",
            CaptionButton::Close => "Close",
        };
        // Click without focus: native caption buttons are not in the Tab order (Alt+Space is).
        let response =
            ui.interact(rect, ui.scope_id().with(("caption_button", label)), Sense::CLICK);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        let is_hovered = pointer.hovered == Some(button);
        let hover = motion::toggle(&ctx, response.id.with("hover"), is_hovered, motion::FAST);
        let is_pressed = pointer.pressed == Some(button);
        paint(ui, rect, button, is_maximized, State { hover, is_pressed, is_active });
        // Maximize shows Snap Layouts on hover instead of a tooltip.
        if is_hovered
            && button != CaptionButton::Maximize
            && let Some(started) = hover_started
        {
            let waited = ctx.input(|input| input.time) - started;
            if waited >= TOOLTIP_DELAY {
                response.show_tooltip_text(label);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                    TOOLTIP_DELAY - waited,
                ));
            }
        }
        if response.clicked() {
            ctx.send_viewport_cmd(match button {
                CaptionButton::Minimize => ViewportCommand::Minimized(true),
                CaptionButton::Maximize => ViewportCommand::Maximized(!is_maximized),
                CaptionButton::Close => ViewportCommand::Close,
            });
        }
    }
    rects
}

#[derive(Clone, Copy)]
struct State {
    /// 0..1, eased.
    hover: f32,
    is_pressed: bool,
    is_active: bool,
}

fn paint(ui: &Ui, rect: Rect, button: CaptionButton, is_maximized: bool, state: State) {
    let palette = theme::palette(ui.ctx());
    let (fill, glyph) = colors(&palette, button, state);
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 0, fill);
    }
    let glyph_char = match button {
        CaptionButton::Minimize => '\u{E921}',
        CaptionButton::Maximize if is_maximized => '\u{E923}',
        CaptionButton::Maximize => '\u{E922}',
        CaptionButton::Close => '\u{E8BB}',
    };
    let center = snap(ui, rect.center());
    match glyph_font(GLYPH_SIZE) {
        Some(font) => {
            // Center the glyph's ink, not its line box: the icon font's ascent is not symmetric.
            let galley = ui.painter().layout_no_wrap(glyph_char.to_string(), font, glyph);
            let origin = center - galley.mesh_bounds.center().to_vec2();
            ui.painter().galley(snap(ui, origin), galley, glyph);
        }
        None => draw_glyph(ui, center, button, is_maximized, glyph),
    }
}

/// Background and glyph color: Windows 11's subtle fills (hover, then the lighter pressed one with
/// a dimmed glyph), solid red under a white glyph for close.
fn colors(palette: &Palette, button: CaptionButton, state: State) -> (Color32, Color32) {
    let rest_glyph = if state.is_active { palette.text } else { palette.text_disabled };
    if button == CaptionButton::Close {
        if state.is_pressed {
            return (CLOSE_RED.gamma_multiply(0.9), Color32::WHITE.gamma_multiply(0.7));
        }
        return (
            CLOSE_RED.gamma_multiply(state.hover),
            motion::mix(rest_glyph, Color32::WHITE, state.hover),
        );
    }
    let (hover_fill, pressed_fill) = if palette.is_dark {
        (Color32::from_white_alpha(15), Color32::from_white_alpha(11))
    } else {
        (Color32::from_black_alpha(10), Color32::from_black_alpha(6))
    };
    if state.is_pressed {
        return (pressed_fill, palette.text_secondary);
    }
    (hover_fill.gamma_multiply(state.hover), motion::mix(rest_glyph, palette.text, state.hover))
}

/// The glyphs as shapes, for systems without either icon font.
fn draw_glyph(ui: &Ui, center: Pos2, button: CaptionButton, is_maximized: bool, color: Color32) {
    let pixel = 1.0 / ui.ctx().pixels_per_point();
    let stroke = Stroke::new(pixel, color);
    let half = GLYPH_SIZE / 2.0;
    let painter = ui.painter();
    match button {
        CaptionButton::Minimize => {
            painter.hline((center.x - half)..=(center.x + half), center.y, stroke);
        }
        CaptionButton::Maximize if is_maximized => {
            // The front window, and the edges of the one behind it that stick out top and right.
            let front = Rect::from_min_size(
                pos2(center.x - half, center.y - half + 2.0),
                vec2(GLYPH_SIZE - 2.0, GLYPH_SIZE - 2.0),
            );
            painter.rect_stroke(front, 1, stroke, egui::StrokeKind::Inside);
            let back = [
                pos2(front.left() + 2.0, front.top()),
                pos2(front.left() + 2.0, center.y - half),
                pos2(center.x + half, center.y - half),
                pos2(center.x + half, front.bottom() - 2.0),
                pos2(front.right(), front.bottom() - 2.0),
            ];
            painter.line(back.to_vec(), stroke);
        }
        CaptionButton::Maximize => {
            painter.rect_stroke(
                Rect::from_center_size(center, vec2(GLYPH_SIZE, GLYPH_SIZE)),
                1,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        CaptionButton::Close => {
            painter.line_segment([center + vec2(-half, -half), center + vec2(half, half)], stroke);
            painter.line_segment([center + vec2(-half, half), center + vec2(half, -half)], stroke);
        }
    }
}

/// Rounds a point to the physical pixel grid, so one-pixel glyph strokes stay sharp.
fn snap(ui: &Ui, point: Pos2) -> Pos2 {
    let pixels_per_point = ui.ctx().pixels_per_point();
    pos2(
        (point.x * pixels_per_point).round() / pixels_per_point,
        (point.y * pixels_per_point).round() / pixels_per_point,
    )
}

/// Input time the pointer came to rest on the current button, for the tooltip delay.
fn hover_started(ctx: &egui::Context, hovered: Option<CaptionButton>) -> Option<f64> {
    let id = egui::Id::unique("caption_button_hover_started");
    let now = ctx.input(|input| input.time);
    ctx.data_mut(|data| {
        let entry = data.get_temp_mut_or_insert_with(id, || (hovered, now));
        if entry.0 != hovered {
            *entry = (hovered, now);
        }
        hovered.map(|_| entry.1)
    })
}
