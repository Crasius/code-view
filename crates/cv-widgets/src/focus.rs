//! Focus rings with `:focus-visible` semantics: they show while the user navigates with the
//! keyboard and disappear as soon as the mouse is used again, as in a browser.

use cv_base::theme;
use egui::{Context, Event, Id, Key, Rect, Response, Stroke, StrokeKind, Ui};

const KEYBOARD_ID: &str = "review.focus.keyboard";
const RING_DRAWN_ID: &str = "review.focus.ring_drawn";

/// Call at the start of every frame: Tab or arrow keys switch to keyboard navigation,
/// any pointer press leaves it.
pub fn track_modality(ctx: &Context) {
    let change = ctx.input(|input| {
        input.events.iter().rev().find_map(|event| match event {
            Event::Key { key, pressed: true, .. } if moves_focus(*key) => Some(true),
            Event::PointerButton { pressed: true, .. } => Some(false),
            _ => None,
        })
    });
    ctx.data_mut(|data| {
        if let Some(is_keyboard) = change {
            data.insert_temp(Id::unique(KEYBOARD_ID), is_keyboard);
        }
        data.remove::<Id>(Id::unique(RING_DRAWN_ID));
    });
}

/// A view's own keys (such as n/p between cards) moved its keyboard cursor: focus shows as after
/// Tab, until the next pointer press.
pub fn use_keyboard(ctx: &Context) {
    ctx.data_mut(|data| data.insert_temp(Id::unique(KEYBOARD_ID), true));
}

/// True while focus should be visible (keyboard navigation).
pub fn is_visible(ctx: &Context) -> bool {
    ctx.data(|data| data.get_temp(Id::unique(KEYBOARD_ID))).unwrap_or(false)
}

/// Keyboard focus outline for a custom-painted row or card, drawn at `rect`.
pub fn ring(ui: &Ui, response: &Response, rect: Rect) {
    if !response.has_focus() || !is_visible(ui.ctx()) {
        return;
    }
    paint(ui.painter(), rect.shrink(1.0), &theme::palette(ui.ctx()));
    ui.ctx().data_mut(|data| data.insert_temp(Id::unique(RING_DRAWN_ID), response.id));
}

/// Call at the end of every frame: outlines the focused built-in widget (buttons, menus,
/// checkboxes) unless it drew its own ring or is a text input, whose border shows focus.
pub fn paint_for_focused_widget(ctx: &Context) {
    if !is_visible(ctx) {
        return;
    }
    let Some(id) = ctx.memory(egui::Memory::focused) else {
        return;
    };
    let has_own_ring = ctx.data(|data| data.get_temp::<Id>(Id::unique(RING_DRAWN_ID))) == Some(id);
    if has_own_ring || egui::TextEdit::load_state(ctx, id).is_some() {
        return;
    }
    let Some(response) = ctx.read_response(id) else {
        return;
    };
    let rect = response.interact_rect;
    if !rect.is_positive() {
        return;
    }
    paint(&ctx.layer_painter(response.layer_id), rect.expand(1.0), &theme::palette(ctx));
}

fn paint(painter: &egui::Painter, rect: Rect, palette: &theme::Palette) {
    painter.rect_stroke(
        rect,
        theme::style::RADIUS_CONTROL,
        Stroke::new(2.0, palette.accent),
        StrokeKind::Inside,
    );
}

fn moves_focus(key: Key) -> bool {
    matches!(key, Key::Tab | Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight)
}
