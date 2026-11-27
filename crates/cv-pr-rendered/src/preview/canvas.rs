//! Draws a preview's images: one column per side, or both laid over each other (Swipe, Onion
//! skin), over a checkerboard. Animations play from when their textures arrived and repaint only
//! when the next frame is due; under reduced motion they stay on their first frame. A side's
//! context menu copies it to the clipboard at its full resolution.

use egui::{Response, RichText, Sense, TextureId, Ui, WidgetInfo, vec2};

use cv_base::theme::{self, icons, motion};
use cv_media::image_clipboard;
use cv_media::picture::Picture;
use cv_widgets::buttons;

/// The uploaded sides of a preview, and when they arrived on egui's clock.
#[derive(Default)]
pub struct Textures {
    pub old: Option<Picture>,
    pub new: Option<Picture>,
    pub since: f64,
}

/// The texture of `picture` to draw now; asks for a repaint when its next frame is due.
fn frame(ui: &Ui, picture: &Picture, since: f64) -> TextureId {
    let ctx = ui.ctx();
    let elapsed = (!motion::is_reduced(ctx)).then(|| ctx.input(|input| input.time) - since);
    let (texture, next) = picture.frame_at(elapsed);
    if let Some(next) = next {
        ctx.request_repaint_after(next);
    }
    texture
}

pub fn image_column(ui: &mut Ui, title: &str, picture: Option<&Picture>, since: f64) {
    let palette = theme::palette(ui.ctx());
    ui.label(RichText::new(title).font(theme::fonts::semibold_font(theme::style::BODY_SIZE)));
    ui.add_space(theme::style::SPACE_XS);
    let Some(picture) = picture else {
        ui.label(RichText::new("No file on this side.").color(palette.text_secondary));
        return;
    };
    let natural = picture.size_vec2();
    ui.label(
        RichText::new(format!("{} × {}", natural.x, natural.y))
            .size(theme::style::SMALL_SIZE)
            .color(palette.text_secondary),
    );
    let size = fit(natural, ui.available_width());
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Image, true, format!("{title} image")));
    copy_menu(&response, &[("Copy image", picture)]);
    paint_checkerboard(ui, rect);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter().image(frame(ui, picture, since), rect, uv, egui::Color32::WHITE);
}

/// The context menu of a shown side: "Copy image" for each picture that has its file.
fn copy_menu(response: &Response, entries: &[(&str, &Picture)]) {
    response.context_menu(|ui| {
        for (label, picture) in entries {
            if let Some(original) = picture.original()
                && buttons::menu_item(ui, icons::COPY, label, None, None).clicked()
            {
                image_clipboard::request(ui.ctx(), original.clone(), None);
                ui.close();
            }
        }
    });
}

pub fn paint_overlay(ui: &mut Ui, [old, new]: [&Picture; 2], since: f64, mix: f32, is_swipe: bool) {
    let natural = old.size_vec2().max(new.size_vec2());
    let size = fit(natural, ui.available_width());
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    copy_menu(&response, &[("Copy Before image", old), ("Copy After image", new)]);
    paint_checkerboard(ui, rect);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let painter = ui.painter_at(rect);
    let (old, new) = (frame(ui, old, since), frame(ui, new, since));
    painter.image(old, rect, uv, egui::Color32::WHITE);
    if is_swipe {
        let split = rect.left() + rect.width() * mix;
        let shown = egui::Rect::from_min_max(rect.min, egui::pos2(split, rect.bottom()));
        let shown_uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(mix, 1.0));
        painter.image(new, shown, shown_uv, egui::Color32::WHITE);
        let palette = theme::palette(ui.ctx());
        painter.vline(split, rect.y_range(), egui::Stroke::new(2.0, palette.accent));
    } else {
        painter.image(new, rect, uv, egui::Color32::from_white_alpha((mix * 255.0) as u8));
    }
}

/// Transparent pixels show over a checkerboard, as in every image viewer.
fn paint_checkerboard(ui: &Ui, rect: egui::Rect) {
    const CELL: f32 = 8.0;
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0, palette.surface);
    let columns = (rect.width() / CELL).ceil() as usize;
    let rows = (rect.height() / CELL).ceil() as usize;
    for row in 0..rows {
        for column in (row % 2..columns).step_by(2) {
            let min = rect.min + vec2(column as f32 * CELL, row as f32 * CELL);
            painter.rect_filled(
                egui::Rect::from_min_size(min, vec2(CELL, CELL)),
                0,
                palette.surface_subtle,
            );
        }
    }
}

fn fit(natural: egui::Vec2, max_width: f32) -> egui::Vec2 {
    let scale = (max_width / natural.x.max(1.0)).min(1.0);
    natural * scale
}
