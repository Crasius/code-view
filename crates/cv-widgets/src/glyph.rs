//! Icon glyphs in a layout, as screen readers should meet them. A Phosphor glyph is a private-use
//! character that a screen reader speaks as nothing or as a code point, so a glyph never becomes
//! a widget's name (the app's test builds check every frame):
//!
//! * [`decorative`]: an icon the text around it already says (a status icon before its message)
//!   takes its place in the layout and stays out of the accessibility tree.
//! * [`named`]: an icon that carries meaning of its own (the arrow between two branches) is read
//!   as the word it stands for.
//! * [`speak_as`] and [`button`]: a label or button whose painted text starts with an icon is
//!   read as its words; [`hint_as`] does the same for a text field's hint.
//! * [`truncated`]: a text cut to its room keeps its icon out of the label, since egui repeats a
//!   cut label's whole text in its tooltip, which no [`speak_as`] reaches.

use egui::accesskit::Role;
use egui::{Label, Response, RichText, Sense, TextStyle, TextWrapMode, Ui, WidgetText};

/// Between an icon and the text it leads, about the two spaces of an inline `"{icon}  text"`.
const GAP: f32 = 6.0;

/// Paints `icon` where a label would sit, without a name for screen readers. The response still
/// takes hover, for a tooltip.
pub fn decorative(ui: &mut Ui, icon: impl Into<WidgetText>) -> Response {
    let galley =
        icon.into().into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Body);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.min, galley, ui.visuals().text_color());
    }
    response
}

/// Paints `icon` where a label would sit; screen readers read it as `name`.
pub fn named(ui: &mut Ui, icon: impl Into<WidgetText>, name: &str) -> Response {
    let response = decorative(ui, icon);
    response.widget_info(|| egui::WidgetInfo::labeled(Role::Label, true, name));
    response
}

/// An egui button (a context menu's entry) that shows `icon` before `text`; screen readers read
/// `text`.
pub fn button(ui: &mut Ui, icon: &str, text: &str) -> Response {
    let response = ui.button(format!("{icon}  {text}"));
    speak_as(&response, text);
    response
}

/// Makes screen readers read the hint of the text field of `response` as `words`, for a hint that
/// starts with an icon.
pub fn hint_as(response: &Response, words: &str) {
    response.ctx.accesskit_node_builder(response.id, |node| node.set_placeholder(words));
}

/// `icon` (styled like `text`) and then `text` on one line, the text cut with an ellipsis to the
/// room left and whole in its tooltip; screen readers read the text alone. Returns the text's
/// response.
pub fn truncated(ui: &mut Ui, icon: RichText, text: RichText) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        decorative(ui, icon);
        ui.add(Label::new(text).truncate())
    })
    .inner
}

/// Makes screen readers read the widget of `response` as `name`, for a label or a button whose
/// painted text carries an icon glyph.
pub fn speak_as(response: &Response, name: &str) {
    response.ctx.accesskit_node_builder(response.id, |node| {
        // egui keeps a label's text in its value, every other widget's name in its label.
        if node.role() == Role::Label {
            node.set_value(name);
        } else {
            node.set_label(name);
        }
    });
}
