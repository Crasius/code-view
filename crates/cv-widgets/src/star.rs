//! The favorite star of a repository: an outlined star at rest, a filled amber one once the
//! repository is a favorite. The fill and the weight say it, not the color alone, and the button
//! reports itself as a pressed or released toggle named "Favorite {repository}".

use cv_base::theme::{self, Palette, icons, motion, style};
use egui::{Align2, Id, Painter, Pos2, Rect, Response, Sense, Ui, vec2};

use super::{buttons, focus, row};

const ADD: &str = "Add to favorites";
const REMOVE: &str = "Remove from favorites";

/// The tooltip for the star's next click.
fn action_label(is_on: bool) -> &'static str {
    if is_on { REMOVE } else { ADD }
}

/// The accessible name of a repository's star toggle.
fn name(repository_key: &str) -> String {
    format!("Favorite {repository_key}")
}

/// The star glyph centred on `center`; `emphasis` (0..1) darkens a resting star while its row or
/// button is hovered.
pub fn paint(
    painter: &Painter,
    palette: &Palette,
    center: Pos2,
    size: f32,
    is_on: bool,
    emphasis: f32,
) {
    let color = if is_on {
        palette.favorite
    } else {
        motion::mix(palette.text_secondary, palette.text, emphasis)
    };
    painter.text(
        center,
        Align2::CENTER_CENTER,
        icons::STAR,
        icons::state_font(palette, size, is_on),
        color,
    );
}

/// A square star button on `rect`, interacted on top of whatever was painted there (a clickable
/// row), so its click never reaches what is below. Clicked (pointer, Enter or Space) means toggle.
pub fn button_at(ui: &mut Ui, rect: Rect, id: Id, is_on: bool, repository_key: &str) -> Response {
    let palette = theme::palette(ui.ctx());
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, is_on, name(repository_key))
    });
    let hover = row::hover_amount(ui, &response);
    let fill = if response.is_pointer_button_down_on() {
        palette.pressed
    } else {
        palette.hover.gamma_multiply(hover)
    };
    ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    paint(ui.painter(), &palette, rect.center(), icons::SIZE, is_on, hover);
    focus::ring(ui, &response, rect);
    response.on_hover_text(action_label(is_on))
}

/// [`button_at`] laid out like an icon button of the command bar.
pub fn toggle(ui: &mut Ui, is_on: bool, repository_key: &str) -> Response {
    let size = buttons::icon_width();
    let (rect, _) = ui.allocate_exact_size(vec2(size, buttons::SUBTLE_HEIGHT), Sense::hover());
    let id = ui.scope_id().with(("favorite_star", repository_key));
    button_at(ui, rect, id, is_on, repository_key)
}
