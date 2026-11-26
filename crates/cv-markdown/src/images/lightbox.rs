//! The lightbox: an image shown large over everything until any click, the close button or Esc.
//! Its corner offers "Copy image" (also Ctrl+C, Cmd+C on macOS), and for an image with an address
//! "Open in browser" and "Copy link".

use cv_base::platform::shortcut;
use cv_base::theme::{self, Palette, icons, motion, style as tokens};
use cv_media::image_clipboard;
use cv_media::picture::Picture;
use cv_widgets::buttons;
use egui::{
    Align, Align2, Color32, Context, FontId, Id, Key, Layout, Modifiers, Rect, Sense, Ui,
    UiBuilder, Vec2, WidgetInfo, pos2,
};

const LIGHTBOX_ID: &str = "review.markdown.lightbox";
const MARGIN: f32 = 48.0;

/// The image shown large over everything, while open.
#[derive(Clone)]
struct Lightbox {
    picture: Picture,
    /// Natural size in points.
    size: Vec2,
    title: String,
    web_url: Option<String>,
    /// Input time it opened at: an animation plays from its start.
    opened: f64,
}

impl Lightbox {
    /// Puts the image on the clipboard; a picture drawn here (a diagram) has no body to copy.
    fn copy(&self, ctx: &Context) {
        if let Some(original) = self.picture.original() {
            image_clipboard::request(ctx, original.clone(), self.web_url.clone());
        }
    }
}

/// Shows `picture` in the lightbox, at most `size` points large.
pub(crate) fn open_lightbox(
    ctx: &Context,
    picture: Picture,
    size: Vec2,
    title: String,
    web_url: Option<String>,
) {
    let opened = ctx.input(|input| input.time);
    let lightbox = Lightbox { picture, size, title, web_url, opened };
    ctx.data_mut(|data| data.insert_temp(Id::unique(LIGHTBOX_ID), lightbox));
}

/// Draws the lightbox over the window when an image opened it: the image as large as the window
/// allows (never past one image pixel per screen pixel) and the buttons in the corner. Any click
/// elsewhere, the close button or Esc closes it. Called once per frame.
pub fn show_lightbox(ctx: &Context) {
    let id = Id::unique(LIGHTBOX_ID);
    let Some(lightbox) = ctx.data(|data| data.get_temp::<Lightbox>(id)) else {
        return;
    };
    let palette = theme::palette(ctx);
    let mut close = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    // Ctrl+C (Cmd+C) arrives as a copy event, not as a key.
    if ctx.input(|input| input.events.iter().any(|event| matches!(event, egui::Event::Copy))) {
        lightbox.copy(ctx);
    }
    let screen = ctx.content_rect();
    egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(screen.min).show(
        ctx,
        |ui| {
            let (rect, backdrop) = ui.allocate_exact_size(screen.size(), Sense::click());
            backdrop
                .widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, "Close the image"));
            let shown = motion::toggle(ctx, id.with("shown"), true, motion::FAST);
            ui.painter().rect_filled(rect, 0, Color32::from_black_alpha((210.0 * shown) as u8));
            let room = rect.shrink(MARGIN);
            let fit = (room.width() / lightbox.size.x.max(1.0))
                .min(room.height() / lightbox.size.y.max(1.0))
                .min(1.0);
            let image = Rect::from_center_size(room.center(), lightbox.size * fit);
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            let elapsed =
                (!motion::is_reduced(ctx)).then(|| ctx.input(|input| input.time) - lightbox.opened);
            let (texture, next_frame) = lightbox.picture.frame_at(elapsed);
            ui.painter().image(texture, image, uv, Color32::WHITE.gamma_multiply(shown));
            if let Some(next_frame) = next_frame {
                ctx.request_repaint_after(next_frame);
            }
            if !lightbox.title.trim().is_empty() {
                let font = FontId::proportional(tokens::BODY_SIZE);
                ui.painter().text(
                    pos2(room.center().x, image.bottom() + tokens::SPACE_S),
                    Align2::CENTER_TOP,
                    lightbox.title.trim(),
                    font,
                    Color32::from_gray(230),
                );
            }
            close |= corner_buttons(ui, rect, &lightbox, &palette);
            close |= backdrop.clicked();
        },
    );
    if close {
        ctx.data_mut(|data| data.remove::<Lightbox>(id));
        ctx.request_repaint();
    }
}

/// Copy image, Open in browser, Copy link and Close in the top-right corner of `rect`; true when
/// Close was clicked.
fn corner_buttons(ui: &mut Ui, rect: Rect, lightbox: &Lightbox, palette: &Palette) -> bool {
    let bar = Rect::from_min_max(
        pos2(rect.left(), rect.top() + tokens::SPACE_S),
        pos2(rect.right() - tokens::SPACE_L, rect.top() + tokens::SPACE_S + tokens::CONTROL_HEIGHT),
    );
    let mut buttons_ui =
        ui.new_child(UiBuilder::new().max_rect(bar).layout(Layout::right_to_left(Align::Center)));
    buttons_ui.visuals_mut().override_text_color = Some(palette.text);
    let mut close = false;
    egui::Frame::new()
        .fill(palette.surface)
        .corner_radius(tokens::RADIUS_CONTROL)
        .inner_margin(egui::Margin::same(2))
        .show(&mut buttons_ui, |ui| {
            ui.horizontal(|ui| {
                if lightbox.picture.original().is_some()
                    && buttons::subtle(ui, icons::COPY, "Copy image")
                        .on_hover_text(shortcut!(command "C"))
                        .clicked()
                {
                    lightbox.copy(ui.ctx());
                }
                if let Some(url) = &lightbox.web_url {
                    if buttons::subtle(ui, icons::ARROW_SQUARE_OUT, "Open in browser").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                    if buttons::subtle(ui, icons::LINK, "Copy link").clicked() {
                        ui.ctx().copy_text(url.clone());
                    }
                }
                close = buttons::subtle(ui, icons::X, "Close (Esc)").clicked();
            });
        });
    close
}
