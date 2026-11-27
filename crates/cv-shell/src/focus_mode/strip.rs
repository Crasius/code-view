//! The one bar focus mode keeps: 28 points at the top of the window, the view's critical controls
//! in compact 26 × 24 buttons, the window's own parts where the app draws the title bar (caption
//! buttons at the right end on Windows, room for the traffic lights at the left end on macOS), and
//! everything between the controls a drag area that moves the window as the title bar does
//! ([`platform::Caption`] serves both).
//!
//! Controls are laid out from the right first, so they keep their places; the view's title takes
//! what is left in the middle and shortens to fit. Every button names its action and shortcut in
//! its tooltip and for screen readers.

use egui::{
    Align, Align2, Color32, FontId, Layout, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2,
    vec2,
};

use super::{EXIT_HINT, request_toggle};
use cv_base::platform;
use cv_base::theme::{self, icons, style};
use cv_widgets::{focus, row};

/// Height of the strip, its hairline included.
pub const HEIGHT: f32 = 28.0;
/// The height of the strip's controls; a view's own field in the strip takes it too.
pub const CONTROL_HEIGHT: f32 = 24.0;
/// A compact control: as wide as a 16 px glyph with 5 points either side, 24 points tall.
const CONTROL: egui::Vec2 = vec2(26.0, CONTROL_HEIGHT);
/// Between the strip's edges and its outermost controls, and between groups.
const PADDING: f32 = 6.0;
/// The accessible name of the strip; tests find it by this.
pub const LABEL: &str = "Focus mode bar";
/// Widgets the drag area leaves out; more groups than this stay draggable.
const MAX_HOLES: usize = 6;

/// Lays out the strip's content: [`Self::right`] groups from the right end, then [`Self::left`]
/// groups from the left, then [`Self::middle`] in what remains.
pub struct Strip<'a> {
    ui: &'a mut Ui,
    bar: Rect,
    left: f32,
    right: f32,
    holes: [Rect; MAX_HOLES],
    hole_count: usize,
}

impl Strip<'_> {
    /// A group of controls, right to left, left of the groups added before.
    pub fn right<R>(&mut self, add: impl FnOnce(&mut Ui) -> R) -> R {
        let area = Rect::from_min_max(
            pos2(self.left, self.bar.top()),
            pos2(self.right, self.bar.bottom()),
        );
        let (inner, used) = self.group(area, Layout::right_to_left(Align::Center), add);
        if used.is_positive() {
            self.right = used.left() - PADDING;
        }
        inner
    }

    /// A group of controls, left to right, right of the left groups added before.
    pub fn left<R>(&mut self, add: impl FnOnce(&mut Ui) -> R) -> R {
        let area = Rect::from_min_max(
            pos2(self.left, self.bar.top()),
            pos2(self.right, self.bar.bottom()),
        );
        let (inner, used) = self.group(area, Layout::left_to_right(Align::Center), add);
        if used.is_positive() {
            self.left = used.right() + PADDING;
        }
        inner
    }

    /// Text between the groups (a title, a path). It is no widget: dragging over it moves the
    /// window, as the title of a title bar does.
    pub fn middle<R>(&mut self, add: impl FnOnce(&mut Ui) -> R) -> R {
        let area = Rect::from_min_max(
            pos2(self.left, self.bar.top()),
            pos2(self.right.max(self.left), self.bar.bottom()),
        );
        let mut child = self.ui.new_child(
            UiBuilder::new()
                .id_salt("focus_strip_middle")
                .max_rect(area)
                .layout(Layout::left_to_right(Align::Center)),
        );
        child.set_clip_rect(area.intersect(self.ui.clip_rect()));
        add(&mut child)
    }

    /// The room left for [`Self::middle`].
    pub fn middle_width(&self) -> f32 {
        (self.right - self.left).max(0.0)
    }

    fn group<R>(
        &mut self,
        area: Rect,
        layout: Layout,
        add: impl FnOnce(&mut Ui) -> R,
    ) -> (R, Rect) {
        let salt = ("focus_strip_group", self.hole_count);
        let mut child =
            self.ui.new_child(UiBuilder::new().id_salt(salt).max_rect(area).layout(layout));
        child.spacing_mut().item_spacing.x = 2.0;
        let inner = add(&mut child);
        let used = child.min_rect();
        if used.is_positive() && self.hole_count < MAX_HOLES {
            self.holes[self.hole_count] = used;
            self.hole_count += 1;
        }
        (inner, used)
    }
}

/// The strip at the top of `ui`'s free space (the top of the window while the chrome is hidden),
/// with `content` laid out in it. `id` keeps the strips of different views apart.
pub fn show<R>(ui: &mut Ui, id: &str, content: impl FnOnce(&mut Strip<'_>) -> R) -> R {
    let palette = theme::palette(ui.ctx());
    egui::Panel::top(id)
        .exact_size(HEIGHT)
        .resizable(false)
        .frame(egui::Frame::new().fill(palette.surface))
        .show(ui, |ui| {
            let bar = ui.max_rect();
            // First, so the controls on top of it take the pointer.
            let response = ui.interact(bar, ui.scope_id().with("focus_strip"), Sense::hover());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Toolbar, true, LABEL));
            ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, palette.border));
            // Where the app draws the title bar, the strip takes the window's own parts and caption.
            let caption =
                super::is_integrated(ui.ctx()).then(|| platform::Caption::show(ui, bar, false));
            let free = caption.as_ref().map_or(bar.x_range(), platform::Caption::free);
            let mut strip = Strip {
                ui,
                bar,
                left: free.min + PADDING,
                right: free.max - PADDING,
                holes: [Rect::NOTHING; MAX_HOLES],
                hole_count: 0,
            };
            let inner = content(&mut strip);
            if let Some(caption) = caption {
                caption.publish(strip.ui.ctx(), &strip.holes[..strip.hole_count]);
            }
            inner
        })
        .inner
}

/// A compact icon button; `tooltip` names the action and its shortcut.
pub fn button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(CONTROL, Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, response.enabled(), tooltip));
    paint(ui, &response, rect, icon, false);
    response.on_hover_text(tooltip)
}

/// A compact on/off button: the selection tint and the icon's Fill weight while on.
pub fn toggle(ui: &mut Ui, icon: &str, is_on: bool, role: egui::Role, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(CONTROL, Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(role, response.enabled(), is_on, tooltip));
    paint(ui, &response, rect, icon, is_on);
    response.on_hover_text(tooltip)
}

/// A compact toggle with a short text after its icon (a count).
pub fn labeled_toggle(
    ui: &mut Ui,
    icon: &str,
    text: &str,
    is_on: bool,
    name: &str,
    tooltip: &str,
) -> Response {
    labeled(ui, icon, text, Some(is_on), name, tooltip)
}

/// A compact button with its action's name after the icon ("Back to latest"); the name is its
/// accessible name too.
pub fn labeled_button(ui: &mut Ui, icon: &str, text: &str, tooltip: &str) -> Response {
    labeled(ui, icon, text, None, text, tooltip)
}

/// `is_on`: `None` for a plain button.
fn labeled(
    ui: &mut Ui,
    icon: &str,
    text: &str,
    is_on: Option<bool>,
    name: &str,
    tooltip: &str,
) -> Response {
    let palette = theme::palette(ui.ctx());
    let font = FontId::proportional(style::SMALL_SIZE);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let width = CONTROL.x + galley.size().x + style::SPACE_S;
    let (rect, response) = ui.allocate_exact_size(vec2(width, CONTROL.y), Sense::click());
    response.widget_info(|| match is_on {
        Some(is_on) => {
            egui::WidgetInfo::selected(egui::Role::Button, response.enabled(), is_on, name)
        }
        None => egui::WidgetInfo::labeled(egui::Role::Button, response.enabled(), name),
    });
    let is_on = is_on.unwrap_or(false);
    let icon_rect = Rect::from_min_size(rect.min, CONTROL);
    let on = row::toggle_on(ui, &response, is_on);
    row::paint_toggle_fill(ui, &response, rect, on);
    let color = row::toggle_color(&palette, &response, on);
    ui.painter().text(
        icon_rect.center(),
        Align2::CENTER_CENTER,
        icon,
        icons::state_font(&palette, icons::SIZE, is_on),
        color,
    );
    ui.painter().galley(
        pos2(icon_rect.right() - 2.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    focus::ring(ui, &response, rect);
    response.on_hover_text(tooltip)
}

/// Exits focus mode.
pub fn exit_button(ui: &mut Ui) {
    if button(ui, icons::ARROWS_IN_SIMPLE, EXIT_HINT).clicked() {
        request_toggle(ui.ctx());
    }
}

/// A short vertical hairline between groups of controls.
pub fn separator(ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, CONTROL.y), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.shrink2(vec2(0.0, 4.0)).y_range(),
        Stroke::new(1.0, palette.border),
    );
}

/// Secondary text in a slot at least `min_width` wide, so a counter that counts never moves the
/// controls beside it.
pub fn note(ui: &mut Ui, text: &str, min_width: f32) {
    let palette = theme::palette(ui.ctx());
    let font = FontId::proportional(style::SMALL_SIZE);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, palette.text_secondary);
    let (rect, response) =
        ui.allocate_exact_size(vec2(galley.size().x.max(min_width), CONTROL.y), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
    let at = pos2(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(at, galley, palette.text_secondary);
}

fn paint(ui: &Ui, response: &Response, rect: Rect, icon: &str, is_on: bool) {
    let palette = theme::palette(ui.ctx());
    let on = row::toggle_on(ui, response, is_on);
    row::paint_toggle_fill(ui, response, rect, on);
    let color = row::toggle_color(&palette, response, on);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        icons::state_font(&palette, icons::SIZE, is_on),
        color,
    );
    focus::ring(ui, response, rect);
}
