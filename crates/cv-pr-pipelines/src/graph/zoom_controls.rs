//! The zoom control floating in the canvas' bottom-right corner: "−  100 %  +" and "Fit". It
//! overlays the canvas instead of taking room from it, and the level sits in a slot sized for
//! the widest level, so neither the canvas nor the control moves while the zoom changes.

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, style};
use cv_widgets::{buttons, focus, row};
use egui::{
    Align2, Color32, Context, FontId, Frame, Id, Margin, Order, Rect, Sense, Stroke, Ui, vec2,
};

use super::pan_canvas::{Command, MAX_ZOOM, MIN_ZOOM};

/// Room between the control and the canvas' corner, clear of the position indicators.
const CORNER_GAP: f32 = style::SPACE_M;
/// The widest level the slot must hold.
const WIDEST_LEVEL: &str = "200 %";

/// Shows the control for `zoom` over the visible part of the canvas `canvas`; returns what was
/// clicked.
pub fn show(ctx: &Context, id: Id, canvas: Rect, zoom: f32) -> Option<Command> {
    if !canvas.is_positive() {
        return None;
    }
    let palette = theme::palette(ctx);
    let mut command = None;
    egui::Area::new(id.with("zoom_controls"))
        .order(Order::Middle)
        .pivot(Align2::RIGHT_BOTTOM)
        .fixed_pos(canvas.right_bottom() - vec2(CORNER_GAP, CORNER_GAP))
        .constrain_to(canvas)
        .movable(false)
        .show(ctx, |ui| {
            Frame::new()
                .fill(palette.surface)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(style::RADIUS_CARD)
                .inner_margin(Margin::same(2))
                .shadow(style::card_shadow(&palette))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.horizontal(|ui| {
                        let out = ui.add_enabled_ui(zoom > MIN_ZOOM, |ui| {
                            buttons::icon(
                                ui,
                                icons::MINUS,
                                concat!("Zoom out (", shortcut!(command "-"), ")"),
                            )
                        });
                        if out.inner.clicked() {
                            command = Some(Command::Step(-1));
                        }
                        if level(ui, zoom).clicked() {
                            command = Some(Command::Reset);
                        }
                        let zoom_in = ui.add_enabled_ui(zoom < MAX_ZOOM, |ui| {
                            buttons::icon(
                                ui,
                                icons::PLUS,
                                concat!("Zoom in (", shortcut!(command "="), ")"),
                            )
                        });
                        if zoom_in.inner.clicked() {
                            command = Some(Command::Step(1));
                        }
                        let (divider, _) = ui.allocate_exact_size(
                            vec2(1.0, buttons::SUBTLE_HEIGHT - style::SPACE_S * 2.0),
                            Sense::hover(),
                        );
                        ui.painter().rect_filled(divider, 0, palette.border);
                        if buttons::subtle(ui, icons::CORNERS_OUT, "Fit")
                            .on_hover_text("Fit the whole graph in view")
                            .clicked()
                        {
                            command = Some(Command::Fit);
                        }
                    });
                });
        });
    command
}

/// The level, as a button that returns to 100 %.
fn level(ui: &mut Ui, zoom: f32) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let font = FontId::proportional(style::BODY_SIZE);
    let slot =
        ui.painter().layout_no_wrap(WIDEST_LEVEL.to_owned(), font.clone(), palette.text).size().x
            + style::SPACE_S * 2.0;
    let (rect, response) =
        ui.allocate_exact_size(vec2(slot, buttons::SUBTLE_HEIGHT), Sense::click());
    let text = format!("{:.0} %", zoom * 100.0);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Zoom {text}, reset to 100 %"))
    });
    let fill = row::state_fill(ui, &response, Color32::TRANSPARENT);
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font, palette.text);
    focus::ring(ui, &response, rect);
    response.on_hover_text(concat!("Reset to 100 % (", shortcut!(command "0"), ")"))
}
